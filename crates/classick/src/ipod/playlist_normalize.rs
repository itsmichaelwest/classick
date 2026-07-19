use crate::ffi;
use crate::ipod::playlist_audit::snapshot_playlists;
use crate::ipod::playlist_profile::{match_firmware_profile, FirmwareProfileId};
use crate::ipod::OwnedDb;
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FirmwareNormalizationReport {
    pub kept: Vec<u64>,
    pub removed: Vec<u64>,
}

pub fn normalize_firmware_playlists(db: &OwnedDb) -> Result<FirmwareNormalizationReport> {
    let profile = FirmwareProfileId::IpodClassicVideoKindV1;
    let mut exact = snapshot_playlists(db)
        .into_iter()
        .filter(|playlist| match_firmware_profile(playlist) == Some(profile))
        .collect::<Vec<_>>();
    exact.sort_by_key(|playlist| (playlist.timestamp, playlist.id));

    let Some(keep) = exact.pop() else {
        return Ok(FirmwareNormalizationReport::default());
    };
    let mut removed = Vec::with_capacity(exact.len());
    for duplicate in exact {
        remove_firmware_playlist_exact(db, duplicate.id, profile)?;
        removed.push(duplicate.id);
    }
    Ok(FirmwareNormalizationReport {
        kept: vec![keep.id],
        removed,
    })
}

fn remove_firmware_playlist_exact(db: &OwnedDb, id: u64, profile: FirmwareProfileId) -> Result<()> {
    let current = snapshot_playlists(db)
        .into_iter()
        .find(|playlist| playlist.id == id)
        .with_context(|| format!("firmware playlist {id} disappeared before removal"))?;
    if match_firmware_profile(&current) != Some(profile) {
        bail!("firmware playlist {id} changed before exact removal; refusing to delete it");
    }

    unsafe {
        let playlist = ffi::itdb_playlist_by_id(db.as_ptr(), id);
        if playlist.is_null() {
            bail!("firmware playlist {id} disappeared after validation");
        }
        ffi::itdb_playlist_remove(playlist);
    }
    Ok(())
}
