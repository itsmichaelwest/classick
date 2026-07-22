use super::host_cache::{deserialize_canonical_device_id, validate_existing_host_path};
use super::profile::{MutationId, SelectionValue, SettingsValue, SubscriptionsValue};
use super::profile_values::COMPONENT_SCHEMA_VERSION;
use crate::device::DeviceId;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

pub const OUTBOX_SCHEMA_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationState {
    PendingDevice,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "component", rename_all = "snake_case", deny_unknown_fields)]
pub enum PendingMutation {
    Selection {
        mutation_id: MutationId,
        #[serde(deserialize_with = "deserialize_canonical_device_id")]
        device_id: DeviceId,
        desired: SelectionValue,
        last_imported_device_revision: u64,
        state: MutationState,
    },
    Settings {
        mutation_id: MutationId,
        #[serde(deserialize_with = "deserialize_canonical_device_id")]
        device_id: DeviceId,
        desired: SettingsValue,
        last_imported_device_revision: u64,
        state: MutationState,
    },
    Subscriptions {
        mutation_id: MutationId,
        #[serde(deserialize_with = "deserialize_canonical_device_id")]
        device_id: DeviceId,
        desired: SubscriptionsValue,
        last_imported_device_revision: u64,
        state: MutationState,
    },
}
impl PendingMutation {
    pub fn selection(
        mutation_id: MutationId,
        device_id: DeviceId,
        desired: SelectionValue,
        last_imported_device_revision: u64,
    ) -> Result<Self> {
        Self::validated(Self::Selection {
            mutation_id,
            device_id,
            desired,
            last_imported_device_revision,
            state: MutationState::PendingDevice,
        })
    }
    pub fn settings(
        mutation_id: MutationId,
        device_id: DeviceId,
        desired: SettingsValue,
        last_imported_device_revision: u64,
    ) -> Result<Self> {
        Self::validated(Self::Settings {
            mutation_id,
            device_id,
            desired,
            last_imported_device_revision,
            state: MutationState::PendingDevice,
        })
    }
    pub fn subscriptions(
        mutation_id: MutationId,
        device_id: DeviceId,
        desired: SubscriptionsValue,
        last_imported_device_revision: u64,
    ) -> Result<Self> {
        Self::validated(Self::Subscriptions {
            mutation_id,
            device_id,
            desired,
            last_imported_device_revision,
            state: MutationState::PendingDevice,
        })
    }
    fn validated(mutation: Self) -> Result<Self> {
        mutation.validate()?;
        Ok(mutation)
    }
    pub fn mutation_id(&self) -> &MutationId {
        match self {
            Self::Selection { mutation_id, .. }
            | Self::Settings { mutation_id, .. }
            | Self::Subscriptions { mutation_id, .. } => mutation_id,
        }
    }
    pub fn device_id(&self) -> &DeviceId {
        match self {
            Self::Selection { device_id, .. }
            | Self::Settings { device_id, .. }
            | Self::Subscriptions { device_id, .. } => device_id,
        }
    }
    pub fn component_name(&self) -> &'static str {
        match self {
            Self::Selection { .. } => "selection",
            Self::Settings { .. } => "settings",
            Self::Subscriptions { .. } => "subscriptions",
        }
    }
    fn component_order(&self) -> u8 {
        match self {
            Self::Selection { .. } => 0,
            Self::Settings { .. } => 1,
            Self::Subscriptions { .. } => 2,
        }
    }
    fn validate(&self) -> Result<()> {
        let (name, schema_version) = match self {
            Self::Selection { desired, .. } => ("selection", desired.schema_version),
            Self::Settings { desired, .. } => ("settings", desired.schema_version),
            Self::Subscriptions { desired, .. } => {
                let mut unique = HashSet::new();
                if desired.playlists.iter().any(|slug| !unique.insert(slug)) {
                    bail!("duplicate pending subscription slug");
                }
                ("subscriptions", desired.schema_version)
            }
        };
        if schema_version != COMPONENT_SCHEMA_VERSION {
            bail!("unsupported pending {name} schema {schema_version}");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingDeviceOutbox {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_canonical_device_id")]
    pub device_id: DeviceId,
    pub mutations: Vec<PendingMutation>,
}
impl PendingDeviceOutbox {
    pub fn empty(device_id: DeviceId) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION,
            device_id,
            mutations: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != OUTBOX_SCHEMA_VERSION {
            bail!("unsupported host outbox schema {}", self.schema_version);
        }
        let mut mutation_ids = HashSet::new();
        let mut previous_component = None;
        for mutation in &self.mutations {
            mutation.validate()?;
            if mutation.device_id() != &self.device_id {
                bail!("pending mutation device ID does not match host outbox");
            }
            if !mutation_ids.insert(mutation.mutation_id()) {
                bail!("duplicate pending mutation ID {}", mutation.mutation_id());
            }
            let order = mutation.component_order();
            if previous_component.is_some_and(|previous| previous >= order) {
                bail!("pending mutations are not in deterministic component order");
            }
            previous_component = Some(order);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxLoad {
    Missing(PendingDeviceOutbox),
    Loaded(PendingDeviceOutbox),
}

#[derive(Debug, Clone)]
/// Read-only access to pending device mutations.
///
/// ```compile_fail
/// use classick::portable::outbox::PendingOutboxStore;
/// let _ = PendingOutboxStore::save;
/// ```
///
/// ```compile_fail
/// use classick::portable::outbox::PendingOutboxStore;
/// let _ = PendingOutboxStore::accept;
/// ```
///
/// ```compile_fail
/// use classick::portable::outbox::PendingOutboxStore;
/// let _ = PendingOutboxStore::confirm;
/// ```
///
/// ```compile_fail
/// use classick::portable::outbox::PendingOutboxStore;
/// let _ = PendingOutboxStore::with_writer;
/// ```
pub struct PendingOutboxStore {
    root: PathBuf,
}
impl PendingOutboxStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self, device_id: &DeviceId) -> PathBuf {
        self.root
            .join("devices")
            .join(device_id.as_str())
            .join("outbox.json")
    }

    pub fn load(&self, device_id: &DeviceId) -> Result<OutboxLoad> {
        let path = self.path(device_id);
        validate_existing_host_path(&self.root, &path)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(OutboxLoad::Missing(PendingDeviceOutbox::empty(
                    device_id.clone(),
                )));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read host outbox {}", path.display()));
            }
        };
        let outbox = parse_outbox(&bytes)
            .with_context(|| format!("parse host outbox {}", path.display()))?;
        if &outbox.device_id != device_id {
            bail!("host outbox device ID does not match its device directory");
        }
        Ok(OutboxLoad::Loaded(outbox))
    }
}

