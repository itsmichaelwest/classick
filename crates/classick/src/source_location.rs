use crate::portable_path::PortablePath;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceIdentity {
    Smb {
        host: String,
        share: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<PortablePath>,
    },
    Local {
        library_id: String,
    },
}

impl PartialEq for SourceIdentity {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Smb {
                    host: left_host,
                    share: left_share,
                    subpath: left_subpath,
                },
                Self::Smb {
                    host: right_host,
                    share: right_share,
                    subpath: right_subpath,
                },
            ) => {
                left_host.eq_ignore_ascii_case(right_host)
                    && left_share.eq_ignore_ascii_case(right_share)
                    && match (left_subpath, right_subpath) {
                        (Some(left), Some(right)) => {
                            left.as_str().to_lowercase() == right.as_str().to_lowercase()
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            (Self::Local { library_id: left }, Self::Local { library_id: right }) => left == right,
            _ => false,
        }
    }
}

impl Eq for SourceIdentity {}

impl SourceIdentity {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Smb { host, share, .. } => validate_smb_host_and_share(host, share),
            Self::Local { library_id } if library_id.trim().is_empty() => {
                bail!("local source identity is empty")
            }
            Self::Local { library_id } if library_id.chars().any(char::is_control) => {
                bail!("local source identity contains control characters")
            }
            Self::Local { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub resolved_path: PathBuf,
    pub identity: SourceIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceIdentityMismatch;

impl std::fmt::Display for SourceIdentityMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("resolved source is a different SMB location")
    }
}

impl std::error::Error for SourceIdentityMismatch {}

impl SourceLocation {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()
    }

    pub fn verify_identity(
        &self,
        actual: &SourceIdentity,
    ) -> std::result::Result<(), SourceIdentityMismatch> {
        match &self.identity {
            SourceIdentity::Smb { .. } if &self.identity != actual => Err(SourceIdentityMismatch),
            _ => Ok(()),
        }
    }

    pub fn verify_resolved_identity(&self) -> Result<()> {
        self.validate()?;
        if !matches!(self.identity, SourceIdentity::Smb { .. }) {
            return Ok(());
        }
        let actual = Self::discover(self.resolved_path.clone())?;
        actual.validate()?;
        self.verify_identity(&actual.identity)?;
        Ok(())
    }

    pub fn discover(resolved_path: PathBuf) -> Result<Self> {
        let value = resolved_path
            .to_str()
            .context("source path is not valid UTF-8")?;

        if let Some(identity) = parse_unc_identity(value)? {
            return Ok(Self {
                resolved_path,
                identity,
            });
        }
        if value
            .get(..6)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("smb://"))
        {
            let identity = parse_smb_url_identity(value)?;
            return Ok(Self {
                resolved_path,
                identity,
            });
        }

        #[cfg(target_os = "macos")]
        if resolved_path.is_absolute() {
            if let Some(location) =
                crate::daemon::macos_netfs::source_location_for_mounted_path(&resolved_path)?
            {
                return Ok(location);
            }
        }

        Ok(Self {
            identity: SourceIdentity::Local {
                library_id: stable_local_library_id(&resolved_path)?,
            },
            resolved_path,
        })
    }
}

fn parse_unc_identity(value: &str) -> Result<Option<SourceIdentity>> {
    if !value.starts_with(r"\\") && !value.starts_with("//") {
        return Ok(None);
    }
    let normalized = value.replace('\\', "/");
    let mut components = normalized[2..].split('/');
    let host = components.next().unwrap_or_default();
    let share = components.next().unwrap_or_default();
    validate_smb_host_and_share(host, share)?;
    let mut remainder = components.map(str::to_owned).collect::<Vec<_>>();
    while remainder.last().is_some_and(String::is_empty) {
        remainder.pop();
    }
    if remainder.iter().any(String::is_empty) {
        bail!("UNC source contains an empty path component");
    }
    let subpath = portable_subpath(&remainder)?;
    Ok(Some(SourceIdentity::Smb {
        host: host.to_owned(),
        share: share.to_owned(),
        subpath,
    }))
}

