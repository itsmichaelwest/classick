use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Review,
    AutoApply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyLevel {
    All,
    ErrorsOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropSyncBehavior {
    Immediate,
    NextSync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalSettings {
    pub first_sync_mode: SyncMode,
    pub subsequent_sync_mode: SyncMode,
    pub schedule_minutes: u32,
    pub notify_on: NotifyLevel,
    pub drop_sync_behavior: DropSyncBehavior,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRoot(String);

impl SourceRoot {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() || value.chars().any(char::is_control) {
            bail!("source root must be a non-empty path without control characters");
        }
        if value
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("smb://"))
        {
            let remainder = &value[6..];
            let (authority, share) = remainder
                .split_once('/')
                .ok_or_else(|| anyhow::anyhow!("SMB source root must include a share"))?;
            if authority.is_empty()
                || share.is_empty()
                || authority.contains('@')
                || value.contains('?')
                || value.contains('#')
            {
                bail!("SMB source root must not contain credentials, queries, or fragments");
            }
        } else if !super::inventory::is_absolute_native_path(value) {
            bail!("source root must be an absolute native path or credential-free SMB URL");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SourceRoot {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SourceRoot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailabilityState {
    Available,
    Remounting,
    AuthRequired,
    Unavailable,
}
