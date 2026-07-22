//! Canonical registry-v2 schema and pure legacy migration planning.
//!
//! This module performs no filesystem writes. The lease-owning host-state
//! coordinator is responsible for publishing a ready plan and only then
//! retiring legacy inputs.

use crate::device::{
    DeviceId, DeviceReadiness, Fact, FactConfidence, FactSource, HardwareFacts, IpodColour,
    IpodFamily,
};
use crate::portable::profile::PORTABLE_PROFILE_SCHEMA_VERSION;
use anyhow::{bail, Result};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const REGISTRY_V2_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_label: Option<String>,
    #[serde(default)]
    pub hardware_facts: RegistryHardwareFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryFact<T> {
    pub value: T,
    pub source: FactSource,
    pub confidence: FactConfidence,
}

impl<T: Clone> From<&Fact<T>> for RegistryFact<T> {
    fn from(fact: &Fact<T>) -> Self {
        Self {
            value: fact.value.clone(),
            source: fact.source,
            confidence: fact.confidence,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryHardwareFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<RegistryFact<IpodFamily>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<RegistryFact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_code: Option<RegistryFact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<RegistryFact<IpodColour>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<RegistryFact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_bytes: Option<RegistryFact<u64>>,
}

impl From<&HardwareFacts> for RegistryHardwareFacts {
    fn from(facts: &HardwareFacts) -> Self {
        Self {
            family: facts.family.as_ref().map(RegistryFact::from),
            generation: facts.generation.as_ref().map(RegistryFact::from),
            model_code: facts.model_code.as_ref().map(RegistryFact::from),
            colour: facts.colour.as_ref().map(RegistryFact::from),
            firmware: facts.firmware.as_ref().map(RegistryFact::from),
            capacity_bytes: facts.capacity_bytes.as_ref().map(RegistryFact::from),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryStorageInfo {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedProfileSummary {
    pub schema_version: u32,
    pub selection_revision: u64,
    pub settings_revision: u64,
    pub subscriptions_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistryMigrationStatus {
    Complete,
    PendingLegacyImport {
        selection_revision: u64,
        settings_revision: u64,
        subscriptions_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryDeviceRecordV2 {
    pub configured: bool,
    pub presentation: RegistryPresentation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_unix_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_storage: Option<RegistryStorageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_readiness: Option<DeviceReadiness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_imported_profile: Option<ImportedProfileSummary>,
    pub migration_status: RegistryMigrationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRegistryV2 {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_devices")]
    devices: BTreeMap<String, RegistryDeviceRecordV2>,
}

impl DeviceRegistryV2 {
    pub fn from_json(json: &str) -> Result<Self> {
        let registry: Self = serde_json::from_str(json)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        self.validate()?;
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn device(&self, device_id: &DeviceId) -> Option<&RegistryDeviceRecordV2> {
        self.devices.get(device_id.as_str())
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != REGISTRY_V2_SCHEMA_VERSION {
            bail!("unsupported device registry schema {}", self.schema_version);
        }
        for (key, record) in &self.devices {
            let device_id = DeviceId::parse(key)
                .map_err(|error| anyhow::anyhow!("invalid registry device key {key:?}: {error}"))?;
            if key != device_id.as_str() {
                bail!("registry device key must use canonical uppercase spelling");
            }
            if let Some(storage) = record.last_storage {
                if storage.free_bytes > storage.total_bytes {
                    bail!("registry storage free bytes exceed total bytes");
                }
            }
            if let Some(summary) = &record.last_imported_profile {
                if summary.schema_version != PORTABLE_PROFILE_SCHEMA_VERSION
                    || summary.selection_revision == 0
                    || summary.settings_revision == 0
                    || summary.subscriptions_revision == 0
                {
                    bail!("imported portable profile summary has an unsupported schema or zero revision");
                }
            }
            match (
                &record.migration_status,
                record.configured,
                &record.last_imported_profile,
            ) {
                (RegistryMigrationStatus::PendingLegacyImport { .. }, true, None)
                | (RegistryMigrationStatus::Complete, true, Some(_))
                | (RegistryMigrationStatus::Complete, false, None) => {}
                _ => bail!("registry record has an impossible migration/import state"),
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryMigrationPlan {
    Ready {
        registry: DeviceRegistryV2,
    },
    Blocked {
        retained_legacy_bytes: Vec<u8>,
        diagnostics: Vec<String>,
    },
}

/// Convert registry-v1 bytes into one all-or-nothing v2 plan.
///
/// Invalid, noncanonical, or colliding legacy identities retain the exact
/// input bytes and produce no partial v2 registry.
pub fn plan_registry_v2_migration(legacy_bytes: &[u8]) -> RegistryMigrationPlan {
    let legacy: LegacyRegistryV1 = match serde_json::from_slice(legacy_bytes) {
        Ok(legacy) => legacy,
        Err(error) => return blocked(legacy_bytes, format!("parse legacy registry: {error}")),
    };
    if legacy.version != 1 {
        return blocked(
            legacy_bytes,
            format!("unsupported legacy registry version {}", legacy.version),
        );
    }

    let mut devices = BTreeMap::new();
    for record in legacy.records {
        let device_id = match DeviceId::parse(&record.serial) {
            Ok(device_id) => device_id,
            Err(error) => {
                return blocked(
                    legacy_bytes,
                    format!(
                        "legacy device serial {:?} is invalid: {error}",
                        record.serial
                    ),
                );
            }
        };
        let key = device_id.as_str().to_owned();
        let migration_status = if record.configured {
            RegistryMigrationStatus::PendingLegacyImport {
                selection_revision: record.selection_revision,
                settings_revision: record.settings_revision,
                subscriptions_revision: record.subscriptions_revision,
            }
        } else {
            RegistryMigrationStatus::Complete
        };
        let migrated = RegistryDeviceRecordV2 {
            configured: record.configured,
            presentation: RegistryPresentation {
                name: record.name,
                model_label: nonempty(record.model_label),
                hardware_facts: RegistryHardwareFacts::default(),
            },
            last_seen_unix_secs: record.last_seen_unix_secs,
            last_storage: None,
            last_readiness: None,
            last_imported_profile: None,
            migration_status,
        };
        if devices.insert(key.clone(), migrated).is_some() {
            return blocked(
                legacy_bytes,
                format!("legacy registry contains colliding device ID {key}"),
            );
        }
    }

    let registry = DeviceRegistryV2 {
        schema_version: REGISTRY_V2_SCHEMA_VERSION,
        devices,
    };
    if let Err(error) = registry.validate() {
        return blocked(
            legacy_bytes,
            format!("validate migrated registry: {error:#}"),
        );
    }
    RegistryMigrationPlan::Ready { registry }
}

fn deserialize_devices<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, RegistryDeviceRecordV2>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DevicesVisitor;

    impl<'de> Visitor<'de> for DevicesVisitor {
        type Value = BTreeMap<String, RegistryDeviceRecordV2>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a device map without duplicate keys")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut devices = BTreeMap::new();
            while let Some((key, record)) = map.next_entry::<String, RegistryDeviceRecordV2>()? {
                if devices.insert(key.clone(), record).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate registry device key {key:?}"
                    )));
                }
            }
            Ok(devices)
        }
    }

    deserializer.deserialize_map(DevicesVisitor)
}

fn blocked(legacy_bytes: &[u8], diagnostic: String) -> RegistryMigrationPlan {
    RegistryMigrationPlan::Blocked {
        retained_legacy_bytes: legacy_bytes.to_vec(),
        diagnostics: vec![diagnostic],
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRegistryV1 {
    version: u32,
    records: Vec<LegacyDeviceRecordV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyDeviceRecordV1 {
    serial: String,
    #[serde(default)]
    model_label: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    last_seen_unix_secs: Option<u64>,
    #[serde(default)]
    selection_revision: u64,
    #[serde(default)]
    settings_revision: u64,
    #[serde(default)]
    subscriptions_revision: u64,
}
