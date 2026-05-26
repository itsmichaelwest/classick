//! Resolved runtime config. CLI + defaults + env vars applied; immutable after construction.

use crate::cli::{Cli, EncoderChoice};
use crate::config_file::{self, PersistedConfig};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Env var name for the source library root. Used when `--source` is not passed.
pub const SOURCE_ENV: &str = "CLASSICK_SOURCE";

#[derive(Debug, Clone)]
pub struct Config {
    pub source: PathBuf,
    pub ipod: Option<String>, // None = auto-detect at runtime
    pub ffmpeg: PathBuf,
    pub dry_run: bool,
    pub apply: bool,
    pub no_delete: bool,
    pub verbose: bool,
    pub rebuild_manifest: bool,
    pub use_tui: bool,
    pub manifest_path: PathBuf,
    pub save_config: bool,
    // Phase 3: encoder + classify-related fields.
    // FUTURE: per-format encoder selection — see Phase 3 addendum Change 2 and
    //         the comment on EncoderChoice in cli.rs. This stays a single global
    //         value until/unless that future arrives.
    pub encoder: EncoderChoice,
    pub refalac_path: PathBuf,
    pub passthrough_wav: bool,
    pub force_reencode: bool,
}

impl Config {
    /// Project the effective runtime config back into a PersistedConfig
    /// suitable for writing via `config_file::save`.
    pub fn to_persisted(&self) -> PersistedConfig {
        // `..Default::default()` covers fields Config doesn't track (`daemon`,
        // `ipod_identity`, future additions). Avoids the LEARNINGS-noted
        // brittle break when PersistedConfig grows a new field.
        PersistedConfig {
            source: Some(self.source.clone()),
            ipod: self.ipod.clone(),
            ffmpeg: Some(self.ffmpeg.clone()),
            no_delete: Some(self.no_delete),
            no_tui: Some(!self.use_tui),
            encoder: Some(self.encoder),
            passthrough_wav: Some(self.passthrough_wav),
            refalac_path: Some(self.refalac_path.clone()),
            force_reencode: Some(self.force_reencode),
            ..PersistedConfig::default()
        }
    }
}

pub fn resolve(cli: Cli) -> Result<Config> {
    let manifest_path = default_manifest_path()?;
    let persisted = config_file::load(&config_file::default_path()?)?;
    resolve_with(cli, std::env::var(SOURCE_ENV).ok(), persisted, manifest_path)
}

/// Inner resolve — separated from `resolve` so tests can inject env + persisted
/// state without mutating process state.
pub fn resolve_with(
    cli: Cli,
    env_source: Option<String>,
    persisted: Option<PersistedConfig>,
    manifest_path: PathBuf,
) -> Result<Config> {
    let ipod = cli
        .ipod
        .clone()
        .or_else(|| persisted.as_ref().and_then(|p| p.ipod.clone()))
        .map(normalize_drive);

    let source = merge_source(&cli, env_source, &persisted).ok_or_else(|| {
        anyhow!(
            "no source library specified.\n\
             Pass --source <path>, set the {SOURCE_ENV} environment variable,\n\
             or run with no args to launch the first-time setup wizard."
        )
    })?;

    let ffmpeg = cli
        .ffmpeg
        .or_else(|| persisted.as_ref().and_then(|p| p.ffmpeg.clone()))
        .unwrap_or_else(|| PathBuf::from("ffmpeg"));

    let no_delete =
        cli.no_delete || persisted.as_ref().and_then(|p| p.no_delete).unwrap_or(false);

    let no_tui = cli.no_tui || persisted.as_ref().and_then(|p| p.no_tui).unwrap_or(false);

    // Encoder: CLI > persisted > default (Ffmpeg). cli.encoder is Option so
    // the persisted layer can actually win when no flag was passed; default
    // applies only when neither layer set it.
    let encoder = cli
        .encoder
        .or_else(|| persisted.as_ref().and_then(|p| p.encoder))
        .unwrap_or_default();

    let refalac_path = cli
        .refalac_path
        .or_else(|| persisted.as_ref().and_then(|p| p.refalac_path.clone()))
        .unwrap_or_else(|| PathBuf::from("refalac64"));

    // bool flags: CLI flag (true means user set it) wins; otherwise persisted;
    // otherwise default false.
    let passthrough_wav = cli.passthrough_wav
        || persisted
            .as_ref()
            .and_then(|p| p.passthrough_wav)
            .unwrap_or(false);

    let force_reencode = cli.force_reencode
        || persisted
            .as_ref()
            .and_then(|p| p.force_reencode)
            .unwrap_or(false);

    Ok(Config {
        source,
        ipod,
        ffmpeg,
        dry_run: cli.dry_run,
        apply: cli.apply,
        no_delete,
        verbose: cli.verbose,
        rebuild_manifest: cli.rebuild_manifest,
        use_tui: !no_tui,
        manifest_path,
        save_config: cli.save_config,
        encoder,
        refalac_path,
        passthrough_wav,
        force_reencode,
    })
}

