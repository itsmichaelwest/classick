use super::{
    ActionPlanSummary, ConfigDelivery, DropSyncDisposition, PlaylistDetailResult,
    SourceAvailabilityState, StoredPlaylist, WireEvent,
};
use anyhow::{bail, Result};
use std::collections::HashSet;

impl WireEvent {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::GlobalConfig { revision, .. } => {
                require_revision(*revision, "global config")?;
            }
            Self::SourceAvailability {
                state, source_root, ..
            } if (*state == SourceAvailabilityState::Available) != source_root.is_some() => {
                bail!("available source requires a root and unavailable source must omit it")
            }
            Self::DeviceInventory { snapshot, .. } => snapshot.validate()?,
            Self::DeviceConfig { config, .. } => config.validate()?,
            Self::ConfigMutationFailed { message, .. } if message.is_empty() => {
                bail!("configuration mutation failure requires a message")
            }
            Self::SyncRejected { message, .. } if message.is_empty() => {
                bail!("sync rejection requires a message")
            }
            Self::History { entries, .. } => {
                for entry in entries {
                    entry.validate()?;
                }
            }
            Self::Library { snapshot, .. } => snapshot.validate()?,
            Self::LibraryScanProgress {
                files_scanned,
                tracks_indexed,
                ..
            } if tracks_indexed > files_scanned => {
                bail!("library scan cannot index more tracks than files scanned")
            }
            Self::LibraryScanFinished {
                success, message, ..
            } if (*success && message.is_some())
                || (!*success && message.as_ref().is_none_or(String::is_empty)) =>
            {
                bail!("library scan result has inconsistent success diagnostics")
            }
            Self::ResolvedTracks { tracks, .. } => super::library::validate_paths(tracks)?,
            Self::DevicePreview { preview, .. }
                if preview
                    .unresolved_subscriptions
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1]) =>
            {
                bail!("unresolved subscriptions must be unique and sorted")
            }
            Self::Playlists {
                revision,
                playlists,
                ..
            } => {
                require_revision(*revision, "playlist collection")?;
                let mut slugs = HashSet::new();
                for playlist in playlists {
                    if !slugs.insert(&playlist.slug) {
                        bail!("playlist collection repeats a slug");
                    }
                    if playlist.name.is_empty()
                        || playlist.error.as_ref().is_some_and(String::is_empty)
                        || (playlist.error.is_some()
                            && (playlist.tracks != 0 || playlist.bytes != 0))
                    {
                        bail!("playlist summary contains inconsistent content");
                    }
                }
                if playlists
                    .windows(2)
                    .any(|pair| pair[0].slug >= pair[1].slug)
                {
                    bail!("playlist collection must be sorted by slug");
                }
            }
            Self::PlaylistDetail {
                revision,
                slug,
                result,
                ..
            } => {
                require_revision(*revision, "playlist detail")?;
                match result {
                    PlaylistDetailResult::Found { playlist } => {
                        playlist.validate()?;
                        if playlist.slug() != slug {
                            bail!("playlist detail slug does not match its content");
                        }
                    }
                    PlaylistDetailResult::Unavailable { message } if message.is_empty() => {
                        bail!("unavailable playlist detail requires a message")
                    }
                    PlaylistDetailResult::Unavailable { .. } => {}
                }
            }
            Self::PlaylistSaved {
                revision, playlist, ..
            } => {
                require_revision(*revision, "saved playlist")?;
                playlist.validate()?;
            }
            Self::DeviceSelectionAdded {
                selection_revision,
                selection,
                delivery,
                sync,
                ..
            } => {
                require_revision(*selection_revision, "device selection")?;
                if selection.schema_version != 1 {
                    bail!("unsupported selection schema");
                }
                super::library::validate_selection_rules(&selection.rules)?;
                if matches!(delivery, ConfigDelivery::PendingDevice { last_failure: Some(message) } if message.is_empty())
                {
                    bail!("pending device delivery failure requires a message");
                }
                if matches!(sync, DropSyncDisposition::Started { .. })
                    && !matches!(delivery, ConfigDelivery::DeviceCommitted)
                {
                    bail!("started sync requires committed device selection delivery");
                }
            }
            Self::PlaylistSelectionAppended {
                slug,
                revision,
                playlist,
                ..
            } => {
                require_revision(*revision, "playlist append")?;
                playlist.validate()?;
                if playlist.slug() != slug || !matches!(playlist, StoredPlaylist::Manual { .. }) {
                    bail!("playlist append requires its matching manual playlist");
                }
            }
            Self::LibraryMutationRejected { code, message, .. }
                if code.is_empty() || message.is_empty() =>
            {
                bail!("library mutation rejection requires a code and message")
            }
            Self::RunHeader {
                source,
                ipod,
                manifest,
                ..
            } if source.is_empty() || ipod.is_empty() || manifest.is_empty() => {
                bail!("run header paths must not be empty")
            }
            Self::SyncSummary { summary, .. } | Self::ReviewRequested { summary, .. } => {
                summary.validate()?
            }
            Self::Prompt {
                message, options, ..
            } if message.is_empty()
                || options.is_empty()
                || options.iter().any(String::is_empty) =>
            {
                bail!("prompt requires a message and non-empty options")
            }
            Self::Form { label, .. } if label.is_empty() => bail!("form label must not be empty"),
            Self::TrackStart {
                current,
                total,
                label,
                ..
            } if *total == 0 || *current == 0 || current > total || label.is_empty() => {
                bail!("track start requires a 1-based position within a non-empty total")
            }
            Self::SyncLog { message, .. }
            | Self::SyncError { message, .. }
            | Self::CommandFailed { message, .. }
                if message.is_empty() =>
            {
                bail!("wire diagnostic message must not be empty")
            }
            Self::SyncFinished {
                skipped_for_space: Some(skipped),
                ..
            } if skipped.albums == 0 || skipped.tracks == 0 || skipped.bytes == 0 => {
                bail!("skipped-for-space summary must describe nonzero skipped content")
            }
            Self::SyncFinished {
                artwork: Some(artwork),
                ..
            } if artwork.embedded > artwork.eligible
                || artwork.failed_sources > artwork.eligible
                || artwork
                    .embedded
                    .checked_add(artwork.failed_sources)
                    .is_none_or(|processed| processed > artwork.eligible) =>
            {
                bail!("artwork summary counts are inconsistent")
            }
            _ => {}
        }
        Ok(())
    }
}

impl ActionPlanSummary {
    fn validate(&self) -> Result<()> {
        let without_removals = self
            .add
            .checked_add(self.modify)
            .and_then(|value| value.checked_add(self.metadata_only))
            .ok_or_else(|| anyhow::anyhow!("action-plan count overflow"))?;
        let with_removals = without_removals
            .checked_add(self.remove)
            .ok_or_else(|| anyhow::anyhow!("action-plan count overflow"))?;
        if self.total_planned != without_removals && self.total_planned != with_removals {
            bail!("action-plan total does not match its component counts");
        }
        Ok(())
    }
}

fn require_revision(revision: u64, kind: &str) -> Result<()> {
    if revision == 0 {
        bail!("{kind} revision must be nonzero");
    }
    Ok(())
}