// Pure planning only; the future lease-owning coordinator decides when it may be persisted.
#[allow(dead_code)]
pub(super) fn coalesce_pending(
    current: &PendingDeviceOutbox,
    mutation: &PendingMutation,
) -> Result<PendingDeviceOutbox> {
    current.validate()?;
    mutation.validate()?;
    if mutation.device_id() != &current.device_id {
        bail!("pending mutation device ID does not match host outbox");
    }
    if let Some(existing) = current
        .mutations
        .iter()
        .find(|existing| existing.mutation_id() == mutation.mutation_id())
    {
        if existing == mutation {
            return Ok(current.clone());
        }
        bail!("pending mutation ID was reused with different contents");
    }
    let mut next = current.clone();
    next.mutations
        .retain(|existing| existing.component_order() != mutation.component_order());
    next.mutations.push(mutation.clone());
    next.mutations.sort_by_key(PendingMutation::component_order);
    next.validate()?;
    Ok(next)
}

// Retained for the future lease-owning coordinator; this module exposes no write operation.
#[allow(dead_code)]
pub(super) fn serialize_outbox(outbox: &PendingDeviceOutbox) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(outbox)?;
    bytes.push(b'\n');
    let reparsed = parse_outbox(&bytes).context("reparse serialized host outbox")?;
    if &reparsed != outbox {
        bail!("serialized host outbox differs after exact reparse");
    }
    Ok(bytes)
}

fn parse_outbox(bytes: &[u8]) -> Result<PendingDeviceOutbox> {
    let outbox: PendingDeviceOutbox = serde_json::from_slice(bytes)?;
    outbox.validate()?;
    Ok(outbox)
}

#[cfg(test)]
mod tests;
