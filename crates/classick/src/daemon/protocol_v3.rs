use crate::device::{DeviceId, DeviceReadiness, ObservationId};
use crate::ipc_daemon::DaemonEvent;
use crate::ipc_device::{DevicePhaseLabel, DeviceSnapshot};
use crate::portable::device_store::{read_profile, OwnedDeviceProfile};
use crate::portable::profile::{MutationId, PlaylistSlug};
use crate::portable_path::PortablePath;
use crate::wire::{
    ConfigDelivery, DeliveredComponent, DeviceConfigSnapshot, DevicePhase,
    IdentifiedDeviceSnapshot, ProfileStatus, RequestId, SessionId, StorageFreshness,
    StorageSnapshot, WireEvent,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub(crate) fn event_to_wire(event: &DaemonEvent) -> Result<Option<WireEvent>> {
    match event {
        DaemonEvent::Protocol3(event) => Ok(Some(event.clone())),
        DaemonEvent::Hello { .. }
        | DaemonEvent::StatusUpdate { .. }
        | DaemonEvent::DeviceConnected { .. }
        | DaemonEvent::DeviceDisconnected { .. }
        | DaemonEvent::SelectionUpdate { .. } => Ok(None),
        DaemonEvent::ConfigUpdate {
            source,
            daemon,
            config_revision,
            acknowledged_request_id,
            ..
        } => {
            let settings = daemon.clone().unwrap_or_default();
            Ok(Some(WireEvent::GlobalConfig {
                request_id: optional_request(acknowledged_request_id.as_deref())?,
                revision: (*config_revision).max(1),
                source_root: source
                    .as_deref()
                    .map(crate::wire::SourceRoot::parse)
                    .transpose()?,
                settings: crate::wire::GlobalSettings {
                    first_sync_mode: map_sync_mode(settings.first_sync_mode),
                    subsequent_sync_mode: map_sync_mode(settings.subsequent_sync_mode),
                    schedule_minutes: settings.schedule_minutes,
                    notify_on: map_notify(settings.notify_on),
                    drop_sync_behavior: map_drop_behavior(settings.drop_sync_behavior),
                },
            }))
        }
        DaemonEvent::HistoryUpdate {
            entries,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::History {
            request_id: acknowledged_request_id
                .as_deref()
                .map(request)
                .transpose()?,
            entries: entries
                .iter()
                .map(history_entry)
                .collect::<Result<Vec<_>>>()?,
        })),
        DaemonEvent::SyncRejected {
            reason,
            serial,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::SyncRejected {
            device_id: device_id(serial)?,
            request_id: request(acknowledged_request_id)?,
            operation: crate::wire::SyncOperation::Sync,
            reason: match reason {
                crate::ipc_daemon::SyncRejectReason::AlreadySyncing => {
                    crate::wire::SyncRejectReason::AlreadyRunning
                }
                crate::ipc_daemon::SyncRejectReason::NoIpod => {
                    crate::wire::SyncRejectReason::DeviceDisconnected
                }
                crate::ipc_daemon::SyncRejectReason::NotConfigured => {
                    crate::wire::SyncRejectReason::NotAdopted
                }
                crate::ipc_daemon::SyncRejectReason::TooManyFailures => {
                    crate::wire::SyncRejectReason::RecoveryRequired
                }
            },
            message: format!("{reason:?}"),
        })),
        DaemonEvent::CommandFailed {
            acknowledged_request_id,
            error,
        } => Ok(Some(WireEvent::CommandFailed {
            request_id: request(acknowledged_request_id)?,
            message: error.clone(),
        })),
        DaemonEvent::DeviceSelectionAdded {
            acknowledged_request_id,
            mutation_id,
            session_id,
            serial,
            matched_tracks,
            missing_tracks,
            selection_changed,
            selection_revision,
            selection,
            delivery,
        } => Ok(Some(WireEvent::DeviceSelectionAdded {
            device_id: device_id(serial)?,
            request_id: request(acknowledged_request_id)?,
            mutation_id: mutation_id.clone().unwrap_or(synthetic_mutation_id(
                serial,
                "selection",
                *selection_revision,
            )?),
            matched_tracks: *matched_tracks as u64,
            missing_tracks: *missing_tracks as u64,
            selection_changed: *selection_changed,
            selection_revision: *selection_revision,
            selection: profile_selection(selection)?,
            delivery: ConfigDelivery::PendingDevice { last_failure: None },
            sync: match (session_id, delivery) {
                (Some(session_id), _) => crate::wire::DropSyncDisposition::Started {
                    session_id: SessionId::new(*session_id)?,
                },
                (None, crate::ipc_daemon::DropDelivery::AlreadyPresent) => {
                    crate::wire::DropSyncDisposition::AlreadyPresent
                }
                (
                    None,
                    crate::ipc_daemon::DropDelivery::AddedAndSyncing
                    | crate::ipc_daemon::DropDelivery::AddedForNextSync,
                ) => crate::wire::DropSyncDisposition::NextSync,
            },
        })),
        DaemonEvent::PlaylistSelectionAppended {
            acknowledged_request_id,
            slug,
            appended_tracks,
            playlist_revision,
            playlist,
        } => Ok(Some(WireEvent::PlaylistSelectionAppended {
            request_id: request(acknowledged_request_id)?,
            slug: PlaylistSlug::parse(slug)?,
            appended_tracks: *appended_tracks as u64,
            revision: *playlist_revision,
            playlist: crate::wire::StoredPlaylist::Manual {
                slug: PlaylistSlug::parse(&playlist.slug)?,
                name: playlist.name.clone(),
                tracks: library_paths(&playlist.tracks)?,
            },
        })),
        DaemonEvent::LibraryMutationRejected {
            acknowledged_request_id,
            target,
            code,
            message,
        } => {
            let target = match target {
                crate::daemon::library_mutations::MutationTarget::DeviceSelection { serial } => {
                    crate::wire::LibraryMutationTarget::DeviceSelection {
                        device_id: device_id(serial)?,
                    }
                }
                crate::daemon::library_mutations::MutationTarget::ManualPlaylist { slug } => {
                    crate::wire::LibraryMutationTarget::ManualPlaylist {
                        slug: PlaylistSlug::parse(slug)?,
                    }
                }
            };
            Ok(Some(WireEvent::LibraryMutationRejected {
                request_id: request(acknowledged_request_id)?,
                target,
                code: code.clone(),
                message: message.clone(),
            }))
        }
        DaemonEvent::SyncEvent {
            wire_event: Some(event),
            ..
        } => Ok(Some(event.clone())),
        DaemonEvent::SyncEvent {
            wire_event: None, ..
        } => Ok(None),
        DaemonEvent::DeviceInventorySnapshot(snapshot) => Ok(Some(WireEvent::DeviceInventory {
            request_id: None,
            snapshot: crate::wire::DeviceInventorySnapshot {
                revision: snapshot.revision.max(1),
                devices: snapshot
                    .devices
                    .iter()
                    .map(inventory_device)
                    .collect::<Result<Vec<_>>>()?,
                unidentified: Vec::new(),
            },
        })),
        DaemonEvent::LibraryUpdate {
            source_root,
            scanned_at_unix_secs,
            artists,
            genres,
            total_tracks,
            total_bytes,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::Library {
            request_id: optional_request(acknowledged_request_id.as_deref())?,
            snapshot: crate::wire::LibrarySnapshot {
                source_root: source_root
                    .as_deref()
                    .map(crate::wire::SourceRoot::parse)
                    .transpose()?,
                scanned_at_unix_secs: *scanned_at_unix_secs,
                artists: artists
                    .iter()
                    .map(|artist| crate::wire::LibraryArtist {
                        name: artist.name.clone(),
                        albums: artist
                            .albums
                            .iter()
                            .map(|album| crate::wire::LibraryAlbum {
                                name: album.name.clone(),
                                genre: album.genre.clone(),
                                tracks: album.tracks as u64,
                                bytes: album.bytes,
                            })
                            .collect(),
                    })
                    .collect(),
                genres: genres
                    .iter()
                    .map(|genre| crate::wire::LibraryGenre {
                        name: genre.name.clone(),
                        tracks: genre.tracks as u64,
                        bytes: genre.bytes,
                    })
                    .collect(),
                total_tracks: *total_tracks as u64,
                total_bytes: *total_bytes,
            },
        })),
        DaemonEvent::SelectionPreview {
            selected_tracks,
            selected_bytes,
            adds,
            removes,
            serial,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::SelectionPreview {
            device_id: device_id(serial)?,
            request_id: request(acknowledged_request_id)?,
            preview: crate::wire::SelectionPreview {
                selected_tracks: *selected_tracks as u64,
                selected_bytes: *selected_bytes,
                adds: *adds as u64,
                removes: *removes as u64,
            },
        })),
        DaemonEvent::PlaylistsUpdate {
            playlists,
            playlist_revision,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::Playlists {
            request_id: optional_request(acknowledged_request_id.as_deref())?,
            revision: (*playlist_revision).max(1),
            playlists: playlists
                .iter()
                .map(|playlist| {
                    Ok(crate::wire::PlaylistSummary {
                        slug: PlaylistSlug::parse(&playlist.slug)?,
                        name: playlist.name.clone(),
                        kind: match playlist.kind {
                            crate::ipc_daemon::PlaylistKind::Manual => {
                                crate::wire::PlaylistKind::Manual
                            }
                            crate::ipc_daemon::PlaylistKind::Smart => {
                                crate::wire::PlaylistKind::Smart
                            }
                        },
                        tracks: playlist.tracks as u64,
                        bytes: playlist.bytes,
                        error: playlist.error.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        })),
        DaemonEvent::PlaylistDetail {
            slug,
            name,
            kind,
            tracks,
            rules,
            error,
            playlist_revision,
            acknowledged_request_id,
        } => {
            let slug = PlaylistSlug::parse(slug)?;
            let result = match (name, kind, tracks, rules, error) {
                (
                    Some(name),
                    Some(crate::ipc_daemon::PlaylistKind::Manual),
                    Some(tracks),
                    None,
                    None,
                ) => crate::wire::PlaylistDetailResult::Found {
                    playlist: crate::wire::StoredPlaylist::Manual {
                        slug: slug.clone(),
                        name: name.clone(),
                        tracks: library_paths(tracks)?,
                    },
                },
                (
                    Some(name),
                    Some(crate::ipc_daemon::PlaylistKind::Smart),
                    None,
                    Some(rules),
                    None,
                ) => crate::wire::PlaylistDetailResult::Found {
                    playlist: crate::wire::StoredPlaylist::Smart {
                        slug: slug.clone(),
                        name: name.clone(),
                        rules: serde_json::from_value(serde_json::to_value(rules)?)
                            .context("translate smart playlist rules")?,
                    },
                },
                _ => crate::wire::PlaylistDetailResult::Unavailable {
                    message: error
                        .clone()
                        .unwrap_or_else(|| "playlist unavailable".to_string()),
                },
            };
            Ok(Some(WireEvent::PlaylistDetail {
                request_id: request(acknowledged_request_id)?,
                revision: (*playlist_revision).max(1),
                slug,
                result,
            }))
        }
        DaemonEvent::DeviceConfigUpdate {
            serial,
            selection,
            subscriptions,
            settings,
            selection_revision,
            settings_revision,
            subscriptions_revision,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::DeviceConfig {
            request_id: optional_request(acknowledged_request_id.as_deref())?,
            config: legacy_device_config(
                serial,
                selection,
                subscriptions,
                settings,
                *selection_revision,
                *settings_revision,
                *subscriptions_revision,
            )?,
        })),
        DaemonEvent::DevicePreview {
            serial,
            selected_tracks,
            selected_bytes,
            playlist_extra_tracks,
            playlist_extra_bytes,
            projected_free_bytes,
            unresolved_subscriptions,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::DevicePreview {
            device_id: device_id(serial)?,
            request_id: request(acknowledged_request_id)?,
            preview: crate::wire::DevicePreview {
                selected_tracks: *selected_tracks as u64,
                selected_bytes: *selected_bytes,
                playlist_extra_tracks: *playlist_extra_tracks as u64,
                playlist_extra_bytes: *playlist_extra_bytes,
                projected_free_bytes: *projected_free_bytes,
                unresolved_subscriptions: unresolved_subscriptions
                    .iter()
                    .map(|slug| PlaylistSlug::parse(slug))
                    .collect::<Result<Vec<_>>>()?,
            },
        })),
        DaemonEvent::ResolvedTracks {
            tracks,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::ResolvedTracks {
            request_id: request(acknowledged_request_id)?,
            tracks: library_paths(tracks)?,
        })),
        DaemonEvent::SourceAvailability {
            state,
            source_root,
            acknowledged_request_id,
        } => Ok(Some(WireEvent::SourceAvailability {
            request_id: optional_request(acknowledged_request_id.as_deref())?,
            state: match state {
                crate::ipc_daemon::SourceAvailabilityState::Available => {
                    crate::wire::SourceAvailabilityState::Available
                }
                crate::ipc_daemon::SourceAvailabilityState::Remounting => {
                    crate::wire::SourceAvailabilityState::Remounting
                }
                crate::ipc_daemon::SourceAvailabilityState::AuthRequired => {
                    crate::wire::SourceAvailabilityState::AuthRequired
                }
                crate::ipc_daemon::SourceAvailabilityState::Unavailable => {
                    crate::wire::SourceAvailabilityState::Unavailable
                }
            },
            source_root: source_root
                .as_deref()
                .map(crate::wire::SourceRoot::parse)
                .transpose()?,
        })),
    }
}

