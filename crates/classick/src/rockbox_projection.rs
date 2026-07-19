use crate::ipod::playlist_ownership::{
    ManagedPlaylistKind, ManagedPlaylistOwnership, RockboxProjectionRecord,
    VerifiedPlaylistMembership,
};
use crate::pending_session::PendingRockboxOp;
use crate::rockbox_playlist::{
    candidate_filename, render_verified_paths, validate_projection_record,
};
use crate::rockbox_projection_fs::{ProjectionIo, TargetState};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};

type ProjectionAuthority = (HashSet<String>, BTreeMap<String, BTreeSet<String>>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredVerifiedPlaylist {
    pub display_name: String,
    pub membership: VerifiedPlaylistMembership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlan {
    pub candidate_ownership: ManagedPlaylistOwnership,
    pub operations: BTreeMap<String, PendingRockboxOp>,
}

pub fn plan_projection(
    serial: &str,
    enabled: bool,
    desired: &[DesiredVerifiedPlaylist],
    settled: &ManagedPlaylistOwnership,
    candidate: &ManagedPlaylistOwnership,
    io: &dyn ProjectionIo,
) -> Result<ProjectionPlan> {
    settled
        .validate_for_serial(serial)
        .context("validate settled playlist ownership for projection planning")?;
    candidate
        .validate_for_serial(serial)
        .context("validate candidate playlist ownership for projection planning")?;

    let (authorized, recorded_owners) = validated_authority(settled, candidate)?;
    let prepared = validate_and_render_desired(desired, candidate, enabled)?;
    let desired_slugs = prepared.keys().cloned().collect::<HashSet<_>>();
    let mut enriched = candidate.clone();
    let mut operations = BTreeMap::new();

    if !enabled {
        for entry in enriched.playlists.values_mut() {
            entry.rockbox = None;
        }
        stage_recorded_deletes(settled, &mut operations);
        return Ok(ProjectionPlan {
            candidate_ownership: enriched,
            operations,
        });
    }

    let mut selected_names = BTreeSet::new();
    for (slug, item) in prepared {
        let previous = settled
            .playlists
            .get(&slug)
            .and_then(|entry| entry.rockbox.clone());
        let (desired_record, target_state) = choose_record(
            &slug,
            &item.display_name,
            &item.content_hash,
            &authorized,
            &recorded_owners,
            &selected_names,
            io,
        )?;
        selected_names.insert(desired_record.relative_filename.clone());

        enriched
            .playlists
            .get_mut(&slug)
            .expect("validated candidate entry")
            .rockbox = Some(desired_record.clone());

        let already_settled = previous.as_ref() == Some(&desired_record)
            && target_state == TargetState::RecordedFile
            && io
                .content_matches(
                    &desired_record.relative_filename,
                    &desired_record.content_hash,
                    &authorized,
                )
                .with_context(|| {
                    format!(
                        "verify settled projection content for {:?}",
                        desired_record.relative_filename
                    )
                })?;
        if !already_settled {
            operations.insert(
                slug,
                PendingRockboxOp {
                    previous,
                    desired: Some(desired_record),
                },
            );
        }
    }

    for (slug, entry) in &settled.playlists {
        if !desired_slugs.contains(slug) {
            if let Some(previous) = entry.rockbox.clone() {
                operations.insert(
                    slug.clone(),
                    PendingRockboxOp {
                        previous: Some(previous),
                        desired: None,
                    },
                );
            }
        }
    }

    Ok(ProjectionPlan {
        candidate_ownership: enriched,
        operations,
    })
}

struct PreparedDesired {
    display_name: String,
    content_hash: String,
}

fn validate_and_render_desired(
    desired: &[DesiredVerifiedPlaylist],
    candidate: &ManagedPlaylistOwnership,
    enabled: bool,
) -> Result<BTreeMap<String, PreparedDesired>> {
    let mut prepared = BTreeMap::new();
    for item in desired {
        let slug = item.membership.slug.as_str();
        let Some(entry) = candidate.playlists.get(slug) else {
            bail!("verified playlist {slug:?} is absent from candidate ownership");
        };
        if entry.apple_playlist_id != item.membership.apple_playlist_id
            || entry.expected_kind != ManagedPlaylistKind::Normal
        {
            bail!("verified playlist {slug:?} disagrees with candidate Apple ownership");
        }
        let bytes = render_verified_paths(&item.membership)
            .with_context(|| format!("render verified membership for playlist {slug:?}"))?;
        if prepared
            .insert(
                slug.to_string(),
                PreparedDesired {
                    display_name: item.display_name.clone(),
                    content_hash: blake3::hash(&bytes).to_hex().to_string(),
                },
            )
            .is_some()
        {
            bail!("duplicate verified playlist slug {slug:?}");
        }
    }

    if enabled
        && (prepared.len() != candidate.playlists.len()
            || candidate
                .playlists
                .keys()
                .any(|slug| !prepared.contains_key(slug)))
    {
        bail!("verified memberships do not exactly cover candidate playlist ownership");
    }
    Ok(prepared)
}

fn validated_authority(
    settled: &ManagedPlaylistOwnership,
    candidate: &ManagedPlaylistOwnership,
) -> Result<ProjectionAuthority> {
    let mut authorized = HashSet::new();
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ownership in [settled, candidate] {
        for (slug, entry) in &ownership.playlists {
            let Some(record) = &entry.rockbox else {
                continue;
            };
            validate_projection_record(record)
                .with_context(|| format!("validate recorded projection for playlist {slug:?}"))?;
            authorized.insert(record.relative_filename.clone());
            owners
                .entry(record.relative_filename.clone())
                .or_default()
                .insert(slug.clone());
        }
    }
    if let Some((name, slugs)) = owners.iter().find(|(_, slugs)| slugs.len() > 1) {
        bail!("recorded projection filename {name:?} is shared by playlists {slugs:?}");
    }
    Ok((authorized, owners))
}

fn choose_record(
    slug: &str,
    display_name: &str,
    content_hash: &str,
    authorized: &HashSet<String>,
    recorded_owners: &BTreeMap<String, BTreeSet<String>>,
    selected_names: &BTreeSet<String>,
    io: &dyn ProjectionIo,
) -> Result<(RockboxProjectionRecord, TargetState)> {
    for collision_index in 0..256 {
        let relative_filename = candidate_filename(display_name, slug, collision_index);
        if selected_names.contains(&relative_filename)
            || recorded_owners
                .get(&relative_filename)
                .is_some_and(|owners| !owners.contains(slug))
        {
            continue;
        }

        let state = io
            .target_state(&relative_filename, authorized)
            .with_context(|| {
                format!("inspect projection collision candidate {relative_filename:?}")
            })?;
        let owned_by_slug = recorded_owners
            .get(&relative_filename)
            .is_some_and(|owners| owners.contains(slug));
        if state == TargetState::ForeignFile
            || (state == TargetState::RecordedFile && !owned_by_slug)
        {
            continue;
        }
        return Ok((
            RockboxProjectionRecord {
                relative_filename,
                content_hash: content_hash.to_string(),
            },
            state,
        ));
    }
    bail!("could not select a collision-free Rockbox filename for playlist {slug:?}")
}

fn stage_recorded_deletes(
    settled: &ManagedPlaylistOwnership,
    operations: &mut BTreeMap<String, PendingRockboxOp>,
) {
    for (slug, entry) in &settled.playlists {
        if let Some(previous) = entry.rockbox.clone() {
            operations.insert(
                slug.clone(),
                PendingRockboxOp {
                    previous: Some(previous),
                    desired: None,
                },
            );
        }
    }
}

#[cfg(test)]
#[path = "rockbox_projection/tests.rs"]
mod tests;
