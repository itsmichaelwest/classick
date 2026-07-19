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

/// Checkpoint when EITHER this many tracks have committed OR
/// `CHECKPOINT_MAX_SECONDS` have elapsed since the last checkpoint.
pub const CHECKPOINT_MAX_TRACKS: usize = 10;
pub const CHECKPOINT_MAX_SECONDS: u64 = 60;

/// Minimum bytes to hold back below the reported free space when planning
/// what fits on the device (`fit::reserve_bytes`). FAT32 (the iPod Classic's
/// filesystem) misbehaves badly when driven to exactly 100% full — libgpod
/// writes can fail partway through, corrupting the iTunesDB rather than
/// cleanly rejecting the sync. A fixed floor protects small/near-empty
/// devices where a fraction-only reserve would round to nearly nothing.
pub const FIT_RESERVE_MIN_BYTES: u64 = 512 * 1024 * 1024;
/// Fraction of total device capacity to additionally hold back, on top of
/// [`FIT_RESERVE_MIN_BYTES`], so large-capacity devices keep a proportional
/// safety margin rather than always reserving the same fixed floor.
pub const FIT_RESERVE_FRACTION: f64 = 0.02;

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
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .clamp(1, 4)
}
/// Max jobs transcoded ahead of the committer (bounds temp-file disk use).
pub const PIPELINE_WINDOW: usize = 8;

pub mod apply_loop;
pub mod art_audit;
pub mod artwork;
pub mod artwork_cache;
pub mod atomic_file;
pub mod checkpoint;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod daemon;
pub mod device_config;
pub mod device_state;
pub mod ffi;
pub mod fit;
pub mod free_space;
pub mod ipc;
pub mod ipc_daemon;
pub mod ipc_device;
pub mod ipod;
pub mod library_index;
pub mod logging;
pub mod manifest;
pub mod manifest_store;
pub mod orchestrator;
pub mod pending_session;
pub mod pipeline;
pub mod playlist;
pub mod playlist_audit_command;
pub mod playlist_rules;
pub mod portable_path;
pub mod preflight;
pub mod progress;
pub mod scan;
#[cfg(windows)]
pub mod scsi_inquiry;
pub mod selection;
pub mod source;
pub mod source_location;
pub mod sync_set;
pub mod sync_transaction;
pub mod sysinfo_extended;
pub mod tags;
pub mod transcode;
pub mod try_with_prompt;
pub mod windows_proc;
pub mod wizard;
