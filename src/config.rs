//! Resolved runtime config. CLI + defaults + env vars applied; immutable after construction.

use crate::cli::Cli;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Env var name for the source library root. Used when `--source` is not passed.
pub const SOURCE_ENV: &str = "IPOD_SYNC_SOURCE";

#[derive(Debug, Clone)]
pub struct Config {
    pub source: PathBuf,
    pub ipod: Option<String>, // None = auto-detect at runtime
    pub ffmpeg: PathBuf,
    pub dry_run: bool,
    pub no_delete: bool,
    pub verbose: bool,
    pub rebuild_manifest: bool,
    pub use_tui: bool,
    pub manifest_path: PathBuf,
}

pub fn resolve(cli: Cli) -> Result<Config> {
    let manifest_path = default_manifest_path()?;
    let ipod = cli.ipod.map(normalize_drive);
    let source = cli.source
        .or_else(|| std::env::var(SOURCE_ENV).ok().map(PathBuf::from))
        .ok_or_else(|| anyhow!(
            "no source library specified.\n\
             Pass --source <path> or set the {SOURCE_ENV} environment variable.\n\
             Example: --source \"\\\\server\\music\" (UNC) or --source D:\\music (local)."
        ))?;

    Ok(Config {
        source,
        ipod,
        ffmpeg: cli.ffmpeg.unwrap_or_else(|| PathBuf::from("ffmpeg")),
        dry_run: cli.dry_run,
        no_delete: cli.no_delete,
        verbose: cli.verbose,
        rebuild_manifest: cli.rebuild_manifest,
        use_tui: !cli.no_tui,
        manifest_path,
    })
}

fn default_manifest_path() -> Result<PathBuf> {
    let appdata = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve %APPDATA% via dirs::config_dir"))?;
    Ok(appdata.join("ipod-sync").join("manifest.json"))
}

/// "G" -> "G:". "G:" -> "G:". "G:\\" -> "G:\\". The Windows convention for
/// `--ipod` is a drive letter + colon (with optional trailing backslash).
fn normalize_drive(s: String) -> String {
    if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() {
        format!("{s}:")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    /// Tests must serialize on the env var since they share the process. A static
    /// mutex avoids cross-test races when several tests mutate IPOD_SYNC_SOURCE.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn errors_when_no_source_specified() {
        let _g = env_lock();
        // SAFETY: tests are serialized by env_lock; no other thread reads IPOD_SYNC_SOURCE.
        unsafe { std::env::remove_var(SOURCE_ENV); }
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        let err = resolve(cli).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no source library specified"), "got: {msg}");
        assert!(msg.contains(SOURCE_ENV), "error should name the env var: {msg}");
    }

    #[test]
    fn uses_env_var_when_no_flag() {
        let _g = env_lock();
        unsafe { std::env::set_var(SOURCE_ENV, r"E:\env-music"); }
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"E:\env-music"));
        unsafe { std::env::remove_var(SOURCE_ENV); }
    }

    #[test]
    fn flag_overrides_env_var() {
        let _g = env_lock();
        unsafe { std::env::set_var(SOURCE_ENV, r"E:\env-music"); }
        let cli = Cli::try_parse_from(["ipod-sync", "--source", r"D:\music"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"D:\music"));
        unsafe { std::env::remove_var(SOURCE_ENV); }
    }

    #[test]
    fn other_defaults_apply_when_source_is_present() {
        let _g = env_lock();
        unsafe { std::env::remove_var(SOURCE_ENV); }
        let cli = Cli::try_parse_from(["ipod-sync", "--source", r"D:\music"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.ipod, None);
        assert_eq!(config.ffmpeg, std::path::PathBuf::from("ffmpeg"));
        assert!(!config.dry_run);
        assert!(!config.no_delete);
        assert!(!config.verbose);
        assert!(!config.rebuild_manifest);
        assert!(config.use_tui, "TUI defaults on");
        assert!(config.manifest_path.to_string_lossy().contains("ipod-sync"));
        assert!(config.manifest_path.to_string_lossy().ends_with("manifest.json"));
    }

    #[test]
    fn flags_override_defaults() {
        let _g = env_lock();
        unsafe { std::env::remove_var(SOURCE_ENV); }
        let cli = Cli::try_parse_from([
            "ipod-sync",
            "--source",
            r"D:\music",
            "--ipod",
            "F:",
            "--no-tui",
        ])
        .unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"D:\music"));
        assert_eq!(config.ipod, Some("F:".to_string()));
        assert!(!config.use_tui);
    }

    #[test]
    fn ipod_normalizes_drive_letter() {
        let _g = env_lock();
        unsafe { std::env::remove_var(SOURCE_ENV); }
        let cli = Cli::try_parse_from(["ipod-sync", "--source", r"D:\music", "--ipod", "G"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.ipod, Some("G:".to_string()), "single letter gets colon appended");
    }
}
