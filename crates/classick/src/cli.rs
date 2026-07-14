//! clap CLI definitions. Parsing only; defaults + resolution live in `config`.

use clap::Parser;
use std::path::PathBuf;

/// Encoder choice for the transcode pipeline. Passthrough sources never see
/// this (no encoding happens). See docs/superpowers/specs/2026-05-23-phase-3-addendum.md
/// Change 1 for why ffmpeg is the default (was: auto in the original spec).
//
// FUTURE: per-format encoder selection. If a future user wants per-source-codec
// encoder choice (e.g. flac -> refalac, opus -> ffmpeg), this enum stays as-is;
// add a `pub struct EncoderConfig { default: EncoderChoice, per_format: HashMap<String, EncoderChoice> }`
// and have apply_loop resolve `cfg.for_source(&probe.codec_name)` instead of
// passing the global `cfg.encoder`. Everything below this layer is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum EncoderChoice {
    Ffmpeg,
    Refalac,
}

impl EncoderChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncoderChoice::Ffmpeg => "ffmpeg",
            EncoderChoice::Refalac => "refalac",
        }
    }
}

impl Default for EncoderChoice {
    fn default() -> Self {
        Self::Ffmpeg
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "classick",
    version,
    about = "Sync a FLAC library to an iPod Classic via libgpod with on-the-fly ALAC transcoding."
)]
pub struct Cli {
    /// Source library root. If omitted, falls back to the CLASSICK_SOURCE
    /// environment variable. Errors out if neither is set.
    #[arg(long)]
    pub source: Option<PathBuf>,

    /// iPod drive (e.g. G:). Auto-detected if omitted.
    #[arg(long)]
    pub ipod: Option<String>,

    /// Path to ffmpeg.exe. Defaults to "ffmpeg" on PATH.
    #[arg(long)]
    pub ffmpeg: Option<PathBuf>,

    /// Print the action plan; write nothing to manifest, iPod, or temp.
    #[arg(long)]
    pub dry_run: bool,

    /// Skip the interactive review and apply the action plan immediately.
    /// Useful for scripts/CI. Mutually exclusive with --dry-run.
    #[arg(long)]
    pub apply: bool,

    /// After the run completes, persist the effective settings (including any
    /// one-shot CLI flag overrides from this invocation) to
    /// %APPDATA%\classick\config.toml.
    #[arg(long)]
    pub save_config: bool,

    /// Never remove tracks from iPod, even if removed from source.
    #[arg(long)]
    pub no_delete: bool,

    /// Enable debug-level tracing output.
    #[arg(short, long)]
    pub verbose: bool,

    /// Ignore existing manifest; rebuild a best-effort one from the iPod's
    /// current iTunesDB. Existing tracks on the iPod are preserved and not
    /// touched by subsequent syncs.
    #[arg(long)]
    pub rebuild_manifest: bool,

    /// Disable the ratatui TUI; use plain log output even when stdout is a TTY.
    #[arg(long)]
    pub no_tui: bool,

    /// Speak IPC over stdin/stdout instead of rendering a TUI. Used by GUI
    /// frontends (WinUI 3 on Windows, future SwiftUI on macOS, etc.). See
    /// `docs/ipc-protocol.md` for the wire format. Implies --no-tui.
    #[arg(long)]
    pub ipc_mode: bool,

    /// Run as a long-lived background daemon. Listens on a named pipe for
    /// UI clients, handles device events + scheduling, spawns sync
    /// subprocesses on demand. See
    /// docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md.
    /// Mutually exclusive with --ipc-mode and --no-tui.
    #[arg(long, conflicts_with_all = ["ipc_mode", "no_tui"])]
    pub daemon: bool,

    /// Encoder for transcoded tracks (non-passthrough). Default: ffmpeg.
    /// Passthrough source codecs (mp3, aac, alac) are unaffected.
    #[arg(long, value_enum)]
    pub encoder: Option<EncoderChoice>,

    /// Path to refalac64.exe. Defaults to "refalac64" (PATH lookup or vendored
    /// copy alongside the binary). Only consulted when --encoder refalac.
    #[arg(long)]
    pub refalac_path: Option<PathBuf>,

    /// Copy WAV/AIFF (PCM) sources bit-perfect to the iPod instead of
    /// transcoding to ALAC. Default: transcode (saves space).
    #[arg(long)]
    pub passthrough_wav: bool,

    /// Treat every Add/Modify track as "must re-encode" regardless of the
    /// manifest's stored encoder. Useful after an ffmpeg/refalac upgrade or
    /// to switch encoders for an existing library.
    #[arg(long)]
    pub force_reencode: bool,

    /// Make transcoded .m4a files self-describing (embed tags + cover art) so
    /// an iPod running Rockbox can read the library. Persist with --save-config.
    #[arg(long)]
    pub rockbox_compat: bool,

    /// Embed tags + cover art into the EXISTING on-iPod .m4a files in place
    /// (no re-transcode), then exit. Makes an already-synced library
    /// Rockbox-readable. Requires --ipod (or auto-detect).
    #[arg(long)]
    pub backfill_rockbox: bool,