fn legacy_device_config(
    serial: &str,
    selection: &crate::ipc_daemon::SelectionPayload,
    subscriptions: &crate::ipc_daemon::SubscriptionsPayload,
    settings: &crate::ipc_daemon::DeviceSettingsPayload,
    selection_revision: u64,
    settings_revision: u64,
    subscriptions_revision: u64,
) -> Result<DeviceConfigSnapshot> {
    Ok(DeviceConfigSnapshot {
        device_id: device_id(serial)?,
        selection: DeliveredComponent {
            revision: selection_revision.max(1),
            mutation_id: synthetic_mutation_id(serial, "selection", selection_revision)?,
            value: profile_selection(selection)?,
            delivery: ConfigDelivery::PendingDevice { last_failure: None },
        },
        settings: DeliveredComponent {
            revision: settings_revision.max(1),
            mutation_id: synthetic_mutation_id(serial, "settings", settings_revision)?,
            value: crate::portable::profile::SettingsValue {
                schema_version: 1,
                auto_sync: settings.auto_sync,
                rockbox_compat: settings.rockbox_compat,
                transcode_profile: settings.transcode_profile,
            },
            delivery: ConfigDelivery::PendingDevice { last_failure: None },
        },
        subscriptions: DeliveredComponent {
            revision: subscriptions_revision.max(1),
            mutation_id: synthetic_mutation_id(serial, "subscriptions", subscriptions_revision)?,
            value: crate::portable::profile::SubscriptionsValue {
                schema_version: 1,
                playlists: subscriptions
                    .playlists
                    .iter()
                    .map(|slug| PlaylistSlug::parse(slug))
                    .collect::<Result<Vec<_>>>()?,
            },
            delivery: ConfigDelivery::PendingDevice { last_failure: None },
        },
    })
}

