use crate::portable::profile::PlaylistSlug;
use crate::portable::profile::ProfilePath;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistKind {
    Manual,
    Smart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartMatch {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartField {
    Artist,
    Album,
    Genre,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartOperator {
    Is,
    Contains,
    Gte,
    Lte,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartRule {
    pub field: SmartField,
    pub op: SmartOperator,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartLimit {
    Bytes(u64),
    Tracks(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartOrder {
    RecentlyModified,
    RandomStable,
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartRules {
    pub version: u32,
    pub matching: SmartMatch,
    pub rules: Vec<SmartRule>,
    pub limit: Option<SmartLimit>,
    pub order: SmartOrder,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaylistDraft {
    Manual {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<PlaylistSlug>,
        name: String,
        tracks: Vec<ProfilePath>,
    },
    Smart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<PlaylistSlug>,
        name: String,
        rules: SmartRules,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoredPlaylist {
    Manual {
        slug: PlaylistSlug,
        name: String,
        tracks: Vec<ProfilePath>,
    },
    Smart {
        slug: PlaylistSlug,
        name: String,
        rules: SmartRules,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistSummary {
    pub slug: PlaylistSlug,
    pub name: String,
    pub kind: PlaylistKind,
    pub tracks: u64,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaylistDetailResult {
    Found { playlist: StoredPlaylist },
    Unavailable { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LibraryMutationTarget {
    DeviceSelection { device_id: crate::device::DeviceId },
    ManualPlaylist { slug: PlaylistSlug },
}

impl PlaylistDraft {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Manual { name, .. } => validate_name(name),
            Self::Smart { name, rules, .. } => {
                validate_name(name)?;
                rules.validate()
            }
        }
    }
}

impl StoredPlaylist {
    pub(super) fn slug(&self) -> &PlaylistSlug {
        match self {
            Self::Manual { slug, .. } | Self::Smart { slug, .. } => slug,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Manual { name, .. } => validate_name(name),
            Self::Smart { name, rules, .. } => {
                validate_name(name)?;
                rules.validate()
            }
        }
    }
}

impl SmartRules {
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported smart-playlist rule version");
        }
        if self
            .rules
            .iter()
            .any(|rule| rule.value.is_empty() || rule.value.chars().any(char::is_control))
        {
            bail!("smart-playlist rules require non-empty safe values");
        }
        if self
            .limit
            .is_some_and(|limit| matches!(limit, SmartLimit::Bytes(0) | SmartLimit::Tracks(0)))
        {
            bail!("smart-playlist limits must be nonzero");
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.chars().any(char::is_control) {
        bail!("playlist name must not be empty or contain control characters");
    }
    Ok(())
}
