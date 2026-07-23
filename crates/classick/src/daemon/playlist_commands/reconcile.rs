use crate::daemon::device_config_transaction::{self, ConfigComponentKind, ConfigComponentUpdate};
use crate::daemon::device_registry::DeviceRegistry;
use crate::device_config::Subscriptions;
use crate::playlist::PlaylistStore;
use crate::portable::outbox::PendingMutation;
use crate::portable::profile::{MutationId, PlaylistSlug, SubscriptionsValue};
use crate::portable::state_store::PortableStateStore;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn reconcile_missing_subscriptions(
    store: &PlaylistStore,
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut changed = BTreeMap::new();
    for record in registry.records() {
        let mut removed = reconcile_portable_subscriptions(store, state_root, &record.serial)?;
        let path = crate::device_state::device_subscriptions_path_in(state_root, &record.serial)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !removed.is_empty() {
                    changed.insert(record.serial, removed);
                }
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read subscriptions {}", path.display()))
            }
        };
        let subscriptions: Subscriptions = match serde_json::from_slice(&bytes) {
            Ok(subscriptions) => subscriptions,
            Err(error) => {
                tracing::warn!(
                    serial = record.serial,
                    "daemon: cannot reconcile invalid subscriptions at {}: {error}",
                    path.display()
                );
                if !removed.is_empty() {
                    changed.insert(record.serial, removed);
                }
                continue;
            }
        };
        let mut retained = Vec::with_capacity(subscriptions.playlists.len());
        let mut missing = Vec::new();
        for slug in subscriptions.playlists {
            if playlist_is_missing(store, &record.serial, &slug) {
                missing.push(slug);
            } else {
                retained.push(slug);
            }
        }
        if missing.is_empty() {
            if !removed.is_empty() {
                changed.insert(record.serial, removed);
            }
            continue;
        }
        let target_contents = serde_json::to_vec_pretty(&Subscriptions {
            version: subscriptions.version,
            playlists: retained,
        })
        .context("encode reconciled subscriptions")?;
        let outcome = device_config_transaction::commit(
            registry,
            state_root,
            &record.serial,
            &format!("startup-reconcile-missing-subscriptions-{}", record.serial),
            vec![ConfigComponentUpdate {
                kind: ConfigComponentKind::Subscriptions,
                live_path: path,
                target_contents,
                failure_message: "could not reconcile missing playlist subscriptions",
            }],
        )?;
        if let Some(failure) = outcome.component_failure {
            bail!("{failure} for device {:?}", record.serial);
        }
        tracing::info!(
            serial = record.serial,
            removed = ?missing,
            "daemon: removed subscriptions to missing playlists"
        );
        for slug in missing {
            if !removed.contains(&slug) {
                removed.push(slug);
            }
        }
        changed.insert(record.serial, removed);
    }
    Ok(changed)
}

fn reconcile_portable_subscriptions(
    playlist_store: &PlaylistStore,
    state_root: &Path,
    serial: &str,
) -> Result<Vec<String>> {
    let device_id = crate::device::DeviceId::parse(serial)?;
    let state_store = PortableStateStore::new(state_root);
    if !state_store.is_initialized(&device_id) {
        return Ok(Vec::new());
    }
    let state = state_store.load(&device_id)?;
    let snapshot = match crate::portable::coordinator::config_snapshot(&state, None) {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(Vec::new()),
    };
    let mut retained = Vec::with_capacity(snapshot.subscriptions.value.playlists.len());
    let mut missing = Vec::new();
    for slug in snapshot.subscriptions.value.playlists {
        if playlist_is_missing(playlist_store, serial, slug.as_str()) {
            missing.push(slug.to_string());
        } else {
            retained.push(slug);
        }
    }
    if missing.is_empty() {
        return Ok(missing);
    }
    let desired = SubscriptionsValue {
        schema_version: snapshot.subscriptions.value.schema_version,
        playlists: retained,
    };
    let imported_revision = state
        .cache
        .as_ref()
        .and_then(|cache| cache.last_imported_profile.as_ref())
        .map(|profile| profile.subscriptions.revision)
        .unwrap_or(0);
    let mutation = PendingMutation::subscriptions(
        reconciliation_mutation_id(&device_id, &snapshot.subscriptions.mutation_id, &desired)?,
        device_id,
        desired,
        imported_revision,
    )?;
    state_store.accept_mutation(&mutation)?;
    Ok(missing)
}

fn playlist_is_missing(store: &PlaylistStore, serial: &str, slug: &str) -> bool {
    if PlaylistSlug::parse(slug).is_err() {
        return true;
    }
    match store.load(slug) {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(error) => {
            tracing::warn!(
                serial,
                playlist = slug,
                "daemon: cannot verify subscribed playlist; preserving it: {error:#}"
            );
            false
        }
    }
}

fn reconciliation_mutation_id(
    device_id: &crate::device::DeviceId,
    basis_mutation_id: &MutationId,
    desired: &SubscriptionsValue,
) -> Result<MutationId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"classick:reconcile-missing-subscriptions:v1\0");
    hasher.update(device_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(basis_mutation_id.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(&serde_json::to_vec(desired)?);
    let hex = hasher.finalize().to_hex();
    MutationId::parse(&format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}