fn inventory_device(device: &DeviceSnapshot) -> Result<IdentifiedDeviceSnapshot> {
    let observation = device
        .mount
        .as_deref()
        .and_then(|mount| crate::device::observe_mount(Path::new(mount), ObservationId::new(1)));
    let readiness = observation
        .as_ref()
        .map(|observation| observation.readiness())
        .unwrap_or(DeviceReadiness::Ready);
    let hardware = observation
        .as_ref()
        .map(|observation| observation.hardware_facts().clone())
        .unwrap_or_else(|| device.hardware.clone());
    let mut profile_status = match device.mount.as_deref() {
        Some(mount) => match read_profile(Path::new(mount))? {
            OwnedDeviceProfile::Valid(_) => ProfileStatus::Adopted,
            OwnedDeviceProfile::Invalid(_) => ProfileStatus::Invalid,
            OwnedDeviceProfile::Absent if device.configured => ProfileStatus::PendingAdoption,
            OwnedDeviceProfile::Absent => ProfileStatus::NotAdopted,
        },
        None if device.configured => ProfileStatus::Adopted,
        None => ProfileStatus::NotAdopted,
    };
    let phase = match device.phase {
        DevicePhaseLabel::Disconnected => DevicePhase::Disconnected,
        DevicePhaseLabel::Unconfigured => DevicePhase::Unconfigured,
        DevicePhaseLabel::Idle => DevicePhase::Idle,
        DevicePhaseLabel::Syncing => DevicePhase::Syncing,
        DevicePhaseLabel::Paused => DevicePhase::Paused,
        DevicePhaseLabel::Error => DevicePhase::Error,
    };
    if phase == DevicePhase::Syncing {
        profile_status = ProfileStatus::Adopted;
    }
    Ok(IdentifiedDeviceSnapshot {
        device_id: device_id(&device.identity.serial)?,
        name: device.identity.name.clone(),
        readiness,
        hardware,
        profile_status,
        connected: device.connected,
        mount_path: device.mount.clone(),
        phase,
        session_id: device.session_id.map(SessionId::new).transpose()?,
        storage: device.storage.map(|storage| StorageSnapshot {
            total_bytes: storage.total_bytes,
            free_bytes: storage.free_bytes,
            freshness: if device.connected {
                StorageFreshness::Live
            } else {
                StorageFreshness::Cached
            },
        }),
        synced_count: device.synced_count as u64,
        library_count: device.library_count.map(|count| count as u64),
        last_terminal_error: device.last_terminal_error.clone(),
    })
}

