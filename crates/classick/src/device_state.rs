//! Per-device state directories under `<config>/classick/devices/<serial>/`.
//!
//! This module owns path resolution (and one-time migration off the legacy
//! flat `manifest.json`) for the "trust package" per-device layout. It is
//! pure path/filesystem logic — no FFI, no daemon awareness.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Uppercase, strip a leading `0x`, keep only `[A-Za-z0-9_-]`, map anything
/// else to `_`. Empty input (or input that is empty after stripping `0x`)
/// falls back to `"UNKNOWN"` so callers always get a non-empty, filesystem-
/// safe directory name.
pub fn sanitize_serial(serial: &str) -> String {
    let stripped = serial
        .strip_prefix("0x")
        .or_else(|| serial.strip_prefix("0X"))
        .unwrap_or(serial);

    if stripped.is_empty() {
        return "UNKNOWN".to_string();
    }

    let sanitized: String = stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();

    sanitized
}

/// Root of a device's state directory, created on demand. Uses the default
/// config location (`dirs::config_dir()/classick`).
pub fn device_dir(serial: &str) -> Result<PathBuf> {
    let config_root = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve config dir via dirs::config_dir"))?
        .join(crate::PROJECT_DIR);
    device_dir_in(&config_root, serial)
}

/// Test/override variant of [`device_dir`]: `root/devices/<sanitized>/`,
/// created on demand.
pub fn device_dir_in(root: &Path, serial: &str) -> Result<PathBuf> {
    let dir = root.join("devices").join(sanitize_serial(serial));
    fs::create_dir_all(&dir).with_context(|| format!("create device dir {}", dir.display()))?;
    Ok(dir)
}

/// Path to a device's manifest.json, creating the device dir on demand.
pub fn device_manifest_path(serial: &str) -> Result<PathBuf> {
    Ok(device_dir(serial)?.join("manifest.json"))
}

/// Test/override variant of [`device_manifest_path`].
pub fn device_manifest_path_in(root: &Path, serial: &str) -> Result<PathBuf> {
    Ok(device_dir_in(root, serial)?.join("manifest.json"))
}

/// Path to a device's selection.json, creating the device dir on demand.
pub fn device_selection_path(serial: &str) -> Result<PathBuf> {
    Ok(device_dir(serial)?.join("selection.json"))
}

/// Test/override variant of [`device_selection_path`].
pub fn device_selection_path_in(root: &Path, serial: &str) -> Result<PathBuf> {
    Ok(device_dir_in(root, serial)?.join("selection.json"))
}

/// Path to a device's artwork-dirty marker file, creating the device dir on
/// demand. Used by the artwork-refresh flow (Task 13) to flag that cover art
/// needs re-provisioning.
pub fn artwork_dirty_marker_path(serial: &str) -> Result<PathBuf> {
    Ok(device_dir(serial)?.join("artwork-dirty"))
}

/// Test/override variant of [`artwork_dirty_marker_path`].
pub fn artwork_dirty_marker_path_in(root: &Path, serial: &str) -> Result<PathBuf> {
    Ok(device_dir_in(root, serial)?.join("artwork-dirty"))
}

/// One-time migration of the legacy flat `manifest.json` into the per-device
/// layout. If `legacy_path` exists and the per-device manifest does not,
/// moves it there (rename, falling back to copy+delete across filesystems).
/// If the per-device manifest already exists, leaves the legacy file in
/// place untouched and logs a warning — the per-device manifest wins.
/// Returns the per-device manifest path either way.
pub fn migrate_legacy_manifest(legacy_path: &Path, serial: &str) -> Result<PathBuf> {
    let config_root = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve config dir via dirs::config_dir"))?
        .join(crate::PROJECT_DIR);
    migrate_legacy_manifest_in(&config_root, legacy_path, serial)
}

/// Test/override variant of [`migrate_legacy_manifest`].
pub fn migrate_legacy_manifest_in(root: &Path, legacy_path: &Path, serial: &str) -> Result<PathBuf> {
    let dst = device_manifest_path_in(root, serial)?;

    if !legacy_path.exists() {
        return Ok(dst);
    }

    if dst.exists() {
        tracing::warn!(
            legacy = %legacy_path.display(),
            device_manifest = %dst.display(),
            "per-device manifest already exists; leaving legacy manifest in place",
        );
        return Ok(dst);
    }

    match fs::rename(legacy_path, &dst) {
        Ok(()) => {}
        Err(_) => {
            // Cross-filesystem rename fails on some platforms; fall back to
            // copy + delete.
            fs::copy(legacy_path, &dst).with_context(|| {
                format!("copy legacy manifest {} -> {}", legacy_path.display(), dst.display())
            })?;
            fs::remove_file(legacy_path).with_context(|| {
                format!("remove legacy manifest {}", legacy_path.display())
            })?;
        }
    }

    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temp dir under `target/` so leftover dirs don't
    /// pollute the system temp and so they're easy to clean. Per-test unique
    /// via an AtomicU32 counter (PID alone collides under parallel test
    /// execution — see LEARNINGS.md).
    fn tempdir_under_target() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("device_state-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn sanitize_serial_uppercases_and_strips_0x() {
        assert_eq!(sanitize_serial("0x000A27002138B0A8"), "000A27002138B0A8");
        assert_eq!(sanitize_serial("abc-123"), "ABC-123");
        assert_eq!(sanitize_serial("weird/serial:name"), "WEIRD_SERIAL_NAME");
        assert_eq!(sanitize_serial(""), "UNKNOWN");
    }

    #[test]
    fn device_paths_nest_under_devices_dir() {
        let root = tempdir_under_target();
        let p = device_manifest_path_in(&root, "0xABC").unwrap();
        assert_eq!(p, root.join("devices").join("ABC").join("manifest.json"));
        assert!(p.parent().unwrap().is_dir(), "device_dir is created on demand");
    }

    #[test]
    fn migrate_moves_legacy_manifest_once() {
        let root = tempdir_under_target();
        let legacy = root.join("manifest.json");
        std::fs::write(&legacy, r#"{"version":1,"tracks":[]}"#).unwrap();
        let dst = migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
        assert_eq!(dst, root.join("devices").join("SER1").join("manifest.json"));
        assert!(!legacy.exists(), "legacy file moved, not copied");
        assert!(dst.exists());
        // Second call: legacy gone, per-device present — no-op, same path back.
        let dst2 = migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
        assert_eq!(dst, dst2);
    }

    #[test]
    fn migrate_never_clobbers_existing_device_manifest() {
        let root = tempdir_under_target();
        let legacy = root.join("manifest.json");
        std::fs::write(&legacy, r#"{"version":1,"tracks":[]}"#).unwrap();
        let dst = device_manifest_path_in(&root, "SER1").unwrap();
        std::fs::write(&dst, r#"{"version":1,"ipod_serial":"SER1","tracks":[]}"#).unwrap();
        migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
        let kept = std::fs::read_to_string(&dst).unwrap();
        assert!(kept.contains("SER1"), "existing per-device manifest wins; legacy left in place");
        assert!(legacy.exists());
    }
}
