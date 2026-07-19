//! Long-lived daemon mode (`classick --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod command_handler;
pub(crate) mod device_config_transaction;
pub mod device_registry;
pub mod device_snapshot;
pub mod device_storage;
pub mod device_watcher;
pub mod format;
pub mod history;
#[cfg(target_os = "macos")]
pub mod iokit_watcher;
pub mod ipc_server;
pub mod library;
pub(crate) mod library_drop;
pub mod library_mutations;
pub mod library_watcher;
pub mod lifecycle;
#[cfg(target_os = "macos")]
pub mod macos_netfs;
pub(crate) mod mutation_ledger;
pub(crate) mod playlist_commands;
pub mod runtime;
pub mod runtime_state;
pub mod scheduler;
pub mod session_admission;
pub mod source_availability;
pub mod state;
pub mod sync_orchestrator;
#[cfg(unix)]
pub(crate) mod unix_socket;

// ---------------------------------------------------------------------------
// Tuning constants (F-27, F-28). Grouped here so the relationships between
// them are visible at a glance. Invariants to preserve when changing:
//   * DEVICE_POLL_INTERVAL > DEVICE_DEBOUNCE_WINDOW × 2 (debounce must
//     absorb at least one duplicate scan within its window).
//   * BROADCAST_CHANNEL_CAPACITY >> typical events-per-second × max-UI-stall
//     (UI lag of a few seconds × thousands of track events must not drop
//     events; see F-27).
// ---------------------------------------------------------------------------

/// Default schedule (minutes) when config.toml is absent or daemon settings
/// are missing. Matches `DaemonSettings::default().schedule_minutes`.
pub const DEFAULT_SCHEDULE_MINUTES: u32 = 30;

/// Capacity of the daemon's broadcast channel. Forwarded sync-subprocess events
/// (1 per IPC line) can spike to thousands per second on a large library; this
/// must stay well above the UI's worst-case lag-times-throughput to avoid
/// `RecvError::Lagged` drops on the UI side (per F-27).
pub const BROADCAST_CHANNEL_CAPACITY: usize = 1024;

/// Debounce window for `Debouncer` — collapses duplicate Connected events
/// for the same serial that fire during Windows' multi-step drive enumeration.
pub const DEVICE_DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Polling interval for `PollingDeviceWatcher`. Must exceed the debounce
/// window so each scan can be debounced if duplicate-fired by the OS.
pub const DEVICE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

/// Quiet period after the last filesystem event before a watcher-triggered
/// library scan fires. Bulk file operations (a Lidarr import, a big copy) emit
/// many events; this coalesces them into one scan.
pub const LIBRARY_DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// Capacity of the mpsc channel `PollingDeviceWatcher` emits on. Events are
/// drained by the runtime's `select!` on every iteration; 32 buffers the
/// occasional flap without blocking the watcher.
pub const DEVICE_EVENT_CHANNEL_CAPACITY: usize = 32;

/// Grace period for the sync orchestrator's bounded_kill after a Cancel
/// command. The subprocess gets `cancel\n` on stdin, then this long to exit
/// cleanly before we hard-kill it.
pub const SYNC_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Backstop grace period after a Pause command, before the orchestrator
/// force-kills a subprocess that never drained. Generous relative to
/// `SYNC_KILL_GRACE` because a legitimate pause-exit has more work to do than
/// a cancel: at most one in-flight libgpod track add, plus the final
/// `db.write()` and manifest save. This should only ever fire if the
/// subprocess genuinely wedges (e.g. a libgpod/FS write stuck on a slow
/// spinning-HDD + fskit FAT32 volume during the final checkpoint).
pub const PAUSE_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

/// How long the daemon waits, after a graceful Shutdown command, for an
/// in-flight sync to drain (cancel → write iTunesDB → exit cleanly) before
/// returning from the main loop. Must be larger than `SYNC_KILL_GRACE` so
/// the orchestrator's own bounded_kill has time to fire first; the +3s
/// padding covers libgpod's final `itdb_write` on a large library.
pub const SHUTDOWN_DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);