fn parse_smb_url_identity(value: &str) -> Result<SourceIdentity> {
    let remainder = &value[6..];
    let (authority, path) = remainder
        .split_once('/')
        .context("SMB URL must include a share")?;
    if authority.contains('@') {
        bail!("SMB URL must not contain user information");
    }
    if path.contains('?') || path.contains('#') {
        bail!("SMB URL must not contain a query or fragment");
    }
    let mut components = path
        .split('/')
        .map(percent_decode_url_component)
        .collect::<Result<Vec<_>>>()?;
    while components.last().is_some_and(String::is_empty) {
        components.pop();
    }
    let (share, remainder) = components
        .split_first()
        .context("SMB URL must include a share")?;
    validate_smb_host_and_share(authority, share)?;
    if remainder.iter().any(String::is_empty) {
        bail!("SMB URL contains an empty path component");
    }
    Ok(SourceIdentity::Smb {
        host: authority.to_owned(),
        share: share.clone(),
        subpath: portable_subpath(remainder)?,
    })
}

fn validate_smb_host_and_share(host: &str, share: &str) -> Result<()> {
    if host.is_empty() || share.is_empty() {
        bail!("SMB source must include a host and share");
    }
    if host.contains('@') || host.contains('/') || host.contains('\\') {
        bail!("SMB host contains user information or a path");
    }
    if share.contains('@') || share.contains('/') || share.contains('\\') {
        bail!("SMB share contains user information or a path");
    }
    if host.chars().any(char::is_control) || share.chars().any(char::is_control) {
        bail!("SMB source contains control characters");
    }
    Ok(())
}

fn portable_subpath(components: &[String]) -> Result<Option<PortablePath>> {
    if components.is_empty() {
        return Ok(None);
    }
    Ok(Some(PortablePath::parse(&components.join("/"))?))
}

fn percent_decode_url_component(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = *bytes.get(index + 1).context("invalid SMB URL escape")?;
        let low = *bytes.get(index + 2).context("invalid SMB URL escape")?;
        decoded.push((decode_hex(high)? << 4) | decode_hex(low)?);
        index += 3;
    }
    let decoded = String::from_utf8(decoded).context("SMB URL path is not valid UTF-8")?;
    if decoded.contains('/') || decoded.contains('\\') || decoded.chars().any(char::is_control) {
        bail!("SMB URL contains an invalid escaped path component");
    }
    Ok(decoded)
}

fn decode_hex(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("invalid SMB URL escape"),
    }
}

