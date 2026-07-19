use crate::daemon::source_availability::{
    BoxFuture, MountInteraction, SourceMountBackend, SourceUnavailable,
};
use crate::source_location::{SourceIdentity, SourceLocation};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use tokio::sync::oneshot;

pub struct MacosNetFsBackend;

struct MountCompletion {
    sender: Option<oneshot::Sender<Result<PathBuf, SourceUnavailable>>>,
}

unsafe extern "C" {
    fn classick_netfs_mount_async(
        url: *const c_char,
        allow_ui: c_int,
        completion: unsafe extern "C" fn(*mut c_void, c_int, *const c_char),
        context: *mut c_void,
    ) -> c_int;
}

impl SourceMountBackend for MacosNetFsBackend {
    fn mount<'a>(
        &'a self,
        location: &'a SourceLocation,
        interaction: MountInteraction,
    ) -> BoxFuture<'a, Result<PathBuf, SourceUnavailable>> {
        Box::pin(async move {
            let (host, share) = match &location.identity {
                SourceIdentity::Smb { host, share, .. } => (host.as_str(), share.as_str()),
                SourceIdentity::Local { .. } => {
                    return Err(SourceUnavailable::MissingSubpath(
                        location.resolved_path.clone(),
                    ));
                }
            };
            let url = CString::new(build_smb_url(host, share)?)
                .map_err(|_| SourceUnavailable::MountFailed("invalid SMB source".into()))?;
            let (sender, receiver) = oneshot::channel();

            let immediate_status = unsafe {
                let context = Box::into_raw(Box::new(MountCompletion {
                    sender: Some(sender),
                }));
                let status = classick_netfs_mount_async(
                    url.as_ptr(),
                    i32::from(interaction == MountInteraction::AllowUi),
                    mount_completed,
                    context.cast(),
                );
                if status != 0 {
                    drop(Box::from_raw(context));
                }
                status
            };

            if immediate_status != 0 {
                return Err(map_status(immediate_status));
            }

            receiver.await.unwrap_or_else(|_| {
                Err(SourceUnavailable::MountFailed(
                    "NetFS mount request ended unexpectedly".into(),
                ))
            })
        })
    }
}

unsafe extern "C" fn mount_completed(
    context: *mut c_void,
    status: c_int,
    mountpoint: *const c_char,
) {
    let mut completion = unsafe { Box::from_raw(context.cast::<MountCompletion>()) };
    let result = if status != 0 {
        Err(map_status(status))
    } else if mountpoint.is_null() {
        Err(SourceUnavailable::MountFailed(
            "NetFS returned no mountpoint".into(),
        ))
    } else {
        let bytes = unsafe { CStr::from_ptr(mountpoint) }.to_bytes();
        match std::str::from_utf8(bytes) {
            Ok(path) => Ok(PathBuf::from(path)),
            Err(_) => Err(SourceUnavailable::MountFailed(
                "NetFS returned a non-UTF-8 mountpoint".into(),
            )),
        }
    };
    if let Some(sender) = completion.sender.take() {
        let _ = sender.send(result);
    }
}

fn build_smb_url(host: &str, share: &str) -> Result<String, SourceUnavailable> {
    if host.is_empty()
        || host.bytes().any(|byte| {
            !byte.is_ascii_alphanumeric()
                && !matches!(byte, b'.' | b'-' | b'_' | b':' | b'[' | b']')
        })
        || share.is_empty()
        || share.bytes().any(|byte| {
            byte.is_ascii_control() || matches!(byte, b'/' | b'\\' | b'@' | b'?' | b'#')
        })
    {
        return Err(SourceUnavailable::MountFailed("invalid SMB source".into()));
    }

    let mut encoded_share = String::with_capacity(share.len());
    for byte in share.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded_share.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut encoded_share, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    Ok(format!("smb://{host}/{encoded_share}"))
}

fn map_status(status: c_int) -> SourceUnavailable {
    match status {
        libc::EACCES | libc::EPERM | 80 | -5045 | -5046 => SourceUnavailable::AuthRequired,
        _ => SourceUnavailable::MountFailed(format!("NetFS mount failed (status {status})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_credential_free_smb_url() {
        assert_eq!(
            build_smb_url("jupiter", "data").unwrap(),
            "smb://jupiter/data"
        );
        assert_eq!(
            build_smb_url("JUPITER.local", "Music Archive").unwrap(),
            "smb://JUPITER.local/Music%20Archive"
        );
    }

    #[test]
    fn rejects_authority_or_path_injection() {
        for host in ["alice:secret@jupiter", "jupiter/data", "jupiter\\data", ""] {
            assert!(
                build_smb_url(host, "data").is_err(),
                "accepted host {host:?}"
            );
        }
        for share in ["data/music", "data\\music", "alice@data", ""] {
            assert!(
                build_smb_url("jupiter", share).is_err(),
                "accepted share {share:?}"
            );
        }
    }

    #[test]
    fn maps_authentication_statuses_without_server_diagnostics() {
        for status in [libc::EACCES, libc::EPERM, 80, -5045, -5046] {
            assert_eq!(map_status(status), SourceUnavailable::AuthRequired);
        }
        assert_eq!(
            map_status(libc::ENOENT),
            SourceUnavailable::MountFailed("NetFS mount failed (status 2)".into())
        );
    }
}
