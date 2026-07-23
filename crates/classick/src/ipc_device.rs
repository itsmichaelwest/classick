//! Serial-keyed device inventory values published on the daemon IPC wire.

use crate::daemon::device_storage::StorageInfo;
use crate::daemon::history::HistoryEntry;
use crate::device::HardwareFacts;
use serde::{Deserialize, Serialize};

pub type SessionId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInventorySnapshot {
    pub revision: u64,
    pub devices: Vec<DeviceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentitySnapshot {
    pub serial: String,
    pub model_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePhaseLabel {
    Disconnected,
    Unconfigured,
    Idle,
    Syncing,
    Paused,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSnapshot {
    pub identity: DeviceIdentitySnapshot,
    #[serde(skip)]
    pub hardware: HardwareFacts,
    pub configured: bool,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    pub phase: DevicePhaseLabel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageInfo>,
    pub synced_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_successful_sync: Option<HistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_attempt: Option<HistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_terminal_error: Option<String>,
    pub selection_revision: u64,
    pub settings_revision: u64,
    pub subscriptions_revision: u64,
}
