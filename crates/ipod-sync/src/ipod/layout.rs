//! iPod filesystem layout constants + path helpers.
//!
//! Centralizes the on-disk paths libgpod and our own code reach for, so a
//! change to the layout (or even a typo at a single site) can't desync the
//! codebase. See findings F-08 for the rationale.

use std::path::{Path, PathBuf};

pub const IPOD_CONTROL: &str = "iPod_Control";
pub const DEVICE: &str = "Device";
pub const ITUNES: &str = "iTunes";
pub const SYSINFO: &str = "SysInfo";
pub const ITUNES_DB: &str = "iTunesDB";
pub const PLAY_COUNTS_BAK: &str = "Play Counts.bak";

/// `<mount>\iPod_Control\Device\SysInfo` — the flat-text key/value file we
/// read FirewireGuid + ModelNumStr from. Present on every iPod we support.
pub fn sysinfo_path(mount: &Path) -> PathBuf {
    mount.join(IPOD_CONTROL).join(DEVICE).join(SYSINFO)
}

/// `<mount>\iPod_Control\iTunes\iTunesDB` — the hashed-and-signed track DB
/// libgpod parses + writes. Its presence is our canonical "this is an iPod"
/// indicator at the apply-loop level.
pub fn itunes_db_path(mount: &Path) -> PathBuf {
    mount.join(IPOD_CONTROL).join(ITUNES).join(ITUNES_DB)
}

/// `<mount>\iPod_Control\iTunes\Play Counts.bak` — the stale backup that
/// libgpod's POSIX rename() trips over on Windows. `OwnedDb::write` pre-emptively
/// removes this file before each `itdb_write`.
pub fn play_counts_bak_path(mount: &Path) -> PathBuf {
    mount.join(IPOD_CONTROL).join(ITUNES).join(PLAY_COUNTS_BAK)
}

/// Canonical "is this a usable iPod mount?" predicate. Requires BOTH the
/// `SysInfo` file (we need FirewireGuid + ModelNumStr to identify the
/// device) AND the `iTunesDB` (we need to be able to read + write tracks).
/// A device with only one is mid-restore or corrupted; we don't try to
/// sync to it. See findings F-09.
pub fn is_ipod_mount(mount: &Path) -> bool {
    sysinfo_path(mount).exists() && itunes_db_path(mount).exists()
}
