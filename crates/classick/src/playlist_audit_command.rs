use crate::atomic_file::AtomicFileWriter;
use crate::ipod::playlist_audit::{audit_playlists, PlaylistAudit};
use crate::ipod::playlist_ownership::DeviceOwnershipStore;
use crate::ipod::{detect_ipod_mount, read_firewire_guid, OwnedDb};
use crate::progress::Progress;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Audit the explicitly selected or auto-detected iPod without resolving any
/// source-library configuration and emit one presentation-safe JSON payload.
pub fn run(ipod: Option<&str>, progress: &Progress) -> Result<PlaylistAudit> {
    let mount = match ipod {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(detect_ipod_mount().context("detect iPod for playlist audit")?),
    };
    let serial = read_firewire_guid(&mount).context("read raw iPod serial for playlist audit")?;
    let audit = run_at(&mount, &serial)?;
    progress.log(serde_json::to_string_pretty(&audit).context("serialize playlist audit as JSON")?);
    Ok(audit)
}

/// Read-only audit entry point with identity injected for deterministic tests.
pub fn run_at(mount: &Path, serial: &str) -> Result<PlaylistAudit> {
    let db = OwnedDb::open(mount).context("open iTunesDB read-only for playlist audit")?;
    let host_cache = crate::device_state::managed_playlists_cache_path(serial)
        .context("resolve read-only host playlist cache path")?;
    let ownership = DeviceOwnershipStore::new(
        mount.to_path_buf(),
        serial.to_string(),
        host_cache,
        AtomicFileWriter::new(),
    )
    .load_device_read_only()
    .context("load device playlist ownership read-only")?;
    Ok(audit_playlists(&db, &ownership))
}
