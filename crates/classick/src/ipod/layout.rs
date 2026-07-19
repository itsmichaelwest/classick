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
pub const CLASSICK: &str = "classick";
pub const PLAYLISTS: &str = "playlists";
pub const MANAGED_PLAYLISTS: &str = "managed_playlists.json";

/// `/Playlists/Classick/` — Rockbox-compatible playlist projections.
pub fn rockbox_playlists_dir(mount: &Path) -> PathBuf {
    crate::rockbox_playlist::ROCKBOX_PLAYLIST_DIR
        .split('/')
        .fold(mount.to_path_buf(), |path, component| path.join(component))
}

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

/// `<mount>\iPod_Control\classick\playlists\` — the on-device mirror of the
/// host's playlist store (Task 6). Not read by libgpod or the Apple/Rockbox
/// firmware; purely a Classick-to-Classick backup channel so a fresh
/// install (or a different machine) can adopt playlists back from a
/// previously-synced iPod. See `device_playlists::mirror_to_ipod` /
/// `adopt_from_ipod`.
pub fn playlists_mirror_dir(mount: &Path) -> PathBuf {
    mount.join(IPOD_CONTROL).join(CLASSICK).join(PLAYLISTS)
}

/// Device-authoritative record of the normal playlists Classick may mutate.
pub fn managed_playlists_path(mount: &Path) -> PathBuf {
    mount
        .join(IPOD_CONTROL)
        .join(CLASSICK)
        .join(MANAGED_PLAYLISTS)
}

/// Canonical "is this a usable iPod mount?" predicate. Requires BOTH the
/// `SysInfo` file (we need FirewireGuid + ModelNumStr to identify the
/// device) AND the `iTunesDB` (we need to be able to read + write tracks).
/// A device with only one is mid-restore or corrupted; we don't try to
/// sync to it. See findings F-09.
pub fn is_ipod_mount(mount: &Path) -> bool {
    sysinfo_path(mount).exists() && itunes_db_path(mount).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rockbox_playlist_directory_uses_host_path_components() {
        assert_eq!(
            rockbox_playlists_dir(Path::new("mount")),
            Path::new("mount").join("Playlists").join("Classick")
        );
    }
}
