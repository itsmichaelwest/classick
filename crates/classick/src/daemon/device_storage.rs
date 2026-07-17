//! Daemon-facing free/total-bytes lookup for a mounted iPod drive.
//!
//! The actual platform query lives in the core `free_space` module
//! (`crate::free_space`) so it's reusable outside the daemon. This
//! module just adapts the daemon's `drive: &str` convention to
//! `free_space::query`'s `&Path` and re-exports `StorageInfo` so
//! `ipc_daemon.rs` (the daemon wire format) keeps its existing import
//! path.

use std::path::Path;

pub use crate::free_space::StorageInfo;

/// Returns `None` if the drive is unreachable (unplugged, permissions,
/// path invalid) or the underlying syscall fails. The caller treats
/// absence as "no storage info available yet" and the UI shows a
/// neutral placeholder.
pub fn query_storage(drive: &str) -> Option<StorageInfo> {
    crate::free_space::query(Path::new(drive))
}
