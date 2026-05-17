//! Resolved runtime config. CLI + defaults applied; immutable after construction.

use crate::cli::Cli;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Default source library root. Confirmed in Phase 2 brainstorming
/// (SPEC §4.1's `\\server\music\` was stale).
pub const DEFAULT_SOURCE: &str = r"<source-library-path>\";

#[derive(Debug, Clone)]
pub struct Config {
    pub source: PathBuf,
    pub ipod: Option<String>,  // None = auto-detect at runtime
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

    Ok(Config {
        source: cli.source.unwrap_or_else(|| PathBuf::from(DEFAULT_SOURCE)),
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

    #[test]
    fn defaults_when_no_flags_set() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"<source-library-path>\"));
        assert_eq!(config.ipod, None);  // auto-detect later
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
        let cli = Cli::try_parse_from([
            "ipod-sync",
            "--source", r"D:\music",
            "--ipod", "F:",
            "--no-tui",
        ]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"D:\music"));
        assert_eq!(config.ipod, Some("F:".to_string()));
        assert!(!config.use_tui);
    }

    #[test]
    fn ipod_normalizes_drive_letter() {
        let cli = Cli::try_parse_from(["ipod-sync", "--ipod", "G"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.ipod, Some("G:".to_string()), "single letter gets colon appended");
    }
}