fn stable_local_library_id(path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve current directory for local source identity")?
            .join(path)
    };
    let normalized = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let value = normalized
        .to_str()
        .context("source path is not valid UTF-8")?;
    Ok(format!(
        "local-v1:{}",
        blake3::hash(value.as_bytes()).to_hex()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smb_host_and_share_compare_case_insensitively() {
        let a = SourceIdentity::Smb {
            host: "JUPITER".into(),
            share: "Data".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };
        let b = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "data".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };

        assert_eq!(a, b);
    }

    #[test]
    fn smb_subpath_compares_case_insensitively() {
        let a = SourceIdentity::Smb {
            host: "JUPITER".into(),
            share: "Data".into(),
            subpath: Some(PortablePath::parse("Media/Music").unwrap()),
        };
        let b = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "data".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };

        assert_eq!(a, b);
    }

    #[test]
    fn discovers_windows_unc_sources_on_any_host() {
        let location =
            SourceLocation::discover(PathBuf::from(r"\\JUPITER\Data\media\music")).unwrap();

        assert_eq!(
            location.identity,
            SourceIdentity::Smb {
                host: "JUPITER".into(),
                share: "Data".into(),
                subpath: Some(PortablePath::parse("media/music").unwrap()),
            }
        );
    }

    #[test]
    fn accepts_a_trailing_separator_at_the_smb_share_root() {
        let unc = SourceLocation::discover(PathBuf::from(r"\\JUPITER\Data\")).unwrap();
        let url = SourceLocation::discover(PathBuf::from("smb://JUPITER/Data/")).unwrap();

        assert_eq!(unc.identity, url.identity);
        assert!(matches!(
            unc.identity,
            SourceIdentity::Smb { subpath: None, .. }
        ));
    }

    #[test]
    fn discovers_credential_free_smb_urls() {
        let location =
            SourceLocation::discover(PathBuf::from("smb://JUPITER/Data/media/My%20Music")).unwrap();

        assert_eq!(
            location.identity,
            SourceIdentity::Smb {
                host: "JUPITER".into(),
                share: "Data".into(),
                subpath: Some(PortablePath::parse("media/My Music").unwrap()),
            }
        );
    }

    #[test]
    fn rejects_smb_urls_with_user_information() {
        for value in [
            "smb://alice@jupiter/data/music",
            "smb://alice:secret@jupiter/data/music",
        ] {
            assert!(
                SourceLocation::discover(PathBuf::from(value)).is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn local_identity_is_stable_for_the_same_path_and_changes_with_the_source() {
        let root = std::env::temp_dir().join(format!(
            "classick-local-source-identity-{}",
            std::process::id()
        ));
        let music = root.join("Music");
        let archive = root.join("Archive");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        let first = SourceLocation::discover(music.clone()).unwrap();
        let repeated = SourceLocation::discover(music).unwrap();
        let changed = SourceLocation::discover(archive).unwrap();

        assert_eq!(first.identity, repeated.identity);
        assert_ne!(first.identity, changed.identity);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn different_smb_share_or_subpath_does_not_match() {
        let music = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "data".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };
        let archive = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "archive".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };
        let videos = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "data".into(),
            subpath: Some(PortablePath::parse("media/videos").unwrap()),
        };

        assert_ne!(music, archive);
        assert_ne!(music, videos);
    }

    #[test]
    fn local_library_identity_is_independent_of_resolved_path() {
        let mac = SourceLocation {
            resolved_path: PathBuf::from("/Volumes/Music"),
            identity: SourceIdentity::Local {
                library_id: "library-123".into(),
            },
        };
        let windows = SourceLocation {
            resolved_path: PathBuf::from("D:/Music"),
            identity: SourceIdentity::Local {
                library_id: "library-123".into(),
            },
        };

        assert_eq!(mac.identity, windows.identity);
        assert_ne!(mac, windows);
    }

    #[test]
    fn local_library_ids_must_match() {
        assert_ne!(
            SourceIdentity::Local {
                library_id: "library-a".into()
            },
            SourceIdentity::Local {
                library_id: "library-b".into()
            }
        );
    }

    #[test]
    fn verification_rejects_a_different_live_smb_share_without_exposing_identity() {
        let configured = SourceLocation {
            resolved_path: PathBuf::from("/Volumes/data/media/music"),
            identity: SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(PortablePath::parse("media/music").unwrap()),
            },
        };
        let actual = SourceIdentity::Smb {
            host: "jupiter".into(),
            share: "archive".into(),
            subpath: Some(PortablePath::parse("media/music").unwrap()),
        };

        let error = configured.verify_identity(&actual).unwrap_err();
        assert_eq!(
            error.to_string(),
            "resolved source is a different SMB location"
        );
        assert!(!error.to_string().contains("jupiter"));
    }

    #[test]
    fn verification_preserves_local_source_behavior() {
        let configured = SourceLocation {
            resolved_path: PathBuf::from("/Users/test/Music"),
            identity: SourceIdentity::Local {
                library_id: "local-library".into(),
            },
        };

        configured.verify_resolved_identity().unwrap();
    }
}
