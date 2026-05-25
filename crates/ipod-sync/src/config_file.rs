//! TOML round-trip for the persistent config at %APPDATA%\ipod-sync\config.toml.
//!
//! Implemented in Task 1.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[default]
    Review,
    AutoApply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyLevel {
    #[default]
    All,
    ErrorsOnly,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub autostart_with_windows: bool,
    #[serde(default = "default_review_mode")]
    pub first_sync_mode: SyncMode,
    #[serde(default = "default_auto_apply_mode")]
    pub subsequent_sync_mode: SyncMode,
    #[serde(default = "default_schedule_minutes")]
    pub schedule_minutes: u32,
    #[serde(default)]
    pub notify_on: NotifyLevel,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            autostart_with_windows: false,
            first_sync_mode: SyncMode::Review,
            subsequent_sync_mode: SyncMode::AutoApply,
            schedule_minutes: 30,
            notify_on: NotifyLevel::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpodIdentity {
    pub serial: String,
    #[serde(default)]
    pub model_label: String,
    /// User-set "iPod name" from the device's iTunesDB master playlist
    /// (e.g. "Michael's iPod"). Updated each time the daemon detects a
    /// plug-in so the UI shows the current firmware name even if the
    /// user has renamed it. `None` if the iTunesDB couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn default_true() -> bool { true }
fn default_review_mode() -> SyncMode { SyncMode::Review }
fn default_auto_apply_mode() -> SyncMode { SyncMode::AutoApply }
fn default_schedule_minutes() -> u32 { 30 }
fn default_daemon_settings() -> Option<DaemonSettings> { Some(DaemonSettings::default()) }

/// Persistent config. Every field is optional so a partial / missing TOML
/// deserializes cleanly — the precedence logic in `config::resolve` decides
/// what each None means.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipod: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ffmpeg: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_delete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_tui: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<crate::cli::EncoderChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passthrough_wav: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refalac_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_reencode: Option<bool>,
    #[serde(default = "default_daemon_settings", skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipod_identity: Option<IpodIdentity>,
}

/// Default location of the persisted config: %APPDATA%\ipod-sync\config.toml.
pub fn default_path() -> Result<PathBuf> {
    let appdata = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve %APPDATA% via dirs::config_dir"))?;
    Ok(appdata.join(crate::PROJECT_DIR).join("config.toml"))
}

/// Load the persisted config from `path`. Returns `Ok(None)` if the file
/// doesn't exist (a missing config is not an error — it just means "no
/// overrides set yet"). Returns `Err` only on read or parse failure.
pub fn load(path: &Path) -> Result<Option<PersistedConfig>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let parsed: PersistedConfig = toml::from_str(&s)
                .with_context(|| format!("parse config at {}", path.display()))?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("read config at {}: {e}", path.display())),
    }
}

