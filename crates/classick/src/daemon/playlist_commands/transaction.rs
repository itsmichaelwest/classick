use super::journal::{
    cleanup, file_hash, hash, mutation_root, playlist_path, relative_to, replace, require_hash,
    save_journal, subscription_path, validate_component, validate_journal, MutationJournal,
    MutationPhase, PlaylistMutation, SubscriptionMutation, JOURNAL_VERSION,
};
use super::DeletePlaylistOutcome;
use crate::atomic_file::AtomicFileWriter;
use crate::daemon::device_registry::DeviceRegistry;
use crate::device_config::Subscriptions;
use crate::playlist::{Playlist, PlaylistStore};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn delete_and_scrub_subscriptions(
    store: &PlaylistStore,
    registry: &mut DeviceRegistry,
    state_root: &Path,
    slug: &str,
    request_id: &str,
) -> Result<DeletePlaylistOutcome> {
    validate_component("playlist slug", slug)?;
    validate_component("request id", request_id)?;

    let Some(playlist) = store
        .load(slug)
        .with_context(|| format!("load playlist {slug:?} before deletion"))?
    else {
        return Ok(DeletePlaylistOutcome {
            request_id: request_id.to_string(),
            deleted: false,
            changed_revisions: BTreeMap::new(),
        });
    };

    let playlist_live = playlist_path(state_root, &playlist);
    let stale_other_kind = match &playlist {
        Playlist::Manual(_) => state_root
            .join("playlists")
            .join(format!("{slug}.rules.json")),
        Playlist::Smart(_) => state_root.join("playlists").join(format!("{slug}.m3u8")),
    };
    if stale_other_kind.exists() {
        bail!("playlist {slug:?} exists as both manual and smart");
    }
    let playlist_bytes = std::fs::read(&playlist_live)
        .with_context(|| format!("read playlist {}", playlist_live.display()))?;
    let mutation_root = mutation_root(state_root);
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    let journal_path = mutation_root.join(format!("{request_id}.json"));
    if journal_path.exists() || stage_root.exists() {
        bail!("playlist mutation request id {request_id:?} already exists");
    }

    let playlist_mutation = PlaylistMutation {
        live_path: relative_to(state_root, &playlist_live)?,
        staged_original_path: relative_to(state_root, &stage_root.join("playlist.original"))?,
        original_hash: hash(&playlist_bytes),
    };
    let writer = AtomicFileWriter::new();
    let mut prepared = Vec::new();
    for record in registry.records() {
        let live = subscription_path(state_root, &record.serial);
        let bytes = match std::fs::read(&live) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read subscriptions {}", live.display()))
            }
        };
        let mut parsed: Subscriptions = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse subscriptions {}", live.display()))?;
        if !parsed.playlists.iter().any(|candidate| candidate == slug) {
            continue;
        }
        parsed.playlists.retain(|candidate| candidate != slug);
        let target = serde_json::to_vec_pretty(&parsed)
            .with_context(|| format!("encode subscriptions for {:?}", record.serial))?;
        let index = prepared.len();
        let original_stage = stage_root.join(format!("subscription-{index}.original"));
        let target_stage = stage_root.join(format!("subscription-{index}.target"));
        let target_revision = record
            .subscriptions_revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("subscriptions revision overflow for {:?}", record.serial))?;
        prepared.push((
            SubscriptionMutation {
                serial: record.serial,
                live_path: relative_to(state_root, &live)?,
                staged_original_path: relative_to(state_root, &original_stage)?,
                staged_target_path: relative_to(state_root, &target_stage)?,
                original_hash: hash(&bytes),
                target_hash: hash(&target),
                original_revision: record.subscriptions_revision,
                target_revision,
            },
            bytes,
            target,
        ));
    }
    for (mutation, original, target) in &prepared {
        writer.write(&state_root.join(&mutation.staged_original_path), original)?;
        writer.write(&state_root.join(&mutation.staged_target_path), target)?;
    }

    let mut journal = MutationJournal {
        version: JOURNAL_VERSION,
        request_id: request_id.to_string(),
        slug: slug.to_string(),
        phase: MutationPhase::Prepared,
        playlist: playlist_mutation,
        subscriptions: prepared
            .into_iter()
            .map(|(mutation, _, _)| mutation)
            .collect(),
    };
    save_journal(&writer, &journal_path, &journal)?;
    journal.phase = MutationPhase::Publishing;
    save_journal(&writer, &journal_path, &journal)?;

    if let Err(error) = publish(&journal, registry, state_root) {
        if let Err(rollback_error) = restore(&journal, registry, state_root) {
            return Err(error).context(format!(
                "playlist deletion failed and rollback remains pending: {rollback_error:#}"
            ));
        }
        cleanup(&journal_path, &stage_root, &mutation_root)?;
        return Err(error);
    }

    let changed_revisions = journal
        .subscriptions
        .iter()
        .map(|mutation| (mutation.serial.clone(), mutation.target_revision))
        .collect();
    cleanup(&journal_path, &stage_root, &mutation_root)?;
    Ok(DeletePlaylistOutcome {
        request_id: request_id.to_string(),
        deleted: true,
        changed_revisions,
    })
}

