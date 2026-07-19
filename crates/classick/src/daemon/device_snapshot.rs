//! Authoritative serial-keyed device inventory publication.

use crate::daemon::device_registry::DeviceRegistry;
use crate::daemon::history::{HistoryService, SyncOutcome};
use crate::daemon::runtime_state::RuntimeState;
use crate::ipc_daemon::DaemonEvent;
use crate::ipc_device::{
    DeviceIdentitySnapshot, DeviceInventorySnapshot, DevicePhaseLabel, DeviceSnapshot,
};
use std::path::Path;
use tokio::sync::broadcast;

#[derive(Debug, Default)]
pub(crate) struct DeviceSnapshotPublisher {
    revision: u64,
}

impl DeviceSnapshotPublisher {
    pub(crate) fn publish(
        &mut self,
        event_tx: &broadcast::Sender<DaemonEvent>,
        registry: &DeviceRegistry,
        state: &RuntimeState,
        history: &HistoryService,
        config_path: &Path,
        library_count_cache: Option<usize>,
    ) {
        let event = self.next_event(registry, state, history, config_path, library_count_cache);
        let _ = event_tx.send(event);
    }

    pub(crate) fn next_event(
        &mut self,
        registry: &DeviceRegistry,
        state: &RuntimeState,
        history: &HistoryService,
        config_path: &Path,
        library_count_cache: Option<usize>,
    ) -> DaemonEvent {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("device inventory revision space exhausted");
        let snapshot = build_snapshot(
            self.revision,
            registry,
            state,
            history,
            config_path,
            library_count_cache,
        );
        DaemonEvent::DeviceInventorySnapshot(snapshot)
    }
}

fn build_snapshot(
    revision: u64,
    registry: &DeviceRegistry,
    state: &RuntimeState,
    history: &HistoryService,
    config_path: &Path,
    library_count_cache: Option<usize>,
) -> DeviceInventorySnapshot {
    let devices = registry
        .records()
        .into_iter()
        .map(|record| {
            let connected = state.connected_device(&record.serial);
            let active = state.active_session().filter(|session| {
                session.serial.as_deref().is_some_and(|serial| {
                    crate::daemon::command_handler::same_serial(serial, &record.serial)
                })
            });
            let retained_attempt = state.retained_terminal_attempt(&record.serial).cloned();
            let latest_attempt = retained_attempt
                .clone()
                .or_else(|| history.latest_attempt(&record.serial));
            let latest_successful_sync = retained_attempt
                .filter(|entry| entry.outcome == SyncOutcome::Ok)
                .or_else(|| history.latest_success(&record.serial));
            let last_terminal_error = state
                .terminal_persistence_error(&record.serial)
                .map(str::to_string)
                .or_else(|| {
                    latest_attempt
                        .as_ref()
                        .filter(|entry| entry.outcome != SyncOutcome::Ok)
                        .and_then(|entry| entry.error_message.clone())
                });
            let phase = if connected.is_none() {
                DevicePhaseLabel::Disconnected
            } else if !record.configured {
                DevicePhaseLabel::Unconfigured
            } else if active.is_some() {
                DevicePhaseLabel::Syncing
            } else if last_terminal_error.as_deref() == Some("paused") {
                DevicePhaseLabel::Paused
            } else if last_terminal_error.is_some() {
                DevicePhaseLabel::Error
            } else {
                DevicePhaseLabel::Idle
            };
            DeviceSnapshot {
                identity: DeviceIdentitySnapshot {
                    serial: record.serial.clone(),
                    model_label: record.model_label,
                    name: record.name,
                },
                configured: record.configured,
                connected: connected.is_some(),
                mount: connected.map(|device| device.drive.clone()),
                phase,
                session_id: active.map(|session| session.id),
                storage: connected
                    .and_then(|device| crate::daemon::device_storage::query_storage(&device.drive)),
                synced_count: crate::daemon::runtime::synced_track_count_at_mount(
                    config_path,
                    Some(&record.serial),
                    connected.map(|device| Path::new(&device.drive)),
                ),
                library_count: crate::daemon::library::selected_library_count(
                    config_path,
                    &record.serial,
                )
                .or(library_count_cache),
                latest_successful_sync,
                latest_attempt,
                last_terminal_error,
                selection_revision: record.selection_revision,
                settings_revision: record.settings_revision,
                subscriptions_revision: record.subscriptions_revision,
            }
        })
        .collect();
    DeviceInventorySnapshot { revision, devices }
}
