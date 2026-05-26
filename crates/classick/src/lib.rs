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

pub mod apply_loop;
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
