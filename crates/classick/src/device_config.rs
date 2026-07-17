//! Per-device subscriptions (which playlists sync to a given iPod) and
//! settings (auto-sync, Rockbox compatibility) — JSON files that live under
//! `devices/<serial>/` alongside `manifest.json` and `selection.json` (see
//! `device_state.rs` for path resolution).
//!
//! Missing or corrupt files degrade to sensible defaults with a logged
//! warning (fail-open) — these are config, not user-curated content, so the
//! same convention as `selection.rs` applies: never hard-fail a sync over a
//! bad config file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SUBSCRIPTIONS_VERSION: u32 = 1;
pub const DEVICE_SETTINGS_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

/// Atomic write: tmp + fsync + rename, same as `selection::save_atomic`.
fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let json = serde_json::to_string_pretty(value)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp file {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, json.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Never errors: missing or unparseable JSON degrades to `T::default()` with
/// a logged warning.
fn load_json_or_default<T: for<'de> Deserialize<'de> + Default>(kind: &str, path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("{kind}: parse failed at {} ({e}); using default", path.display());
                T::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(e) => {
            tracing::warn!("{kind}: read failed at {} ({e}); using default", path.display());
            T::default()
        }
    }
}

/// Which playlists (by slug) sync to a given device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscriptions {
    pub version: u32,
    #[serde(default)]
    pub playlists: Vec<String>,
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self { version: SUBSCRIPTIONS_VERSION, playlists: Vec::new() }
    }
}

impl Subscriptions {
    /// Never errors: missing or unparseable subscriptions degrade to "no
    /// playlists subscribed" with a logged warning.
    pub fn load_or_default(path: &Path) -> Self {
        load_json_or_default("subscriptions", path)
    }

    /// Atomic write: tmp + fsync + rename.
    pub fn save_atomic(path: &Path, subs: &Subscriptions) -> Result<()> {
        save_json_atomic(path, subs)
    }
}

/// Per-device settings: auto-sync and Rockbox compatibility, seeded once
/// from the global `DaemonSettings` the first time a device is seen (see
/// [`DeviceSettings::load_or_migrate`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub version: u32,
    #[serde(default = "default_true")]
    pub auto_sync: bool,
    #[serde(default)]
    pub rockbox_compat: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self { version: DEVICE_SETTINGS_VERSION, auto_sync: true, rockbox_compat: false }
    }
}

impl DeviceSettings {
    /// Never errors: missing or unparseable settings degrade to
    /// [`DeviceSettings::default`] with a logged warning.
    pub fn load_or_default(path: &Path) -> Self {
        load_json_or_default("device_settings", path)
    }

    /// Atomic write: tmp + fsync + rename.
    pub fn save_atomic(path: &Path, settings: &DeviceSettings) -> Result<()> {
        save_json_atomic(path, settings)
    }

    /// Path-injected core of [`Self::load_or_migrate`] (testable without
    /// touching the real config dir). Seed-once by construction: if `path`
    /// already exists, its persisted values win — even if `global` has
    /// since changed — because this is a one-shot migration, not an
    /// ongoing mirror of the global settings. If `path` is absent, seed
    /// from `global.daemon` (`enabled` -> `auto_sync`, `rockbox_compat` ->
    /// `rockbox_compat`; a missing `global.daemon` falls back to
    /// `DaemonSettings::default()`), persist it, and return the seeded
    /// value. If persisting the seeded file fails, the seeded values are
    /// still returned and seeding is retried on the next call — under a
    /// persistently unwritable device dir, later calls re-seed from the
    /// then-current global (fail-open).
    fn load_or_migrate_at(path: &Path, global: &crate::config_file::PersistedConfig) -> Self {
        if path.exists() {
            return Self::load_or_default(path);
        }
        let daemon = global.daemon.clone().unwrap_or_default();
        let seeded = Self {
            version: DEVICE_SETTINGS_VERSION,
            auto_sync: daemon.enabled,
            rockbox_compat: daemon.rockbox_compat,
        };
        if let Err(e) = Self::save_atomic(path, &seeded) {
            tracing::warn!(
                "device_settings: failed to save seeded settings at {} ({e:#})",
                path.display()
            );
        }
        seeded
    }