pub(super) fn recover_pending_playlist_mutations(
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    let root = mutation_root(state_root);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read playlist mutations {}", root.display()))
        }
    };
    let mut journals = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .with_context(|| format!("read entry in {}", root.display()))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    journals.sort();
    for journal_path in journals {
        let bytes = std::fs::read(&journal_path)
            .with_context(|| format!("read playlist mutation {}", journal_path.display()))?;
        let journal: MutationJournal = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode playlist mutation {}", journal_path.display()))?;
        validate_journal(&journal, &journal_path, registry, state_root)?;
        match journal.phase {
            MutationPhase::Prepared => restore(&journal, registry, state_root)?,
            MutationPhase::Publishing => publish(&journal, registry, state_root)?,
        }
        let stage_root = state_root.join(
            journal
                .playlist
                .staged_original_path
                .parent()
                .context("playlist staged path has no parent")?,
        );
        cleanup(&journal_path, &stage_root, &root)?;
    }
    Ok(())
}

fn publish(
    journal: &MutationJournal,
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    for mutation in &journal.subscriptions {
        let live = state_root.join(&mutation.live_path);
        match file_hash(&live)? {
            Some(actual) if actual == mutation.target_hash => {}
            Some(actual) if actual == mutation.original_hash => {
                let target = state_root.join(&mutation.staged_target_path);
                require_hash(&target, &mutation.target_hash)?;
                replace(&target, &live).with_context(|| {
                    format!(
                        "publish subscriptions {} -> {}",
                        target.display(),
                        live.display()
                    )
                })?;
            }
            actual => bail!(
                "subscriptions {} have unexpected hash {:?}",
                live.display(),
                actual
            ),
        }
    }

    let playlist_live = state_root.join(&journal.playlist.live_path);
    let playlist_stage = state_root.join(&journal.playlist.staged_original_path);
    match file_hash(&playlist_live)? {
        Some(actual) if actual == journal.playlist.original_hash => {
            if playlist_stage.exists() {
                require_hash(&playlist_stage, &journal.playlist.original_hash)?;
                std::fs::remove_file(&playlist_live).with_context(|| {
                    format!("remove duplicate playlist {}", playlist_live.display())
                })?;
            } else {
                std::fs::rename(&playlist_live, &playlist_stage).with_context(|| {
                    format!(
                        "stage deleted playlist {} -> {}",
                        playlist_live.display(),
                        playlist_stage.display()
                    )
                })?;
            }
        }
        None => require_hash(&playlist_stage, &journal.playlist.original_hash)?,
        actual => bail!(
            "playlist {} has unexpected hash {:?}",
            playlist_live.display(),
            actual
        ),
    }

    for mutation in &journal.subscriptions {
        let current = registry
            .record(&mutation.serial)
            .with_context(|| {
                format!(
                    "missing device {:?} during playlist mutation",
                    mutation.serial
                )
            })?
            .subscriptions_revision;
        if current == mutation.target_revision {
            continue;
        }
        if current != mutation.original_revision {
            bail!(
                "device {:?} subscriptions revision is {}, expected {} or {}",
                mutation.serial,
                current,
                mutation.original_revision,
                mutation.target_revision
            );
        }
        registry.advance_config_revisions(&mutation.serial, false, false, true)?;
    }
    Ok(())
}

fn restore(journal: &MutationJournal, registry: &DeviceRegistry, state_root: &Path) -> Result<()> {
    for mutation in &journal.subscriptions {
        let current_revision = registry
            .record(&mutation.serial)
            .with_context(|| format!("missing device {:?} during rollback", mutation.serial))?
            .subscriptions_revision;
        if current_revision != mutation.original_revision {
            bail!(
                "cannot restore device {:?} at subscriptions revision {} (expected {})",
                mutation.serial,
                current_revision,
                mutation.original_revision
            );
        }
        let live = state_root.join(&mutation.live_path);
        match file_hash(&live)? {
            Some(actual) if actual == mutation.original_hash => {}
            Some(actual) if actual == mutation.target_hash => {
                let original = state_root.join(&mutation.staged_original_path);
                require_hash(&original, &mutation.original_hash)?;
                replace(&original, &live).with_context(|| {
                    format!(
                        "restore subscriptions {} -> {}",
                        original.display(),
                        live.display()
                    )
                })?;
            }
            actual => bail!(
                "cannot restore subscriptions {} with hash {:?}",
                live.display(),
                actual
            ),
        }
    }
    let live = state_root.join(&journal.playlist.live_path);
    let staged = state_root.join(&journal.playlist.staged_original_path);
    match file_hash(&live)? {
        Some(actual) if actual == journal.playlist.original_hash => {}
        None => {
            require_hash(&staged, &journal.playlist.original_hash)?;
            std::fs::rename(&staged, &live).with_context(|| {
                format!(
                    "restore playlist {} -> {}",
                    staged.display(),
                    live.display()
                )
            })?;
        }
        actual => bail!(
            "cannot restore playlist {} with hash {:?}",
            live.display(),
            actual
        ),
    }
    Ok(())
}
