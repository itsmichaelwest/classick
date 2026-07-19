use crate::daemon::source_availability::{
    BoxFuture, MountInteraction, SourceMountBackend, SourceUnavailable,
};
use crate::source_location::{SourceIdentity, SourceLocation};
use anyhow::{bail, Context, Result};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use tokio::sync::oneshot;

pub struct MacosNetFsBackend;

struct MountCompletion {
    sender: Option<oneshot::Sender<Result<PathBuf, SourceUnavailable>>>,
}

struct NetFsString(*mut c_char);

impl NetFsString {
    fn is_null(&self) -> bool {
        self.0.is_null()
    }

    fn to_utf8(&self, label: &str) -> Result<String> {
        unsafe { CStr::from_ptr(self.0) }
            .to_str()
            .with_context(|| format!("NetFS returned a non-UTF-8 {label}"))
            .map(str::to_owned)
    }
}

impl Drop for NetFsString {
    fn drop(&mut self) {
        unsafe { classick_netfs_free_string(self.0) };
    }
}

unsafe extern "C" {
    fn classick_netfs_copy_remount_info(
        path: *const c_char,
        host: *mut *mut c_char,
        share_path: *mut *mut c_char,
        mount_root: *mut *mut c_char,
    ) -> c_int;
    fn classick_netfs_free_string(value: *mut c_char);
    fn classick_netfs_mount_async(
        url: *const c_char,
        allow_ui: c_int,
        completion: unsafe extern "C" fn(*mut c_void, c_int, *const c_char),
        context: *mut c_void,
    ) -> c_int;
}

pub fn source_location_for_mounted_path(path: &std::path::Path) -> Result<Option<SourceLocation>> {
    let path_utf8 = path.to_str().context("source path is not valid UTF-8")?;
    let path_c = CString::new(path_utf8).context("source path contains an interior NUL")?;
    let mut host = std::ptr::null_mut();
    let mut share_path = std::ptr::null_mut();
    let mut mount_root = std::ptr::null_mut();
    let status = unsafe {
        classick_netfs_copy_remount_info(
            path_c.as_ptr(),
            &mut host,
            &mut share_path,
            &mut mount_root,
        )
    };
    if status != 0 {
        bail!("NetFS source identity lookup failed (status {status})");
    }
    let host = NetFsString(host);
    let share_path = NetFsString(share_path);
    let mount_root = NetFsString(mount_root);
    if host.is_null() && share_path.is_null() && mount_root.is_null() {
        return Ok(None);
    }
    if host.is_null() || share_path.is_null() || mount_root.is_null() {
        bail!("NetFS returned incomplete source identity");
    }

    let host_value = host.to_utf8("host")?;
    let share_path_value = share_path.to_utf8("share path")?;
    let mount_root_value = mount_root.to_utf8("mount root")?;

    source_location_from_mounted_parts(
        path.to_path_buf(),
        PathBuf::from(mount_root_value),
        host_value,
        share_path_value,
    )
    .map(Some)
}

fn source_location_from_mounted_parts(
    resolved_path: PathBuf,
    mount_root: PathBuf,
    host: String,
    share_path: String,
) -> Result<SourceLocation> {
    let share = share_path
        .trim_matches('/')
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .context("NetFS remount URL did not include a share")?;
    let relative = resolved_path.strip_prefix(&mount_root).with_context(|| {
        format!(
            "source {} is outside NetFS mount root {}",
            resolved_path.display(),
            mount_root.display()
        )
    })?;
    let subpath = if relative.as_os_str().is_empty() {
        None
    } else {
        Some(crate::portable_path::PortablePath::from_absolute(
            &mount_root,
            &resolved_path,
        )?)
    };
    Ok(SourceLocation {
        resolved_path,
        identity: SourceIdentity::Smb {
            host,
            share: share.to_owned(),
            subpath,
        },
    })
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

    #[test]
    fn derives_share_identity_from_the_actual_mount_root() {
        let location = source_location_from_mounted_parts(
            PathBuf::from("/Volumes/data-1/media/music"),
            PathBuf::from("/Volumes/data-1"),
            "JUPITER".into(),
            "/Data".into(),
        )
        .unwrap();

        assert_eq!(
            location.identity,
            SourceIdentity::Smb {
                host: "JUPITER".into(),
                share: "Data".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("media/music").unwrap()),
            }
        );
    }
}
