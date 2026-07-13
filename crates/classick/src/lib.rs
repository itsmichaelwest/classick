/// Project identifier used as the per-user data directory name
/// (`%APPDATA%\classick\`, `%LOCALAPPDATA%\classick\logs\`), the named-pipe
/// label (`\\.\pipe\classick`), the temp-dir subdirectory, and the lowercase
/// stem behind binary/socket/pipe paths. The .NET side mirrors this in
/// `Classick.UI.Core.AppIdentity`; the two MUST stay in sync (named-pipe
/// label is the IPC contract). See findings F-02 for the rationale.
pub const PROJECT_DIR: &str = "classick";

/// User-facing brand name. Used in wizard prompts, toast titles, log
/// banners, and anywhere the product is named to a human. Kept separate
/// from [`PROJECT_DIR`] so the on-disk/IPC identifier can stay lowercase
/// and case-insensitive while the display name preserves capitalization.
pub const DISPLAY_NAME: &str = "Classick";

/// How many completed apply-loop actions trigger a mid-sync checkpoint
/// (`db.write()` + `manifest::save_atomic`). Without this, a daemon
/// crash / USB unplug / power loss mid-sync leaves every file already
/// copied via `itdb_cp_track_to_ipod` as an orphan: present under
/// `iPod_Control\Music\F**` but unreferenced by the iTunesDB on disk
/// (since the only `db.write()` was at the very end of the apply loop).
/// With checkpoints, the worst-case orphan window is `N` tracks.
///
/// 25 picked as a compromise: on a ~1,400-track library that's ~56
/// checkpoints × ~100ms each ≈ 5.6s overhead on a ~90min sync (<0.2%).
/// Lower N = safer-but-slower; higher N = larger orphan window on
/// crash. Tunable if real-world failure modes warrant it.
pub const SYNC_CHECKPOINT_EVERY: usize = 25;

/// Checkpoint when EITHER this many tracks have committed OR
/// `CHECKPOINT_MAX_SECONDS` have elapsed since the last checkpoint.
pub const CHECKPOINT_MAX_TRACKS: usize = 10;
pub const CHECKPOINT_MAX_SECONDS: u64 = 60;

use std::time::Duration;

/// Backoff schedule for transient iPod-write retries (add/copy, delete,
/// checkpoint write). 3 delays = up to 3 retries. See `retry_transient`.
pub const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
];

/// Concurrent afconvert transcode workers (afconvert is CPU-bound; oversubscribing
/// hurts). Resolved at runtime via available_parallelism, capped.
pub fn transcode_workers() -> usize {
    std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(1).clamp(1, 4)
}
/// Max jobs transcoded ahead of the committer (bounds temp-file disk use).
pub const PIPELINE_WINDOW: usize = 8;

pub mod apply_loop;
pub mod checkpoint;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod daemon;
pub mod ffi;
pub mod ipc;
pub mod ipc_daemon;
pub mod ipod;
pub mod logging;
pub mod manifest;
pub mod orchestrator;
pub mod pipeline;
pub mod preflight;
pub mod progress;
#[cfg(windows)]
pub mod scsi_inquiry;
pub mod source;
pub mod sysinfo_extended;
pub mod tags;
pub mod transcode;
pub mod try_with_prompt;
pub mod windows_proc;
pub mod wizard;
