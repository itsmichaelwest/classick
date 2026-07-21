use super::profile_scalars::{MutationId, PlaylistSlug};
use serde::{Deserialize, Serialize};

pub const COMPONENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revised<T> {
    pub revision: u64,
    pub mutation_id: MutationId,
    pub value: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    All,
    Include,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionRule {
    Artist { name: String },
    Album { artist: String, album: String },
    Genre { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionValue {
    pub schema_version: u32,
    pub mode: SelectionMode,
    pub rules: Vec<SelectionRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsValue {
    pub schema_version: u32,
    pub auto_sync: bool,
    pub rockbox_compat: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionsValue {
    pub schema_version: u32,
    pub playlists: Vec<PlaylistSlug>,
}
