use crate::ipod::db::{
    ensure_managed_playlist, remove_playlist_by_id, OwnedDb, PlaylistStructuralKind,
};
use crate::ipod::playlist_ownership::{
    ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership,
};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredPlaylist {
    pub slug: String,
    pub display_name: String,
    pub ordered_dbids: Vec<u64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    pub created: usize,
    pub updated: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistDiagnostic {
    InvalidManagedAssociation {
        slug: String,
        playlist_id: u64,
        actual_kind: Option<PlaylistStructuralKind>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistReconcileOutcome {
    pub candidate_ownership: ManagedPlaylistOwnership,
    pub desired_memberships: BTreeMap<String, Vec<u64>>,
    pub stats: ReconcileStats,
    pub diagnostics: Vec<PlaylistDiagnostic>,
}

pub fn reconcile_candidate(
    db: &OwnedDb,
    desired: &[DesiredPlaylist],
    previous: &ManagedPlaylistOwnership,
) -> Result<PlaylistReconcileOutcome> {
    previous
        .validate_for_serial(&previous.device_serial)
        .context("validate previous device playlist ownership")?;
    validate_desired(desired)?;

    let desired_slugs: HashSet<&str> = desired.iter().map(|item| item.slug.as_str()).collect();
    let mut candidate = ManagedPlaylistOwnership::empty_for_serial(&previous.device_serial);
    let mut desired_memberships = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut stats = ReconcileStats::default();
    let mut reused_ids = HashSet::new();

    for item in desired {
        let prior = previous.playlists.get(&item.slug);
        let recorded_id = prior.map(|entry| entry.apple_playlist_id);
        let reusable_id = recorded_id.filter(|id| {
            db.playlist_kind_by_id(*id) == Some(PlaylistStructuralKind::Normal)
                && reused_ids.insert(*id)
        });
        if let Some(id) = recorded_id {
            if reusable_id.is_none() {
                diagnostics.push(invalid_association(db, &item.slug, id));
            }
        }
        let apple_playlist_id =
            ensure_managed_playlist(db, &item.display_name, &item.ordered_dbids, reusable_id)
                .with_context(|| format!("reconcile managed playlist {:?}", item.slug))?;
        if reusable_id == Some(apple_playlist_id) {
            stats.updated += 1;
        } else {
            stats.created += 1;
        }
        candidate.playlists.insert(
            item.slug.clone(),
            ManagedPlaylistEntry {
                apple_playlist_id,
                expected_kind: ManagedPlaylistKind::Normal,
                rockbox: prior.and_then(|entry| entry.rockbox.clone()),
            },
        );
        desired_memberships.insert(item.slug.clone(), item.ordered_dbids.clone());
    }

    let retained_ids: HashSet<u64> = candidate
        .playlists
        .values()
        .map(|entry| entry.apple_playlist_id)
        .collect();
    let mut removed_ids = HashSet::new();
    for (slug, prior) in &previous.playlists {
        if desired_slugs.contains(slug.as_str()) || retained_ids.contains(&prior.apple_playlist_id)
        {
            continue;
        }
        let id = prior.apple_playlist_id;
        match db.playlist_kind_by_id(id) {
            Some(PlaylistStructuralKind::Normal) if removed_ids.insert(id) => {
                if remove_playlist_by_id(db, id)
                    .with_context(|| format!("remove unsubscribed managed playlist {slug:?}"))?
                {
                    stats.removed += 1;
                }
            }
            Some(PlaylistStructuralKind::Normal) => {}
            _ => diagnostics.push(invalid_association(db, slug, id)),
        }
    }

    candidate
        .validate_for_serial(&previous.device_serial)
        .context("validate candidate device playlist ownership")?;
    Ok(PlaylistReconcileOutcome {
        candidate_ownership: candidate,
        desired_memberships,
        stats,
        diagnostics,
    })
}

fn validate_desired(desired: &[DesiredPlaylist]) -> Result<()> {
    let mut slugs = HashSet::new();
    for item in desired {
        if item.slug.is_empty()
            || !item
                .slug
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            bail!("unsafe desired playlist slug {:?}", item.slug);
        }
        if !slugs.insert(item.slug.as_str()) {
            bail!("duplicate desired playlist slug {:?}", item.slug);
        }
    }
    Ok(())
}

fn invalid_association(db: &OwnedDb, slug: &str, playlist_id: u64) -> PlaylistDiagnostic {
    PlaylistDiagnostic::InvalidManagedAssociation {
        slug: slug.to_string(),
        playlist_id,
        actual_kind: db.playlist_kind_by_id(playlist_id),
    }
}