    /// Seed-once migration from the global `DaemonSettings` into this
    /// device's per-device settings file. See [`Self::load_or_migrate_at`]
    /// for the exact seed-once contract; on write failure, seeded values are
    /// still returned and seeding is retried on the next call (fail-open).
    pub fn load_or_migrate(serial: &str, global: &crate::config_file::PersistedConfig) -> Self {
        let path = match crate::device_state::device_settings_path(serial) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "device_settings: cannot resolve settings path for {serial} ({e:#}); using defaults"
                );
                return Self::default();
            }
        };
        Self::load_or_migrate_at(&path, global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir_under_target(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("device_config-{label}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn persisted_config_with_daemon(enabled: bool, rockbox_compat: bool) -> crate::config_file::PersistedConfig {
        crate::config_file::PersistedConfig {
            daemon: Some(crate::config_file::DaemonSettings {
                enabled,
                rockbox_compat,
                ..crate::config_file::DaemonSettings::default()
            }),
            ..crate::config_file::PersistedConfig::default()
        }
    }

    // --- Subscriptions -------------------------------------------------

    #[test]
    fn subscriptions_load_or_default_missing_file_returns_default() {
        let base = tempdir_under_target("subs-missing");
        let path = base.join("subscriptions.json");
        assert_eq!(Subscriptions::load_or_default(&path), Subscriptions::default());
    }

    #[test]
    fn subscriptions_load_or_default_corrupt_file_degrades_to_default() {
        let base = tempdir_under_target("subs-corrupt");
        let path = base.join("subscriptions.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Subscriptions::load_or_default(&path), Subscriptions::default());
    }

    #[test]
    fn subscriptions_round_trip() {
        let base = tempdir_under_target("subs-roundtrip");
        let path = base.join("subscriptions.json");
        let subs = Subscriptions {
            version: SUBSCRIPTIONS_VERSION,
            playlists: vec!["chill-vibes".into(), "workout-mix".into()],
        };
        Subscriptions::save_atomic(&path, &subs).unwrap();
        assert_eq!(Subscriptions::load_or_default(&path), subs);
    }

    // --- DeviceSettings: load/save --------------------------------------

    #[test]
    fn device_settings_load_or_default_missing_file_returns_default() {
        let base = tempdir_under_target("settings-missing");
        let path = base.join("settings.json");
        assert_eq!(DeviceSettings::load_or_default(&path), DeviceSettings::default());
    }

    #[test]
    fn device_settings_load_or_default_corrupt_file_degrades_to_default() {
        let base = tempdir_under_target("settings-corrupt");
        let path = base.join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(DeviceSettings::load_or_default(&path), DeviceSettings::default());
    }

    #[test]
    fn device_settings_round_trip() {
        let base = tempdir_under_target("settings-roundtrip");
        let path = base.join("settings.json");
        let settings = DeviceSettings {
            version: DEVICE_SETTINGS_VERSION,
            auto_sync: false,
            rockbox_compat: true,
        };
        DeviceSettings::save_atomic(&path, &settings).unwrap();
        assert_eq!(DeviceSettings::load_or_default(&path), settings);
    }

    // --- DeviceSettings: seed-once migration ----------------------------

    #[test]
    fn load_or_migrate_seeds_from_global_when_absent_and_persists() {
        let base = tempdir_under_target("migrate-seed");
        let path = base.join("settings.json");
        let global = persisted_config_with_daemon(false, true);

        let seeded = DeviceSettings::load_or_migrate_at(&path, &global);

        assert_eq!(
            seeded,
            DeviceSettings { version: DEVICE_SETTINGS_VERSION, auto_sync: false, rockbox_compat: true }
        );
        assert!(path.exists(), "seed must persist the file");
        assert_eq!(DeviceSettings::load_or_default(&path), seeded);
    }

    #[test]
    fn load_or_migrate_defaults_when_global_daemon_is_none() {
        let base = tempdir_under_target("migrate-nodaemon");
        let path = base.join("settings.json");
        let global = crate::config_file::PersistedConfig::default();

        let seeded = DeviceSettings::load_or_migrate_at(&path, &global);

        assert_eq!(
            seeded,
            DeviceSettings {
                version: DEVICE_SETTINGS_VERSION,
                auto_sync: crate::config_file::DaemonSettings::default().enabled,
                rockbox_compat: crate::config_file::DaemonSettings::default().rockbox_compat,
            }
        );
    }

    #[test]
    fn load_or_migrate_is_seed_once_and_ignores_later_global_changes() {
        // The brief's pinning test: a second call under a DIFFERENT global
        // must return the values persisted by the first call, not re-seed.
        let base = tempdir_under_target("migrate-once");
        let path = base.join("settings.json");

        let global1 = persisted_config_with_daemon(true, false);
        let first = DeviceSettings::load_or_migrate_at(&path, &global1);
        assert_eq!(
            first,
            DeviceSettings { version: DEVICE_SETTINGS_VERSION, auto_sync: true, rockbox_compat: false }
        );

        let global2 = persisted_config_with_daemon(false, true);
        let second = DeviceSettings::load_or_migrate_at(&path, &global2);

        assert_eq!(
            second, first,
            "second call must return the persisted first values, not re-seed from a changed global"
        );
    }

    #[test]
    fn load_or_migrate_public_fn_resolves_via_device_settings_path() {
        let serial = "DEVCFG-LOMIG-PUBLIC-TEST";
        let global = persisted_config_with_daemon(false, true);

        let seeded = DeviceSettings::load_or_migrate(serial, &global);

        assert_eq!(seeded.auto_sync, false);
        assert_eq!(seeded.rockbox_compat, true);
        let path = crate::device_state::device_settings_path(serial).unwrap();
        assert!(path.exists(), "public fn must persist through device_settings_path");
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
