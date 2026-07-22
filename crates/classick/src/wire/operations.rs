use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTrigger {
    Manual,
    Scheduled,
    PlugIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryTrigger {
    Manual,
    Scheduled,
    PlugIn,
    Coalesced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperation {
    Sync,
    BackfillRockbox,
    ReplaceLibrary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejectReason {
    AlreadyRunning,
    DeviceDisconnected,
    NotAdopted,
    NeedsAppleInitialization,
    InvalidDatabase,
    SourceUnavailable,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropSyncDisposition {
    Started { session_id: super::SessionId },
    NextSync,
    AlreadyPresent,
}
