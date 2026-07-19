use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::ipod::db::{OwnedDb, PlaylistStructuralKind};
use crate::ipod::device_playlists::VerifiedPlaylistMembership;
use crate::ipod::playlist_ownership::{
    DeviceOwnershipStore, ManagedPlaylistOwnership, OwnershipOrigin,
};
use crate::pending_session::{PendingPhase, PendingSession, PendingSessionStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistFailurePoint {
    BeforeDatabaseWrite,
    AfterDatabaseVerified,
    BeforeProjectionPlanPersist,
    AfterProjectionPlanPrepared,
    BeforeDeviceOwnershipRename,
    AfterDeviceOwnershipRename,
    BeforeHostCacheRefresh,
}

pub fn inject(point: Option<PlaylistFailurePoint>, expected: PlaylistFailurePoint) -> Result<()> {
    if point == Some(expected) {
        bail!("injected playlist publication failure at {expected:?}");
    }
    Ok(())
}

pub fn verify_managed_playlists(
    reopened: &OwnedDb,
    candidate: &ManagedPlaylistOwnership,
    desired_memberships: &BTreeMap<String, Vec<u64>>,
) -> Result<Vec<VerifiedPlaylistMembership>> {
    candidate
        .validate_for_serial(&candidate.device_serial)
        .context("validate candidate playlist ownership before verification")?;
    if candidate.playlists.len() != desired_memberships.len()
        || candidate
            .playlists
            .keys()
            .any(|slug| !desired_memberships.contains_key(slug))
    {
        bail!("candidate ownership and desired playlist memberships differ");
    }
    let mut seen_ids = HashSet::new();
    let mut verified = Vec::with_capacity(candidate.playlists.len());
    for (slug, entry) in &candidate.playlists {
        if !seen_ids.insert(entry.apple_playlist_id) {
            bail!(
                "duplicate managed Apple playlist id {}",
                entry.apple_playlist_id
            );
        }
        if reopened.playlist_kind_by_id(entry.apple_playlist_id)
            != Some(PlaylistStructuralKind::Normal)
        {
            bail!(
                "managed playlist {slug:?} id {} is missing or not normal",
                entry.apple_playlist_id
            );
        }
        let members = reopened
            .normal_playlist_members_by_id(entry.apple_playlist_id)
            .with_context(|| format!("read managed playlist {slug:?} members"))?;
        let ordered_dbids = members.iter().map(|(dbid, _)| *dbid).collect::<Vec<_>>();
        let desired = desired_memberships
            .get(slug)
            .context("desired membership missing after key-set validation")?;
        if ordered_dbids != *desired {
            bail!("managed playlist {slug:?} ordered membership differs from journal");
        }
        let ordered_ipod_paths = members
            .into_iter()
            .map(|(_, path)| normalize_device_path(&path))
            .collect::<Result<Vec<_>>>()?;
        verified.push(VerifiedPlaylistMembership {
            slug: slug.clone(),
            apple_playlist_id: entry.apple_playlist_id,
            ordered_dbids,
            ordered_ipod_paths,
        });
    }
    Ok(verified)
}

fn normalize_device_path(path: &str) -> Result<String> {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/');
    let components = trimmed.split('/').collect::<Vec<_>>();
    if trimmed.is_empty()
        || components
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        || components.len() < 3
        || !components[0].eq_ignore_ascii_case("ipod_control")
        || !components[1].eq_ignore_ascii_case("music")
        || trimmed.contains(['\0', '\r', '\n'])
    {
        bail!("invalid verified iPod path {path:?}");
    }
    Ok(format!("/{trimmed}"))
}

pub fn ownership_store(
    mount: &Path,
    serial: &str,
    state_root: Option<&Path>,
) -> Result<DeviceOwnershipStore> {
    let host_cache = match state_root {
        Some(root) => crate::device_state::managed_playlists_path_in(root, serial)?,
        None => crate::device_state::managed_playlists_path(serial)?,
    };
    Ok(DeviceOwnershipStore::new(
        mount.to_path_buf(),
        serial.to_string(),
        host_cache,
        crate::atomic_file::AtomicFileWriter::new(),
    ))
}

pub fn publish_ownership(
    journal: &mut PendingSession,
    store: &PendingSessionStore,
    ownership: &DeviceOwnershipStore,
    failure: Option<PlaylistFailurePoint>,
) -> Result<()> {
    let candidate = journal
        .candidate_playlist_ownership
        .as_ref()
        .context("prepared transaction has no candidate playlist ownership")?;
    inject(failure, PlaylistFailurePoint::BeforeDeviceOwnershipRename)
        .context("publish device playlist ownership")?;
    let published = ownership.load_device_read_only_with_origin()?;
    if published.origin != OwnershipOrigin::Device || published.value != *candidate {
        ownership.publish_device(candidate)?;
    }
    journal.phase = PendingPhase::PlaylistOwnershipPublished;
    store.save(journal)?;
    inject(failure, PlaylistFailurePoint::AfterDeviceOwnershipRename)
}

pub fn refresh_host_cache(
    journal: &PendingSession,
    ownership: &DeviceOwnershipStore,
    failure: Option<PlaylistFailurePoint>,
) -> Option<String> {
    if let Err(error) = inject(failure, PlaylistFailurePoint::BeforeHostCacheRefresh) {
        return Some(format!("{error:#}"));
    }
    let candidate = journal.candidate_playlist_ownership.as_ref()?;
    match ownership.refresh_host_cache(candidate) {
        Ok(warning) => warning,
        Err(error) => Some(format!("{error:#}")),
    }
}
