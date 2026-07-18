//! Durable registry for every device Classick has configured or observed.
//!
//! Registry keys are canonicalized only for lookup. The original serial is
//! kept in each record so persisted data and future IPC retain the exact
//! device-provided value.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config_file::IpodIdentity;
use crate::ipod::device::DetectedIpod;

const REGISTRY_VERSION: u32 = 1;

/// Comparison-only form of a serial. Never use this on disk or on the wire.
pub(crate) fn canonical_serial_key(serial: &str) -> String {
    let trimmed = serial.trim();
    trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed)
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeviceRecord {
    pub serial: String,
    #[serde(default)]
    pub model_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_unix_secs: Option<u64>,
    #[serde(default)]
    pub selection_revision: u64,
    #[serde(default)]
    pub settings_revision: u64,
    #[serde(default)]
    pub subscriptions_revision: u64,
}

impl DeviceRecord {
    fn from_legacy(identity: &IpodIdentity) -> Self {
        Self {
            serial: identity.serial.clone(),
            model_label: identity.model_label.clone(),
            name: identity.name.clone(),
            configured: true,
            last_seen_unix_secs: None,
            selection_revision: 0,
            settings_revision: 0,
            subscriptions_revision: 0,
        }
    }

    #[allow(dead_code)]
    fn from_detected(identity: &DetectedIpod, now: u64) -> Self {
        Self {
            serial: identity.serial.clone(),
            model_label: identity.model_label.clone(),
            name: identity.name.clone(),
            configured: false,
            last_seen_unix_secs: Some(now),
            selection_revision: 0,
            settings_revision: 0,
            subscriptions_revision: 0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistryFile {
    version: u32,
    records: Vec<DeviceRecord>,
}

/// Registry state plus the file it is atomically persisted to.
#[derive(Debug)]
pub(crate) struct DeviceRegistry {
    path: PathBuf,
    records: BTreeMap<String, DeviceRecord>,
}

impl DeviceRegistry {
    pub(crate) fn load_or_migrate(path: PathBuf, legacy: Option<&IpodIdentity>) -> Result<Self> {
        let mut should_persist = false;
        let mut records = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let file: RegistryFile = serde_json::from_str(&text)
                    .with_context(|| format!("parse device registry at {}", path.display()))?;
                if file.version != REGISTRY_VERSION {
                    return Err(anyhow!(
                        "unsupported device registry version {} at {}",
                        file.version,
                        path.display()
                    ));
                }
                Self::index_records(file.records)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                should_persist = true;
                BTreeMap::new()
            }
            Err(error) => {
                return Err(anyhow!(
                    "read device registry at {}: {error}",
                    path.display()
                ))
            }
        };
        if records.is_empty() {
            if let Some(identity) = legacy {
                let record = DeviceRecord::from_legacy(identity);
                let key = Self::required_key(&record.serial)?;
                records.insert(key, record);
                should_persist = true;
            }
        }

        let registry = Self { path, records };
        if should_persist {
            registry.persist(&registry.records)?;
        }
        Ok(registry)
    }

    pub(crate) fn records(&self) -> Vec<DeviceRecord> {
        self.records.values().cloned().collect()
    }

    pub(crate) fn record(&self, serial: &str) -> Option<&DeviceRecord> {
        self.records.get(&canonical_serial_key(serial))
    }

    #[allow(dead_code)]
    pub(crate) fn observe(&mut self, identity: &DetectedIpod, now: u64) -> Result<()> {
        let key = Self::required_key(&identity.serial)?;
        let mut next = self.records.clone();
        match next.get_mut(&key) {
            Some(record) => {
                record.model_label = identity.model_label.clone();
                record.name = identity.name.clone();
                record.last_seen_unix_secs = Some(now);
            }
            None => {
                next.insert(key, DeviceRecord::from_detected(identity, now));
            }
        }
        self.replace_records(next)
    }

    #[allow(dead_code)]
    pub(crate) fn configure(&mut self, serial: &str) -> Result<()> {
        self.configure_identity(&IpodIdentity {
            serial: serial.to_string(),
            model_label: String::new(),
            name: None,
            custom_selection: true,
        })?;
        Ok(())
    }

    pub(crate) fn configure_identity(&mut self, identity: &IpodIdentity) -> Result<bool> {
        let serial = &identity.serial;
        let key = Self::required_key(serial)?;
        let mut next = self.records.clone();
        let record = next
            .get_mut(&key)
            .ok_or_else(|| anyhow!("cannot configure unknown device serial {serial:?}"))?;
        let mut changed = false;
        if !record.configured {
            record.configured = true;
            record.settings_revision = record
                .settings_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("settings revision overflow for device {serial:?}"))?;
            changed = true;
        }
        if !identity.model_label.is_empty() && record.model_label != identity.model_label {
            record.model_label = identity.model_label.clone();
            changed = true;
        }
        if identity.name.is_some() && record.name != identity.name {
            record.name = identity.name.clone();
            changed = true;
        }
        if !changed {
            return Ok(false);
        }
        self.replace_records(next).map(|()| true)
    }