    /// Scan the source library's tags into the library index
    /// (library-index.json), then exit. Powers the Choose Music browser.
    /// Incremental: files whose (mtime, size) match the cached record are
    /// not re-read.
    #[arg(long, conflicts_with = "backfill_rockbox")]
    pub scan_library: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_no_args_with_defaults() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert_eq!(cli.source, None);
        assert_eq!(cli.ipod, None);
        assert_eq!(cli.ffmpeg, None);
        assert!(!cli.dry_run);
        assert!(!cli.no_delete);
        assert!(!cli.verbose);
        assert!(!cli.rebuild_manifest);
        assert!(!cli.no_tui);
        assert!(!cli.ipc_mode);
        assert_eq!(cli.encoder, None);
        assert!(!cli.passthrough_wav);
        assert!(!cli.force_reencode);
        assert!(cli.refalac_path.is_none());
    }

    #[test]
    fn parses_ipc_mode_flag() {
        let cli = Cli::try_parse_from(["classick", "--ipc-mode"]).unwrap();
        assert!(cli.ipc_mode);
    }

    #[test]
    fn ipc_mode_defaults_false() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.ipc_mode);
    }

    #[test]
    fn parses_explicit_encoder_refalac() {
        let cli = Cli::try_parse_from(["classick", "--encoder", "refalac"]).unwrap();
        assert_eq!(cli.encoder, Some(EncoderChoice::Refalac));
    }

    #[test]
    fn parses_explicit_encoder_ffmpeg() {
        let cli = Cli::try_parse_from(["classick", "--encoder", "ffmpeg"]).unwrap();
        assert_eq!(cli.encoder, Some(EncoderChoice::Ffmpeg));
    }

    #[test]
    fn rejects_unknown_encoder() {
        assert!(
            Cli::try_parse_from(["classick", "--encoder", "auto"]).is_err(),
            "spec's 'auto' mode was dropped per the addendum"
        );
        assert!(Cli::try_parse_from(["classick", "--encoder", "lame"]).is_err());
    }

    #[test]
    fn parses_refalac_path_passthrough_wav_force_reencode() {
        let cli = Cli::try_parse_from([
            "classick",
            "--encoder",
            "refalac",
            "--refalac-path",
            r"C:\bin\refalac64.exe",
            "--passthrough-wav",
            "--force-reencode",
        ])
        .unwrap();
        assert_eq!(cli.encoder, Some(EncoderChoice::Refalac));
        assert_eq!(
            cli.refalac_path.as_deref().and_then(|p| p.to_str()),
            Some(r"C:\bin\refalac64.exe")
        );
        assert!(cli.passthrough_wav);
        assert!(cli.force_reencode);
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::try_parse_from([
            "classick",
            "--source", r"D:\music",
            "--ipod", "G:",
            "--ffmpeg", r"C:\bin\ffmpeg.exe",
            "--dry-run",
            "--no-delete",
            "--verbose",
            "--rebuild-manifest",
            "--no-tui",
        ]).unwrap();
        assert_eq!(cli.source.as_deref().and_then(|p| p.to_str()), Some(r"D:\music"));
        assert_eq!(cli.ipod.as_deref(), Some("G:"));
        assert_eq!(cli.ffmpeg.as_deref().and_then(|p| p.to_str()), Some(r"C:\bin\ffmpeg.exe"));
        assert!(cli.dry_run);
        assert!(cli.no_delete);
        assert!(cli.verbose);
        assert!(cli.rebuild_manifest);
        assert!(cli.no_tui);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(Cli::try_parse_from(["classick", "--invented-flag"]).is_err());
    }

    #[test]
    fn parses_apply_flag() {
        let cli = Cli::try_parse_from(["classick", "--apply"]).unwrap();
        assert!(cli.apply, "expected --apply to set the apply field");
    }

    #[test]
    fn parses_save_config_flag() {
        let cli = Cli::try_parse_from(["classick", "--save-config"]).unwrap();
        assert!(cli.save_config, "expected --save-config to set the save_config field");
    }

    #[test]
    fn apply_and_save_config_default_false() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.apply);
        assert!(!cli.save_config);
    }

    #[test]
    fn parses_daemon_flag() {
        let cli = Cli::try_parse_from(["classick", "--daemon"]).unwrap();
        assert!(cli.daemon);
    }

    #[test]
    fn daemon_and_ipc_mode_conflict() {
        let result = Cli::try_parse_from(["classick", "--daemon", "--ipc-mode"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_scan_library_flag() {
        let cli = Cli::try_parse_from(["classick", "--scan-library"]).unwrap();
        assert!(cli.scan_library);
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.scan_library);
    }

    #[test]
    fn scan_library_conflicts_with_backfill_rockbox() {
        assert!(Cli::try_parse_from(["classick", "--scan-library", "--backfill-rockbox"]).is_err());
    }

    #[test]
    fn rockbox_compat_and_backfill_rockbox_default_false() {
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.rockbox_compat);
        assert!(!cli.backfill_rockbox);
    }

    #[test]
    fn parses_rockbox_compat_and_backfill_rockbox_flags() {
        let cli = Cli::try_parse_from([
            "classick",
            "--rockbox-compat",
            "--backfill-rockbox",
        ])
        .unwrap();
        assert!(cli.rockbox_compat);
        assert!(cli.backfill_rockbox);
    }
}
