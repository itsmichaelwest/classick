//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod device_watcher;
#[cfg(windows)]
pub mod device_storage;
pub mod format;
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod scheduler;
pub mod state;
#[cfg(windows)]
pub mod sync_orchestrator;

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
pub const DEVICE_DEBOUNCE_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Polling interval for `PollingDeviceWatcher`. Must exceed the debounce
/// window so each scan can be debounced if duplicate-fired by the OS.
pub const DEVICE_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// Capacity of the mpsc channel `PollingDeviceWatcher` emits on. Events are
/// drained by the runtime's `select!` on every iteration; 32 buffers the
/// occasional flap without blocking the watcher.
pub const DEVICE_EVENT_CHANNEL_CAPACITY: usize = 32;

/// Grace period for the sync orchestrator's bounded_kill after a Cancel
/// command. The subprocess gets `cancel\n` on stdin, then this long to exit
/// cleanly before we hard-kill it.
pub const SYNC_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
