use super::{HistoryTrigger, SessionId, SyncOperation};
use crate::device::DeviceId;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcome {
    Ok,
    Error,
    Aborted,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySummary {
    pub add: u64,
    pub modify: u64,
    pub metadata_only: u64,
    pub remove: u64,
    pub unchanged: u64,
    pub skipped: u64,
    pub skipped_for_space_tracks: u64,
    pub skipped_for_space_bytes: u64,
    pub artwork_failed_sources: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub timestamp: String,
    pub duration_secs: u64,
    pub trigger: HistoryTrigger,
    pub operation: SyncOperation,
    pub outcome: SyncOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<HistorySummary>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub db_restored: bool,
}

impl HistoryEntry {
    pub(super) fn validate(&self) -> Result<()> {
        if self.timestamp.is_empty() || self.timestamp.chars().any(char::is_control) {
            bail!("history timestamp must not be empty or contain control characters");
        }
        if self.error_message.as_ref().is_some_and(String::is_empty) {
            bail!("history error message must not be empty when present");
        }
        if self.outcome == SyncOutcome::Ok && self.error_message.is_some() {
            bail!("successful history entry cannot carry an error");
        }
        Ok(())
    }
}
