use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashSet};

use crate::ipod::device_playlists::VerifiedPlaylistMembership;
use crate::ipod::playlist_ownership::{DeviceOwnershipStore, ManagedPlaylistOwnership};
use crate::pending_session::{
    PendingPhase, PendingSession, PendingSessionStore, ROCKBOX_PROJECTION_PLAN_VERSION,
};
use crate::rockbox_playlist::{render_verified_paths, validate_projection_record};
use crate::rockbox_projection::{plan_projection, DesiredVerifiedPlaylist, ProjectionPlan};
use crate::rockbox_projection_fs::{ProjectionIo, TargetState};

use super::playlist_publication::{self, PlaylistFailurePoint};
use super::DesiredPlaylist;

pub fn prepare_playlist_projection(
    journal: &mut PendingSession,
    store: &PendingSessionStore,
    settled: &ManagedPlaylistOwnership,
    enabled: bool,
    desired_playlists: Option<&[DesiredPlaylist]>,
    projection_io: &dyn ProjectionIo,
    failure: Option<PlaylistFailurePoint>,
) -> Result<()> {
    if journal.phase != PendingPhase::DeviceManifestPublished {
        bail!("Rockbox projection planning requires DeviceManifestPublished");
    }
    if !journal.pending_rockbox_ops.is_empty() || journal.rockbox_projection_plan_version.is_some()
    {
        bail!(
            "DeviceManifestPublished journal already contains Rockbox state; projection publisher refuses to overwrite it"
        );
    }
    let candidate = journal
        .candidate_playlist_ownership
        .as_ref()
        .context("verified transaction has no candidate playlist ownership")?;
    let desired = desired_verified_playlists(journal, desired_playlists)?;
    playlist_publication::inject(failure, PlaylistFailurePoint::BeforeProjectionPlanPersist)?;
    let plan = plan_projection(
        &journal.serial,
        enabled,
        &desired,
        settled,
        candidate,
        projection_io,
    )?;
    stage_playlist_projection(store, journal, plan)?;
    playlist_publication::inject(failure, PlaylistFailurePoint::AfterProjectionPlanPrepared)
}

fn desired_verified_playlists(
    journal: &PendingSession,
    desired_playlists: Option<&[DesiredPlaylist]>,
) -> Result<Vec<DesiredVerifiedPlaylist>> {
    let names = desired_playlists
        .context("playlist display names unavailable while preparing Rockbox projections")?
        .iter()
        .map(|(slug, name, _)| (slug.as_str(), name.as_str()))
        .collect::<BTreeMap<_, _>>();
    journal
        .verified_playlist_memberships
        .iter()
        .map(|membership| {
            let display_name = names
                .get(membership.slug.as_str())
                .with_context(|| {
                    format!(
                        "display name missing for verified playlist {:?}",
                        membership.slug
                    )
                })?
                .to_string();
            Ok(DesiredVerifiedPlaylist {
                display_name,
                membership: membership.clone(),
            })
        })
        .collect()
}

pub fn stage_playlist_projection(
    store: &PendingSessionStore,
    journal: &mut PendingSession,
    plan: ProjectionPlan,
) -> Result<()> {
    if journal.phase != PendingPhase::DeviceManifestPublished {
        bail!("Rockbox projection staging requires DeviceManifestPublished");
    }
    plan.candidate_ownership
        .validate_for_serial(&journal.serial)
        .context("validate enriched playlist ownership")?;
    journal.candidate_playlist_ownership = Some(plan.candidate_ownership);
    journal.pending_rockbox_ops = plan.operations;
    journal.rockbox_projection_plan_version = Some(ROCKBOX_PROJECTION_PLAN_VERSION);
    journal.phase = PendingPhase::RockboxProjectionsPrepared;
    store
        .save(journal)
        .context("persist prepared Rockbox projection operations")
}