fn history_entry(
    entry: &crate::daemon::history::HistoryEntry,
) -> Result<crate::wire::HistoryEntry> {
    Ok(crate::wire::HistoryEntry {
        device_id: device_id(&entry.serial)?,
        session_id: entry.session_id.map(SessionId::new).transpose()?,
        timestamp: entry.timestamp.clone(),
        duration_secs: entry.duration_secs,
        trigger: match entry.trigger {
            crate::daemon::history::SyncTrigger::Manual => crate::wire::HistoryTrigger::Manual,
            crate::daemon::history::SyncTrigger::Scheduled => {
                crate::wire::HistoryTrigger::Scheduled
            }
            crate::daemon::history::SyncTrigger::PlugIn => crate::wire::HistoryTrigger::PlugIn,
            crate::daemon::history::SyncTrigger::Coalesced => {
                crate::wire::HistoryTrigger::Coalesced
            }
        },
        operation: crate::wire::SyncOperation::Sync,
        outcome: match entry.outcome {
            crate::daemon::history::SyncOutcome::Ok => crate::wire::SyncOutcome::Ok,
            crate::daemon::history::SyncOutcome::Error => crate::wire::SyncOutcome::Error,
            crate::daemon::history::SyncOutcome::Aborted => crate::wire::SyncOutcome::Aborted,
            crate::daemon::history::SyncOutcome::Cancelled => crate::wire::SyncOutcome::Cancelled,
        },
        error_message: entry.error_message.clone(),
        summary: entry
            .summary
            .as_ref()
            .map(|summary| crate::wire::HistorySummary {
                add: summary.add as u64,
                modify: summary.modify as u64,
                metadata_only: summary.metadata_only as u64,
                remove: summary.remove as u64,
                unchanged: summary.unchanged as u64,
                skipped: summary.skipped as u64,
                skipped_for_space_tracks: summary.skipped_for_space_tracks as u64,
                skipped_for_space_bytes: summary.skipped_for_space_bytes,
                artwork_failed_sources: summary.artwork_failed_sources as u64,
            }),
        db_restored: entry.db_restored,
    })
}