/// Merge source from CLI → env → persisted (CLI wins; persisted is fallback).
/// Returns None if no layer sets it — the caller decides whether to launch the
/// wizard or fail.
pub fn merge_source(
    cli: &Cli,
    env_source: Option<String>,
    persisted: &Option<PersistedConfig>,
) -> Option<PathBuf> {
    cli.source
        .clone()
        .or_else(|| env_source.map(PathBuf::from))
        .or_else(|| persisted.as_ref().and_then(|p| p.source.clone()))
}

fn default_manifest_path() -> Result<PathBuf> {
    let appdata = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve %APPDATA% via dirs::config_dir"))?;
    Ok(appdata.join(crate::PROJECT_DIR).join("manifest.json"))
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
    use crate::config_file::PersistedConfig;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn errors_when_no_source_specified() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let err = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no source library specified"), "got: {msg}");
        assert!(msg.contains(SOURCE_ENV), "error should name the env var: {msg}");
    }

    #[test]
    fn uses_env_var_when_no_flag() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let config = resolve_with(
            cli,
            Some(r"E:\env-music".to_string()),
            None,
            PathBuf::from("dummy.json"),
        )
        .unwrap();
        assert_eq!(config.source, PathBuf::from(r"E:\env-music"));
    }

    #[test]
    fn flag_overrides_env_var() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\music"]).unwrap();
        let config = resolve_with(
            cli,
            Some(r"E:\env-music".to_string()),
            None,
            PathBuf::from("dummy.json"),
        )
        .unwrap();
        assert_eq!(config.source, PathBuf::from(r"D:\music"));
    }

    #[test]
    fn other_defaults_apply_when_source_is_present() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\music"]).unwrap();
        let manifest = PathBuf::from(r"C:\fake\classick\manifest.json");
        let config = resolve_with(cli, None, None, manifest.clone()).unwrap();
        assert_eq!(config.ipod, None);
        assert_eq!(config.ffmpeg, PathBuf::from("ffmpeg"));
        assert!(!config.dry_run);
        assert!(!config.apply);
        assert!(!config.no_delete);
        assert!(!config.verbose);
        assert!(!config.rebuild_manifest);
        assert!(config.use_tui, "TUI defaults on");
        assert!(!config.save_config);
        assert_eq!(config.manifest_path, manifest);
        // Phase 3 defaults.
        assert_eq!(config.encoder, EncoderChoice::Ffmpeg);
        assert_eq!(config.refalac_path, PathBuf::from("refalac64"));
        assert!(!config.passthrough_wav);
        assert!(!config.force_reencode);
    }

    #[test]
    fn cli_encoder_wins_over_persisted_encoder() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\m",
            "--encoder",
            "refalac",
        ])
        .unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            encoder: Some(EncoderChoice::Ffmpeg),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert_eq!(cfg.encoder, EncoderChoice::Refalac);
    }

    #[test]
    fn persisted_encoder_used_when_no_cli_flag() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            encoder: Some(EncoderChoice::Refalac),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert_eq!(cfg.encoder, EncoderChoice::Refalac);
    }

    #[test]
    fn encoder_falls_back_to_default_when_neither_set() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m"]).unwrap();
        let cfg = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert_eq!(cfg.encoder, EncoderChoice::Ffmpeg);
    }

    #[test]
    fn cli_refalac_path_wins_over_persisted() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\m",
            "--refalac-path",
            r"C:\bin\refalac64.exe",
        ])
        .unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            refalac_path: Some(PathBuf::from(r"X:\persisted\refalac64.exe")),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert_eq!(cfg.refalac_path, PathBuf::from(r"C:\bin\refalac64.exe"));
    }

    #[test]
    fn persisted_refalac_path_used_when_no_cli_flag() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            refalac_path: Some(PathBuf::from(r"X:\persisted\refalac64.exe")),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert_eq!(
            cfg.refalac_path,
            PathBuf::from(r"X:\persisted\refalac64.exe")
        );
    }

    #[test]
    fn cli_passthrough_wav_wins_over_persisted_false() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\m",
            "--passthrough-wav",
        ])
        .unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            passthrough_wav: Some(false),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert!(cfg.passthrough_wav);
    }

    #[test]
    fn persisted_passthrough_wav_used_when_no_cli_flag() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            passthrough_wav: Some(true),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert!(cfg.passthrough_wav);
    }

    #[test]
    fn cli_force_reencode_wins_over_persisted_false() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\m",
            "--force-reencode",
        ])
        .unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            force_reencode: Some(false),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json"))
            .unwrap();
        assert!(cfg.force_reencode);
    }

    #[test]
    fn force_reencode_defaults_false_with_no_layers() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m"]).unwrap();
        let cfg = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert!(!cfg.force_reencode);
    }

    #[test]
    fn flags_override_defaults() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\music",
            "--ipod",
            "F:",
            "--no-tui",
        ])
        .unwrap();
        let config = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert_eq!(config.source, PathBuf::from(r"D:\music"));
        assert_eq!(config.ipod, Some("F:".to_string()));
        assert!(!config.use_tui);
    }

    #[test]
    fn ipod_normalizes_drive_letter() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source",
            r"D:\music",
            "--ipod",
            "G",
        ])
        .unwrap();
        let config = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert_eq!(
            config.ipod,
            Some("G:".to_string()),
            "single letter gets colon appended"
        );
    }

    #[test]
    fn merge_uses_cli_when_set() {
        let cli =
            Cli::try_parse_from(["classick", "--source", r"D:\music"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"E:\persisted")),
            ..Default::default()
        };
        let merged =
            merge_source(&cli, std::env::var(SOURCE_ENV).ok(), &Some(persisted));
        assert_eq!(
            merged.unwrap(),
            PathBuf::from(r"D:\music"),
            "CLI must win over env and persisted"
        );
    }

    #[test]
    fn merge_uses_env_when_no_cli() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"E:\persisted")),
            ..Default::default()
        };
        let merged = merge_source(
            &cli,
            Some(r"F:\env-music".to_string()),
            &Some(persisted),
        );
        assert_eq!(
            merged.unwrap(),
            PathBuf::from(r"F:\env-music"),
            "env must win over persisted when no CLI flag"
        );
    }

    #[test]
    fn merge_uses_persisted_when_no_cli_or_env() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"E:\persisted")),
            ..Default::default()
        };
        let merged = merge_source(&cli, None, &Some(persisted));
        assert_eq!(merged.unwrap(), PathBuf::from(r"E:\persisted"));
    }

    #[test]
    fn merge_returns_none_when_nothing_set() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let merged = merge_source(&cli, None, &None);
        assert!(
            merged.is_none(),
            "no source from any layer must return None so caller can launch wizard"
        );
    }

    #[test]
    fn merge_returns_none_when_persisted_has_no_source() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        let persisted = PersistedConfig {
            source: None,
            ..Default::default()
        };
        let merged = merge_source(&cli, None, &Some(persisted));
        assert!(merged.is_none());
    }
}
