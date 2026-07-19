mod journal;
mod transaction;

use crate::daemon::device_registry::DeviceRegistry;
use crate::playlist::PlaylistStore;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeletePlaylistOutcome {
    pub request_id: String,
    pub deleted: bool,
    pub changed_revisions: BTreeMap<String, u64>,
}

pub(crate) fn delete_and_scrub_subscriptions(
    store: &PlaylistStore,
    registry: &mut DeviceRegistry,
    state_root: &Path,
    slug: &str,
    request_id: &str,
) -> Result<DeletePlaylistOutcome> {
    transaction::delete_and_scrub_subscriptions(store, registry, state_root, slug, request_id)
}

pub(crate) fn recover_pending_playlist_mutations(
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    transaction::recover_pending_playlist_mutations(registry, state_root)
}
