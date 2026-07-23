//! clap CLI definitions. Parsing only; defaults + resolution live in `config`.

use clap::Parser;
use std::path::PathBuf;

/// Encoder choice for the transcode pipeline. Passthrough sources never see
/// this (no encoding happens). See docs/architecture.md.
/// Change 1 for why ffmpeg is the default (was: auto in the original spec).
//
// FUTURE: per-format encoder selection. If a future user wants per-source-codec
// encoder choice (e.g. flac -> refalac, opus -> ffmpeg), this enum stays as-is;
// add a `pub struct EncoderConfig { default: EncoderChoice, per_format: HashMap<String, EncoderChoice> }`
// and have apply_loop resolve `cfg.for_source(&probe.codec_name)` instead of
// passing the global `cfg.encoder`. Everything below this layer is unchanged.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
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
    /// docs/architecture.md and docs/ipc-protocol.md.
    /// Mutually exclusive with --ipc-mode and --no-tui.
    #[arg(long, conflicts_with_all = ["ipc_mode", "no_tui"])]
    pub daemon: bool,

    /// PID of the UI process that launched this daemon. Internal ownership
    /// lease used by GUI frontends; manual daemon launches omit it.
    #[arg(long, hide = true, requires = "daemon")]
    pub daemon_parent_pid: Option<u32>,

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

    /// Restore iPod_Control\iTunes\iTunesDB from the session backup
    /// (iTunesDB.classick-backup) written at the start of the last sync,
    /// then exit. A manual escape hatch for a live DB that failed to parse
    /// and wasn't auto-healed. Requires --ipod (or auto-detect).
    #[arg(long, conflicts_with_all = ["backfill_rockbox", "scan_library"])]
    pub restore_db_backup: bool,

    /// Erase EVERY track on the iPod, then sync the current selection from
    /// scratch. Unlike a normal sync (which only removes tracks the source
    /// no longer has), this wipes the device's existing library
    /// unconditionally before applying. Confirmed interactively unless
    /// --apply is also passed. Cannot be combined with the other one-shot
    /// modes below (dry-run has no meaning for a destructive operation, and
    /// a rebuilt/foreign manifest is moot once the device is wiped).
    #[arg(long, conflicts_with_all = [
        "backfill_rockbox", "scan_library", "restore_db_backup", "dry_run", "rebuild_manifest",
    ])]
    pub replace_library: bool,

    /// Cross-check every synced track's source embedded art against the
    /// on-iPod DB `has_artwork` flag and the on-disk ithmb thumbnail, then
    /// exit. Diagnostic + permanent regression harness for the cover-art
    /// pipeline bugs in LEARNINGS.md ("macOS Artwork Root Cause"). Logs each
    /// inconsistency found and exits non-zero if any are found (scriptable).
    /// Requires --ipod (or auto-detect).
    #[arg(long, conflicts_with_all = [
        "backfill_rockbox", "scan_library", "restore_db_backup", "replace_library",
        "dry_run", "rebuild_manifest",
    ])]
    pub verify_artwork: bool,

    /// Emit a complete structural iTunesDB playlist inventory as JSON, then
    /// exit. Opens the DB and ownership record read-only and performs no
    /// device write.
    #[arg(long, conflicts_with_all = [
        "apply",
        "dry_run",
        "rebuild_manifest",
        "backfill_rockbox",
        "scan_library",
        "restore_db_backup",
        "replace_library",
        "verify_artwork",
    ])]
    pub audit_playlists: bool,
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
            "--source",
            r"D:\music",
            "--ipod",
            "G:",
            "--ffmpeg",
            r"C:\bin\ffmpeg.exe",
            "--dry-run",
            "--no-delete",
            "--verbose",
            "--rebuild-manifest",
            "--no-tui",
        ])
        .unwrap();
        assert_eq!(
            cli.source.as_deref().and_then(|p| p.to_str()),
            Some(r"D:\music")
        );
        assert_eq!(cli.ipod.as_deref(), Some("G:"));
        assert_eq!(
            cli.ffmpeg.as_deref().and_then(|p| p.to_str()),
            Some(r"C:\bin\ffmpeg.exe")
        );
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
        assert!(
            cli.save_config,
            "expected --save-config to set the save_config field"
        );
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
    fn daemon_parent_pid_requires_daemon_mode() {
        assert!(Cli::try_parse_from(["classick", "--daemon-parent-pid", "42"]).is_err());
    }

    #[test]
    fn daemon_parent_pid_parses_with_daemon_mode() {
        let cli =
            Cli::try_parse_from(["classick", "--daemon", "--daemon-parent-pid", "42"]).unwrap();
        assert_eq!(cli.daemon_parent_pid, Some(42));
    }

    #[test]
    fn daemon_parent_pid_is_hidden_from_help() {
        use clap::CommandFactory;

        let mut command = Cli::command();
        let help = command.render_long_help().to_string();
        assert!(!help.contains("daemon-parent-pid"));
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
        let cli =
            Cli::try_parse_from(["classick", "--rockbox-compat", "--backfill-rockbox"]).unwrap();
        assert!(cli.rockbox_compat);
        assert!(cli.backfill_rockbox);
    }

    #[test]
    fn parses_restore_db_backup_flag() {
        let cli = Cli::try_parse_from(["classick", "--restore-db-backup"]).unwrap();
        assert!(cli.restore_db_backup);
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.restore_db_backup);
    }

    #[test]
    fn restore_db_backup_conflicts_with_backfill_rockbox_and_scan_library() {
        assert!(
            Cli::try_parse_from(["classick", "--restore-db-backup", "--backfill-rockbox"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["classick", "--restore-db-backup", "--scan-library"]).is_err()
        );
    }

    #[test]
    fn parses_replace_library_flag() {
        let cli = Cli::try_parse_from(["classick", "--replace-library"]).unwrap();
        assert!(cli.replace_library);
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.replace_library);
    }

    #[test]
    fn replace_library_conflicts_with_backfill_rockbox() {
        assert!(
            Cli::try_parse_from(["classick", "--replace-library", "--backfill-rockbox"]).is_err()
        );
    }

    #[test]
    fn replace_library_conflicts_with_scan_library() {
        assert!(Cli::try_parse_from(["classick", "--replace-library", "--scan-library"]).is_err());
    }

    #[test]
    fn replace_library_conflicts_with_restore_db_backup() {
        assert!(
            Cli::try_parse_from(["classick", "--replace-library", "--restore-db-backup"]).is_err()
        );
    }

    #[test]
    fn replace_library_conflicts_with_dry_run() {
        assert!(Cli::try_parse_from(["classick", "--replace-library", "--dry-run"]).is_err());
    }

    #[test]
    fn replace_library_conflicts_with_rebuild_manifest() {
        assert!(
            Cli::try_parse_from(["classick", "--replace-library", "--rebuild-manifest"]).is_err()
        );
    }

    #[test]
    fn replace_library_combines_with_apply() {
        let cli = Cli::try_parse_from(["classick", "--replace-library", "--apply"]).unwrap();
        assert!(cli.replace_library);
        assert!(cli.apply);
    }

    #[test]
    fn parses_verify_artwork_flag() {
        let cli = Cli::try_parse_from(["classick", "--verify-artwork"]).unwrap();
        assert!(cli.verify_artwork);
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.verify_artwork);
    }

    #[test]
    fn verify_artwork_conflicts_with_backfill_rockbox() {
        assert!(
            Cli::try_parse_from(["classick", "--verify-artwork", "--backfill-rockbox"]).is_err()
        );
    }

    #[test]
    fn verify_artwork_conflicts_with_scan_library() {
        assert!(Cli::try_parse_from(["classick", "--verify-artwork", "--scan-library"]).is_err());
    }

    #[test]
    fn verify_artwork_conflicts_with_restore_db_backup() {
        assert!(
            Cli::try_parse_from(["classick", "--verify-artwork", "--restore-db-backup"]).is_err()
        );
    }

    #[test]
    fn verify_artwork_conflicts_with_replace_library() {
        assert!(
            Cli::try_parse_from(["classick", "--verify-artwork", "--replace-library"]).is_err()
        );
    }

    #[test]
    fn verify_artwork_conflicts_with_dry_run() {
        assert!(Cli::try_parse_from(["classick", "--verify-artwork", "--dry-run"]).is_err());
    }

    #[test]
    fn verify_artwork_conflicts_with_rebuild_manifest() {
        assert!(
            Cli::try_parse_from(["classick", "--verify-artwork", "--rebuild-manifest"]).is_err()
        );
    }
}
