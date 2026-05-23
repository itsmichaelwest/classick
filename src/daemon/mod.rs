//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod device_watcher;
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod scheduler;
pub mod state;