pub fn publish_playlist_finalization(
    journal_store: &PendingSessionStore,
    journal: &mut PendingSession,
    ownership_store: &DeviceOwnershipStore,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()> {
    if journal.phase != PendingPhase::PlaylistOwnershipPublished {
        bail!("Rockbox projection publication requires PlaylistOwnershipPublished");
    }
    require_current_plan(journal)?;
    execute_projection_ops(
        journal,
        ownership_store,
        projection_io,
        verified_memberships,
    )?;
    journal.phase = PendingPhase::RockboxProjectionsPublished;
    journal_store
        .save(journal)
        .context("persist published Rockbox projection checkpoint")
}

#[allow(dead_code)]
pub fn recover_playlist_finalization(
    journal_store: &PendingSessionStore,
    journal: &mut PendingSession,
    ownership_store: &DeviceOwnershipStore,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()> {
    publish_playlist_finalization(
        journal_store,
        journal,
        ownership_store,
        projection_io,
        verified_memberships,
    )
}

fn require_current_plan(journal: &PendingSession) -> Result<()> {
    if journal.rockbox_projection_plan_version != Some(ROCKBOX_PROJECTION_PLAN_VERSION) {
        bail!("Rockbox projection journal has no current recorded operation plan");
    }
    Ok(())
}

pub fn execute_projection_ops(
    journal: &PendingSession,
    ownership_store: &DeviceOwnershipStore,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()> {
    require_current_plan(journal)?;
    let settled = ownership_store
        .load_device_read_only()
        .context("load published playlist ownership for projection authorization")?;
    let candidate = journal
        .candidate_playlist_ownership
        .as_ref()
        .context("projection journal has no candidate playlist ownership")?;
    if settled != *candidate {
        bail!("published playlist ownership differs from prepared projection candidate");
    }

    let authorized = validated_authority(&settled, &journal.pending_rockbox_ops)?;
    for (slug, operation) in &journal.pending_rockbox_ops {
        if let Some(desired) = &operation.desired {
            let membership = verified_memberships
                .get(slug)
                .with_context(|| format!("verified membership missing for projection {slug:?}"))?;
            let entry = candidate
                .playlists
                .get(slug)
                .with_context(|| format!("candidate ownership missing projection {slug:?}"))?;
            if entry.apple_playlist_id != membership.apple_playlist_id
                || entry.rockbox.as_ref() != Some(desired)
            {
                bail!("verified playlist {slug:?} differs from prepared projection ownership");
            }
            let bytes = render_verified_paths(membership)
                .with_context(|| format!("render verified projection {slug:?}"))?;
            if blake3::hash(&bytes).to_hex().as_str() != desired.content_hash {
                bail!("verified playlist {slug:?} content differs from prepared projection hash");
            }
            publish_desired(projection_io, desired, &bytes, &authorized)?;
        }
        if let Some(previous) = &operation.previous {
            let desired_name = operation
                .desired
                .as_ref()
                .map(|record| record.relative_filename.as_str());
            if desired_name != Some(previous.relative_filename.as_str()) {
                projection_io.remove_recorded(
                    &previous.relative_filename,
                    &previous.content_hash,
                    &authorized,
                )?;
            }
        }
    }
    Ok(())
}

fn publish_desired(
    io: &dyn ProjectionIo,
    desired: &crate::ipod::playlist_ownership::RockboxProjectionRecord,
    bytes: &[u8],
    authorized: &HashSet<String>,
) -> Result<()> {
    match io.target_state(&desired.relative_filename, authorized)? {
        TargetState::Missing => {
            io.write_durable(&desired.relative_filename, bytes, authorized, false)
        }
        TargetState::RecordedFile => {
            if io.content_matches(
                &desired.relative_filename,
                &desired.content_hash,
                authorized,
            )? {
                Ok(())
            } else {
                bail!(
                    "prepared Rockbox projection target {:?} no longer matches its recorded hash",
                    desired.relative_filename
                )
            }
        }
        TargetState::ForeignFile => bail!(
            "prepared Rockbox projection target {:?} is no longer an authorized regular file",
            desired.relative_filename
        ),
    }
}

fn validated_authority(
    settled: &ManagedPlaylistOwnership,
    operations: &BTreeMap<String, crate::pending_session::PendingRockboxOp>,
) -> Result<HashSet<String>> {
    let mut authorized = HashSet::new();
    for (slug, entry) in &settled.playlists {
        if let Some(record) = &entry.rockbox {
            validate_projection_record(record)
                .with_context(|| format!("validate published projection {slug:?}"))?;
            authorized.insert(record.relative_filename.clone());
        }
    }
    for (slug, operation) in operations {
        for record in [&operation.previous, &operation.desired]
            .into_iter()
            .flatten()
        {
            validate_projection_record(record)
                .with_context(|| format!("validate journal projection {slug:?}"))?;
            authorized.insert(record.relative_filename.clone());
        }
    }
    Ok(authorized)
}

#[cfg(test)]
#[path = "rockbox_publication/tests.rs"]
mod tests;