    #[allow(dead_code)]
    pub(crate) fn forget(&mut self, serial: &str) -> Result<()> {
        let key = Self::required_key(serial)?;
        let mut next = self.records.clone();
        let record = next
            .get_mut(&key)
            .ok_or_else(|| anyhow!("cannot forget unknown device serial {serial:?}"))?;
        if !record.configured {
            return Ok(());
        }
        record.configured = false;
        self.replace_records(next)
    }

    pub(crate) fn advance_config_revisions(
        &mut self,
        serial: &str,
        selection_changed: bool,
        settings_changed: bool,
        subscriptions_changed: bool,
    ) -> Result<bool> {
        let key = Self::required_key(serial)?;
        let current = self
            .records
            .get(&key)
            .ok_or_else(|| anyhow!("cannot update unknown device serial {serial:?}"))?;
        if !selection_changed && !settings_changed && !subscriptions_changed {
            return Ok(false);
        }

        let mut next = self.records.clone();
        let record = next
            .get_mut(&key)
            .expect("record exists after canonical lookup");
        if selection_changed {
            record.selection_revision = current
                .selection_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("selection revision overflow for device {serial:?}"))?;
        }
        if settings_changed {
            record.settings_revision = current
                .settings_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("settings revision overflow for device {serial:?}"))?;
        }
        if subscriptions_changed {
            record.subscriptions_revision = current
                .subscriptions_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("subscriptions revision overflow for device {serial:?}"))?;
        }
        self.replace_records(next)?;
        Ok(true)
    }

    fn required_key(serial: &str) -> Result<String> {
        let key = canonical_serial_key(serial);
        if key.is_empty() {
            return Err(anyhow!("device serial must not be empty"));
        }
        Ok(key)
    }

    fn index_records(records: Vec<DeviceRecord>) -> Result<BTreeMap<String, DeviceRecord>> {
        let mut indexed = BTreeMap::new();
        for record in records {
            let key = Self::required_key(&record.serial)?;
            if let Some(previous) = indexed.insert(key.clone(), record.clone()) {
                return Err(anyhow!(
                    "canonical serial collision for key {key:?}: {:?} and {:?}",
                    previous.serial,
                    record.serial,
                ));
            }
        }
        Ok(indexed)
    }

    #[allow(dead_code)]
    fn replace_records(&mut self, next: BTreeMap<String, DeviceRecord>) -> Result<()> {
        self.persist(&next)?;
        self.records = next;
        Ok(())
    }

    fn persist(&self, records: &BTreeMap<String, DeviceRecord>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create device registry directory {}", parent.display())
            })?;
        }
        let file = RegistryFile {
            version: REGISTRY_VERSION,
            records: records.values().cloned().collect(),
        };
        let text = serde_json::to_string_pretty(&file).context("serialize device registry")?;
        let tmp = self.path.with_extension("json.tmp");
        {
            let file = std::fs::File::create(&tmp)
                .with_context(|| format!("create temporary device registry {}", tmp.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            use std::io::Write;
            writer
                .write_all(text.as_bytes())
                .with_context(|| format!("write temporary device registry {}", tmp.display()))?;
            let file = writer
                .into_inner()
                .context("flush device registry writer")?;
            file.sync_all()
                .with_context(|| format!("fsync temporary device registry {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_serial_key, DeviceRegistry};
    use crate::config_file::IpodIdentity;
    use crate::ipod::device::DetectedIpod;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("device-registry-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("registry.json")
    }

    fn legacy(serial: &str) -> IpodIdentity {
        IpodIdentity {
            serial: serial.to_string(),
            model_label: "iPod Classic 7G".to_string(),
            name: Some("Library A".to_string()),
            custom_selection: false,
        }
    }

    fn detected(serial: &str) -> DetectedIpod {
        DetectedIpod {
            serial: serial.to_string(),
            model_label: "iPod Classic 6G".to_string(),
            drive: "/Volumes/IPOD".to_string(),
            name: Some("Library B".to_string()),
            volume_guid: None,
        }
    }

    #[test]
    fn migrates_legacy_configured_device_preserving_raw_serial() {
        let path = temp_path("migrate");
        let registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy(" A-001 "))).unwrap();

        let records = registry.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].serial, " A-001 ");
        assert!(records[0].configured);
        assert_eq!(records[0].model_label, "iPod Classic 7G");
        assert_eq!(records[0].name.as_deref(), Some("Library A"));
        assert_eq!(records[0].last_seen_unix_secs, None);
        assert!(path.exists(), "migration must be durable");
    }

    #[test]
    fn migrates_legacy_identity_after_empty_registry_was_persisted() {
        let path = temp_path("migrate-after-empty");
        let empty = DeviceRegistry::load_or_migrate(path.clone(), None).unwrap();
        assert!(empty.records().is_empty());

        let migrated =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy(" A-002 "))).unwrap();
        let records = migrated.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].serial, " A-002 ");
        assert!(records[0].configured);

        let reloaded = DeviceRegistry::load_or_migrate(path, None).unwrap();
        assert_eq!(reloaded.records(), records);
    }

    #[test]
    fn existing_non_empty_registry_remains_authoritative_over_legacy() {
        let path = temp_path("existing-authoritative");
        DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();

        let reloaded = DeviceRegistry::load_or_migrate(path, Some(&legacy("B"))).unwrap();
        let records = reloaded.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].serial, "A");
    }

    #[test]
    fn restart_does_not_remigrate_forgotten_device_from_legacy_config() {
        let path = temp_path("forgotten-authoritative");
        let legacy = legacy("RAW-A");
        let mut registry = DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy)).unwrap();
        registry.forget("raw-a").unwrap();

        let restarted = DeviceRegistry::load_or_migrate(path, Some(&legacy)).unwrap();

        let record = restarted.record("RAW-A").unwrap();
        assert!(!record.configured);
        assert_eq!(record.serial, "RAW-A");
    }

    #[test]
    fn observing_unconfigured_b_does_not_replace_configured_a() {
        let path = temp_path("observe-b");
        let mut registry = DeviceRegistry::load_or_migrate(path, Some(&legacy("A"))).unwrap();

        registry.observe(&detected("B"), 42).unwrap();

        let records = registry.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].serial, "A");
        assert!(records[0].configured);
        assert_eq!(records[1].serial, "B");
        assert!(!records[1].configured);
        assert_eq!(records[1].last_seen_unix_secs, Some(42));
        assert_eq!(records[1].name.as_deref(), Some("Library B"));
    }

    #[test]
    fn forgetting_b_preserves_a_and_marks_b_unconfigured_across_restart() {
        let path = temp_path("forget-b");
        let mut registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();
        registry.observe(&detected("B"), 42).unwrap();

        registry.forget(" b ").unwrap();

        let reloaded = DeviceRegistry::load_or_migrate(path, None).unwrap();
        let records = reloaded.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].serial, "A");
        assert!(records[0].configured);
        assert_eq!(records[1].serial, "B");
        assert!(!records[1].configured);
        assert_eq!(records[1].last_seen_unix_secs, Some(42));
    }

    #[test]
    fn rejects_records_with_colliding_canonical_serials() {
        let path = temp_path("collision");
        std::fs::write(
            &path,
            r#"{"version":1,"records":[
                {"serial":"A-001","model_label":"Classic","configured":true,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0},
                {"serial":" a-001 ","model_label":"Classic","configured":false,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0}
            ]}"#,
        )
        .unwrap();

        let error = DeviceRegistry::load_or_migrate(path, None).unwrap_err();
        assert!(error.to_string().contains("canonical serial collision"));
        assert_eq!(canonical_serial_key(" A-001 "), "a-001");
        assert_eq!(canonical_serial_key("0XABCD"), canonical_serial_key("abcd"));
    }

    #[test]
    fn configure_persists_a_single_settings_revision_bump() {
        let path = temp_path("configure");
        let mut registry = DeviceRegistry::load_or_migrate(path.clone(), None).unwrap();
        registry.observe(&detected("B"), 42).unwrap();

        registry.configure(" b ").unwrap();
        registry.configure("B").unwrap();

        let reloaded = DeviceRegistry::load_or_migrate(path, None).unwrap();
        let record = reloaded.records().pop().unwrap();
        assert!(record.configured);
        assert_eq!(record.settings_revision, 1);
        assert_eq!(record.selection_revision, 0);
        assert_eq!(record.subscriptions_revision, 0);
    }

    #[test]
    fn canonical_lookup_returns_the_record_with_raw_serial_unchanged() {
        let path = temp_path("lookup");
        let registry = DeviceRegistry::load_or_migrate(path, Some(&legacy(" 0xRAW-A "))).unwrap();

        let record = registry.record("raw-a").expect("canonical record");

        assert_eq!(record.serial, " 0xRAW-A ");
        assert!(registry.record("RAW-B").is_none());
    }

    #[test]
    fn config_revision_advance_is_component_selective_and_atomic() {
        let path = temp_path("revision-advance");
        let mut registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();

        registry
            .advance_config_revisions("a", true, false, true)
            .unwrap();

        let reloaded = DeviceRegistry::load_or_migrate(path, None).unwrap();
        let record = reloaded.record("A").unwrap();
        assert_eq!(record.selection_revision, 1);
        assert_eq!(record.settings_revision, 0);
        assert_eq!(record.subscriptions_revision, 1);
    }

    #[test]
    fn config_revision_advance_changes_only_the_target_device() {
        let path = temp_path("revision-target");
        let mut registry = DeviceRegistry::load_or_migrate(path, Some(&legacy("A"))).unwrap();
        registry.observe(&detected("B"), 42).unwrap();
        registry.configure("B").unwrap();

        registry
            .advance_config_revisions("B", true, true, true)
            .unwrap();

        let a = registry.record("A").unwrap();
        assert_eq!(a.selection_revision, 0);
        assert_eq!(a.settings_revision, 0);
        assert_eq!(a.subscriptions_revision, 0);
        let b = registry.record("B").unwrap();
        assert_eq!(b.selection_revision, 1);
        assert_eq!(b.settings_revision, 2, "configure plus settings save");
        assert_eq!(b.subscriptions_revision, 1);
    }

    #[test]
    fn config_revision_no_op_does_not_rewrite_the_registry() {
        let path = temp_path("revision-no-op");
        let mut registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        registry
            .advance_config_revisions("A", false, false, false)
            .unwrap();

        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), before);
        assert_eq!(registry.record("A").unwrap().settings_revision, 0);
    }

    #[test]
    fn failed_config_revision_persist_keeps_in_memory_record_unchanged() {
        let path = temp_path("revision-failure");
        let mut registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();
        let unusable_path = path.parent().unwrap().join("registry-directory");
        std::fs::create_dir_all(&unusable_path).unwrap();
        registry.path = unusable_path;

        let error = registry
            .advance_config_revisions("A", true, true, true)
            .unwrap_err();

        assert!(error.to_string().contains("rename"));
        let record = registry.record("A").unwrap();
        assert_eq!(record.selection_revision, 0);
        assert_eq!(record.settings_revision, 0);
        assert_eq!(record.subscriptions_revision, 0);
    }

    #[test]
    fn failed_forget_persist_keeps_configured_record_in_memory_and_on_restart() {
        let path = temp_path("forget-failure");
        let mut registry =
            DeviceRegistry::load_or_migrate(path.clone(), Some(&legacy("A"))).unwrap();
        let unusable_path = path.parent().unwrap().join("registry-directory");
        std::fs::create_dir_all(&unusable_path).unwrap();
        registry.path = unusable_path;

        registry.forget("a").unwrap_err();

        assert!(registry.record("A").unwrap().configured);
        let restarted = DeviceRegistry::load_or_migrate(path, None).unwrap();
        assert!(restarted.record("A").unwrap().configured);
    }
}