fn profile_selection(
    selection: &crate::ipc_daemon::SelectionPayload,
) -> Result<crate::portable::profile::SelectionValue> {
    let mut value = serde_json::to_value(selection)?;
    value
        .as_object_mut()
        .expect("selection serializes as an object")
        .insert("schema_version".to_string(), Value::from(1));
    serde_json::from_value(value).context("translate portable selection")
}

fn library_paths(paths: &[String]) -> Result<Vec<PortablePath>> {
    paths.iter().map(|path| PortablePath::parse(path)).collect()
}

fn device_id(value: &str) -> Result<DeviceId> {
    DeviceId::parse(value).map_err(Into::into)
}

fn request(value: &str) -> Result<RequestId> {
    RequestId::parse(value)
}

fn optional_request(value: Option<&str>) -> Result<Option<RequestId>> {
    value.map(request).transpose()
}

fn synthetic_mutation_id(device: &str, component: &str, revision: u64) -> Result<MutationId> {
    let digest = blake3::hash(format!("{device}:{component}:{revision}").as_bytes());
    let hex = digest.to_hex();
    MutationId::parse(&format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn map_sync_mode(mode: crate::config_file::SyncMode) -> crate::wire::SyncMode {
    match mode {
        crate::config_file::SyncMode::Review => crate::wire::SyncMode::Review,
        crate::config_file::SyncMode::AutoApply => crate::wire::SyncMode::AutoApply,
    }
}

fn map_notify(level: crate::config_file::NotifyLevel) -> crate::wire::NotifyLevel {
    match level {
        crate::config_file::NotifyLevel::All => crate::wire::NotifyLevel::All,
        crate::config_file::NotifyLevel::ErrorsOnly => crate::wire::NotifyLevel::ErrorsOnly,
        crate::config_file::NotifyLevel::None => crate::wire::NotifyLevel::None,
    }
}

fn map_drop_behavior(
    behavior: crate::config_file::DropSyncBehavior,
) -> crate::wire::DropSyncBehavior {
    match behavior {
        crate::config_file::DropSyncBehavior::Immediate => crate::wire::DropSyncBehavior::Immediate,
        crate::config_file::DropSyncBehavior::NextSync => crate::wire::DropSyncBehavior::NextSync,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Fact, HardwareFacts, IpodColour, IpodFamily};
    use crate::ipc_device::DeviceIdentitySnapshot;

    #[test]
    fn playlist_events_accept_unicode_source_relative_paths() {
        let event = DaemonEvent::PlaylistSelectionAppended {
            acknowledged_request_id: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8764".to_string(),
            slug: "new-playlist".to_string(),
            appended_tracks: 1,
            playlist_revision: 1,
            playlist: crate::ipc_daemon::ManualPlaylistPayload {
                slug: "new-playlist".to_string(),
                name: "New Playlist".to_string(),
                tracks: vec!["Björk/日本語 🎵/03 – I’m So Free.flac".to_string()],
            },
        };

        let converted = event_to_wire(&event).unwrap().unwrap();

        assert!(matches!(
            converted,
            WireEvent::PlaylistSelectionAppended { playlist: crate::wire::StoredPlaylist::Manual { tracks, .. }, .. }
                if tracks[0].as_str() == "Björk/日本語 🎵/03 – I’m So Free.flac"
        ));
    }

    #[test]
    fn disconnected_inventory_uses_persisted_hardware_facts() {
        let device = DeviceSnapshot {
            identity: DeviceIdentitySnapshot {
                serial: "000A27002138B0A8".to_string(),
                model_label: "iPod Classic (3rd gen)".to_string(),
                name: Some("Michael's iPod".to_string()),
            },
            hardware: HardwareFacts {
                family: Some(Fact::decoded(IpodFamily::Classic)),
                model_code: Some(Fact::reported("MC293".to_string())),
                colour: Some(Fact::decoded(IpodColour::Silver)),
                ..HardwareFacts::default()
            },
            configured: true,
            connected: false,
            mount: None,
            phase: DevicePhaseLabel::Disconnected,
            session_id: None,
            storage: None,
            synced_count: 0,
            library_count: None,
            latest_successful_sync: None,
            latest_attempt: None,
            last_terminal_error: None,
            selection_revision: 1,
            settings_revision: 1,
            subscriptions_revision: 1,
        };

        let inventory = inventory_device(&device).unwrap();

        assert_eq!(
            inventory.hardware.model_code,
            Some(Fact::reported("MC293".to_string()))
        );
        assert_eq!(
            inventory.hardware.colour,
            Some(Fact::decoded(IpodColour::Silver))
        );
    }
}
