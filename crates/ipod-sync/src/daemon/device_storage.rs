//! Free + total bytes for a mounted iPod drive.
//!
//! Windows: direct FFI to `GetDiskFreeSpaceExW` (kept in tree rather than
//! pulled via the `windows`/`windows-sys` crates for one call — the
//! surface is tiny and zero new deps keeps the daemon's build matrix
//! unchanged).
//!
//! Non-Windows: a no-op stub returning `None`. A real implementation
//! (e.g. `statvfs` on Linux/macOS) can land here without touching any
//! caller — `Option<StorageInfo>` already expresses "no info yet" and
//! the UI renders a neutral placeholder when the field is absent.

/// Snapshot of a drive's capacity. Sent on `DaemonEvent::StatusUpdate`
/// so the popover can render storage used / free without itself having
/// to know the iPod's drive letter / mount path.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct StorageInfo {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[cfg(windows)]
mod imp {
    use super::StorageInfo;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lp_directory_name: *const u16,
            lp_free_bytes_available_to_caller: *mut u64,
            lp_total_number_of_bytes: *mut u64,
            lp_total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }

    pub fn query_storage(drive: &str) -> Option<StorageInfo> {
        // Win32 requires a trailing slash on the path to query the volume
        // root (e.g. "E:\"), not the volume label.
        let path = if drive.ends_with('\\') || drive.ends_with('/') {
            drive.to_string()
        } else {
            format!("{drive}\\")
        };
        let wide: Vec<u16> = Path::new(&path)
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut free_caller: u64 = 0;
        let mut total: u64 = 0;
        let mut free_total: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_caller as *mut u64,
                &mut total as *mut u64,
                &mut free_total as *mut u64,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(StorageInfo {
            total_bytes: total,
            free_bytes: free_caller,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::StorageInfo;

    // No statvfs/statfs bindings yet on non-Windows. Callers already
    // handle `None` as "no info yet" so this is wire-compatible until
    // a real impl lands.
    pub fn query_storage(_drive: &str) -> Option<StorageInfo> {
        None
    }
}

/// Returns `None` if the drive is unreachable (unplugged, permissions,
/// path invalid) or — on non-Windows — until a native impl lands. The
/// caller treats absence as "no storage info available yet" and the UI
/// shows a neutral placeholder.
pub fn query_storage(drive: &str) -> Option<StorageInfo> {
    imp::query_storage(drive)
}