/// Save the persisted config atomically: write to <path>.tmp, fsync, rename.
pub fn save(path: &Path, cfg: &PersistedConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    {
        let s = toml::to_string_pretty(cfg)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp config {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, s.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/sample-config.toml");

    #[test]
    fn parses_fixture() {
        let cfg: PersistedConfig = toml::from_str(SAMPLE).unwrap();
        assert_eq!(cfg.source.as_deref().unwrap().to_string_lossy(),
                   "\\\\<host>\\data\\media\\music");
        assert_eq!(cfg.ipod.as_deref(), Some("G:"));
        assert_eq!(cfg.ffmpeg.as_deref().unwrap().to_string_lossy(), "ffmpeg");
        assert_eq!(cfg.no_delete, Some(false));
        assert_eq!(cfg.no_tui, Some(false));
    }

    #[test]
    fn empty_toml_deserializes_to_all_none() {
        let cfg: PersistedConfig = toml::from_str("").unwrap();
        assert!(cfg.source.is_none());
        assert!(cfg.ipod.is_none());
        assert!(cfg.ffmpeg.is_none());
        assert!(cfg.no_delete.is_none());
        assert!(cfg.no_tui.is_none());
        assert!(cfg.encoder.is_none());
        assert!(cfg.passthrough_wav.is_none());
        assert!(cfg.refalac_path.is_none());
        assert!(cfg.force_reencode.is_none());
    }

    #[test]
    fn partial_toml_deserializes_with_other_fields_none() {
        let cfg: PersistedConfig = toml::from_str(r#"source = "D:\\music""#).unwrap();
        assert_eq!(cfg.source.as_deref().unwrap().to_string_lossy(), "D:\\music");
        assert!(cfg.ipod.is_none());
        assert!(cfg.no_delete.is_none());
    }

    #[test]
    fn load_missing_returns_none() {
        let path = std::env::temp_dir()
            .join(format!("ipod-sync-test-missing-config-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let result = load(&path).unwrap();
        assert!(result.is_none(), "missing config file must return Ok(None)");
    }

    #[test]
    fn save_then_load_roundtrip() {
        let path = std::env::temp_dir()
            .join(format!("ipod-sync-test-rt-config-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cfg = PersistedConfig {
            source: Some(PathBuf::from(r"D:\music")),
            ipod: Some("F:".to_string()),
            ffmpeg: None,
            no_delete: Some(true),
            no_tui: Some(false),
            encoder: Some(crate::cli::EncoderChoice::Refalac),
            passthrough_wav: Some(true),
            refalac_path: Some(PathBuf::from(r"C:\bin\refalac64.exe")),
            force_reencode: Some(false),
            // Mirror the synthesized default that load() will produce when
            // [daemon] is absent from the TOML — keeps the roundtrip eq honest.
            daemon: Some(DaemonSettings::default()),
            ipod_identity: None,
        };
        save(&path, &cfg).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_skips_none_fields_in_toml_output() {
        let path = std::env::temp_dir()
            .join(format!("ipod-sync-test-skip-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cfg = PersistedConfig {
            source: Some(PathBuf::from(r"D:\music")),
            ipod: None,
            ffmpeg: None,
            no_delete: None,
            no_tui: None,
            encoder: None,
            passthrough_wav: None,
            refalac_path: None,
            force_reencode: None,
            daemon: None,
            ipod_identity: None,
        };
        save(&path, &cfg).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("source"));
        assert!(!saved.contains("ipod"), "None fields must be skipped in TOML output:\n{saved}");
        assert!(!saved.contains("no_delete"));
        assert!(!saved.contains("encoder"), "None fields must be skipped:\n{saved}");
        assert!(!saved.contains("passthrough_wav"));
        assert!(!saved.contains("refalac_path"));
        assert!(!saved.contains("force_reencode"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn config_without_daemon_section_loads_with_defaults() {
        let toml_text = r#"
source = '\\HOST\share\music'
encoder = "ffmpeg"
"#;
        let cfg: PersistedConfig = toml::from_str(toml_text).expect("parse");
        let daemon = cfg.daemon.expect("daemon section synthesized via default");
        assert!(daemon.enabled);
        assert!(!daemon.autostart_with_windows);
        assert_eq!(daemon.first_sync_mode, SyncMode::Review);
        assert_eq!(daemon.subsequent_sync_mode, SyncMode::AutoApply);
        assert_eq!(daemon.schedule_minutes, 30);
        assert_eq!(daemon.notify_on, NotifyLevel::All);
        assert!(cfg.ipod_identity.is_none());
    }

    #[test]
    fn config_with_daemon_and_ipod_identity_round_trips() {
        let cfg = PersistedConfig {
            daemon: Some(DaemonSettings {
                enabled: true,
                autostart_with_windows: true,
                first_sync_mode: SyncMode::AutoApply,
                subsequent_sync_mode: SyncMode::AutoApply,
                schedule_minutes: 60,
                notify_on: NotifyLevel::ErrorsOnly,
            }),
            ipod_identity: Some(IpodIdentity {
                serial: "EXAMPLE1234".to_string(),
                model_label: "iPod Classic 7G".to_string(),
                name: None,
            }),
            ..PersistedConfig::default()
        };

        let toml_text = toml::to_string(&cfg).expect("serialize");
        let parsed: PersistedConfig = toml::from_str(&toml_text).expect("round-trip");
        assert_eq!(cfg, parsed);
    }
}
