use crate::device::{DeviceId, DeviceReadiness, HardwareFacts, ObservationId};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePhase {
    Disconnected,
    Unconfigured,
    Idle,
    Syncing,
    Paused,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    NotAdopted,
    PendingAdoption,
    Adopted,
    Invalid,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageSnapshot {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub freshness: StorageFreshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageFreshness {
    Live,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifiedDeviceSnapshot {
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub readiness: DeviceReadiness,
    pub hardware: HardwareFacts,
    pub profile_status: ProfileStatus,
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,
    pub phase: DevicePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<super::SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageSnapshot>,
    pub synced_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_terminal_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnidentifiedDeviceSnapshot {
    pub observation_id: ObservationId,
    pub readiness: DeviceReadiness,
    pub hardware: HardwareFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInventorySnapshot {
    pub revision: u64,
    pub devices: Vec<IdentifiedDeviceSnapshot>,
    pub unidentified: Vec<UnidentifiedDeviceSnapshot>,
}

impl DeviceInventorySnapshot {
    pub(super) fn validate(&self) -> Result<()> {
        if self.revision == 0 {
            bail!("inventory revision must be nonzero");
        }
        let mut device_ids = HashSet::new();
        let mut mount_paths = HashSet::new();
        for device in &self.devices {
            if !device_ids.insert(&device.device_id) {
                bail!("inventory repeats device {}", device.device_id);
            }
            if device.name.as_ref().is_some_and(String::is_empty) {
                bail!("device name must not be empty when present");
            }
            if device
                .mount_path
                .as_deref()
                .is_some_and(|path| !is_absolute_native_path(path))
            {
                bail!("device mount path must be an absolute native path");
            }
            if let Some(path) = &device.mount_path {
                if !mount_paths.insert(path) {
                    bail!("inventory repeats connected mount path {path}");
                }
            }
            device.hardware.validate()?;
            if device.readiness == DeviceReadiness::IdentityUnavailable {
                bail!("identified inventory entry cannot be identity-unavailable");
            }
            if device.storage.is_some_and(|storage| {
                storage.total_bytes == 0 || storage.free_bytes > storage.total_bytes
            }) {
                bail!("device storage snapshot is inconsistent");
            }
            if !device.connected
                && (device.mount_path.is_some()
                    || device.session_id.is_some()
                    || device.phase != DevicePhase::Disconnected)
            {
                bail!("disconnected device retains connected-only inventory state");
            }
            if device.connected && device.mount_path.is_none() {
                bail!("connected device requires a mount path");
            }
            if device.connected && device.phase == DevicePhase::Disconnected {
                bail!("connected device cannot use the disconnected phase");
            }
            if !device.connected
                && device
                    .storage
                    .is_some_and(|storage| storage.freshness != StorageFreshness::Cached)
            {
                bail!("disconnected device storage must be marked cached");
            }
            if device.phase == DevicePhase::Syncing && device.session_id.is_none() {
                bail!("syncing device requires a session ID");
            }
            if device.session_id.is_some() && device.phase != DevicePhase::Syncing {
                bail!("session ID is only valid for a syncing device");
            }
            if device.phase == DevicePhase::Syncing
                && (device.readiness != DeviceReadiness::Ready
                    || device.profile_status != ProfileStatus::Adopted)
            {
                bail!("syncing device must be ready and adopted");
            }
        }

        let mut observation_ids = HashSet::new();
        for device in &self.unidentified {
            if !observation_ids.insert(&device.observation_id) {
                bail!("inventory repeats unidentified observation");
            }
            if device.readiness != DeviceReadiness::IdentityUnavailable {
                bail!("unidentified inventory entry must be identity-unavailable");
            }
            device.hardware.validate()?;
        }
        Ok(())
    }
}

fn is_absolute_native_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') {
        return false;
    }

    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with(r"\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}
