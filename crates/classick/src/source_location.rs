use crate::portable_path::PortablePath;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
                    && left_subpath == right_subpath
            }
            (Self::Local { library_id: left }, Self::Local { library_id: right }) => left == right,
            _ => false,
        }
    }
}

impl Eq for SourceIdentity {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub resolved_path: PathBuf,
    pub identity: SourceIdentity,
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
}
