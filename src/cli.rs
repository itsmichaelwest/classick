//! clap CLI definitions. Parsing only; defaults + resolution live in `config`.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "ipod-sync",
    version,
    about = "Sync a FLAC library to an iPod Classic via libgpod with on-the-fly ALAC transcoding."
)]
pub struct Cli {
    /// Source library root. Defaults to <source-library-path>\.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_no_args_with_defaults() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        assert_eq!(cli.source, None);
        assert_eq!(cli.ipod, None);
        assert_eq!(cli.ffmpeg, None);
        assert!(!cli.dry_run);
        assert!(!cli.no_delete);
        assert!(!cli.verbose);
        assert!(!cli.rebuild_manifest);
        assert!(!cli.no_tui);
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::try_parse_from([
            "ipod-sync",
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
        assert!(Cli::try_parse_from(["ipod-sync", "--invented-flag"]).is_err());
    }
}
