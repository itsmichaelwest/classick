//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, device watcher, scheduler, and sync orchestrator.
//!
//! M3 scope: real auto-sync on configured-iPod plug-in, Sync Now via
//! manual TriggerSync, periodic Scheduled triggers from the scheduler,
//! and live DeviceConnected/Disconnected broadcasts. Test-only entry
//! `run_daemon_with_deps` exists so the integration suite can inject
//! a scripted device watcher and a fake spawn-fn.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::command_handler::{same_serial, target_rejection};
use crate::daemon::device_registry::DeviceRegistry;
use crate::daemon::device_snapshot::DeviceSnapshotPublisher;
use crate::daemon::device_storage;
#[cfg(not(target_os = "macos"))]
use crate::daemon::device_watcher::PollingDeviceWatcher;
use crate::daemon::device_watcher::{Debouncer, DeviceEvent, DeviceWatcher};
use crate::daemon::history::{HistoryEntry, HistoryService, SyncOutcome, SyncSummary, SyncTrigger};
use crate::daemon::ipc_server::{ClientCommand, InitialClientState};
use crate::daemon::runtime_state::{RuntimeState, SessionControls};
use crate::daemon::scheduler::SyncScheduler;
use crate::daemon::session_admission::EventContext;
use crate::daemon::source_availability::{
    ResolvedSource, SourceAvailabilityService, SourceUnavailable,
};
use crate::daemon::state::SessionKind;
use crate::daemon::sync_orchestrator::{self, OrchestratorOutcome};
use crate::ipc_daemon::{
    DaemonCommand, DaemonEvent, DaemonStateLabel, PlaylistKind, SourceAvailabilityState,
    TriggerSource,
};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Production entry. Constructs the real device watcher + real
/// spawn-fn and runs the daemon.
pub async fn run_daemon() -> Result<()> {
    tracing::info!("daemon: starting");
    let config_path = config_file::default_path()?;
    let configured_serial = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.ipod_identity)
        .map(|i| i.serial);
    let schedule_minutes = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.daemon)
        .map(|d| d.schedule_minutes)
        .unwrap_or(crate::daemon::DEFAULT_SCHEDULE_MINUTES);

    // Build the broadcast event_tx FIRST so the spawn_sync closure can
    // capture a clone — that way orchestrator events flow through the
    // same channel UI clients are subscribed to.
    let (event_tx, _) =
        broadcast::channel::<DaemonEvent>(crate::daemon::BROADCAST_CHANNEL_CAPACITY);
    let exe = std::env::current_exe()?;
    let event_tx_for_spawn = event_tx.clone();
    // The spawn closure re-reads the persisted (global) config AND the
    // syncing device's own settings at spawn time (same re-read-at-spawn
    // pattern as the GetConfig/SaveConfig command arms) rather than
    // capturing a snapshot at daemon startup — so a Settings change to
    // `rockbox_compat` takes effect on the very next sync without a daemon
    // restart. The device settings win over the global value once seeded;
    // see `device_config::DeviceSettings::load_or_migrate`.
    let config_path_for_spawn = config_path.clone();
    let spawn_sync: SpawnFn = Arc::new(
        move |serial: String, drive: String, cancel_rx, pause_rx, prompt_rx, event_context| {
            let exe = exe.clone();
            let event_tx = event_tx_for_spawn.clone();
            let global = config_file::load(&config_path_for_spawn)
                .ok()
                .flatten()
                .unwrap_or_default();
            let rockbox_compat =
                crate::device_config::DeviceSettings::load_or_migrate(&serial, &global)
                    .rockbox_compat;
            Box::pin(async move {
                sync_orchestrator::run(
                    exe,
                    drive,
                    rockbox_compat,
                    cancel_rx,
                    pause_rx,
                    prompt_rx,
                    event_tx,
                    event_context,
                )
                .await
            })
        },
    );

    let exe_for_backfill = std::env::current_exe()?;
    let event_tx_for_backfill = event_tx.clone();
    let spawn_backfill: SpawnFn = Arc::new(
        move |_serial: String, drive: String, cancel_rx, pause_rx, prompt_rx, event_context| {
            let exe = exe_for_backfill.clone();
            let event_tx = event_tx_for_backfill.clone();
            Box::pin(async move {
                sync_orchestrator::run_backfill(
                    exe,
                    drive,
                    cancel_rx,
                    pause_rx,
                    prompt_rx,
                    event_tx,
                    event_context,
                )
                .await
            })
        },
    );

    let exe_for_replace = std::env::current_exe()?;
    let event_tx_for_replace = event_tx.clone();
    let spawn_replace_library: SpawnFn = Arc::new(
        move |_serial: String, drive: String, cancel_rx, pause_rx, prompt_rx, event_context| {
            let exe = exe_for_replace.clone();
            let event_tx = event_tx_for_replace.clone();
            Box::pin(async move {
                sync_orchestrator::run_replace_library(
                    exe,
                    drive,
                    cancel_rx,
                    pause_rx,
                    prompt_rx,
                    event_tx,
                    event_context,
                )
                .await
            })
        },
    );

    let exe_for_scan = std::env::current_exe()?;
    let event_tx_for_scan = event_tx.clone();
    let spawn_scan: SpawnFn = Arc::new(
        move |_serial: String, _drive: String, cancel_rx, pause_rx, prompt_rx, event_context| {
            let exe = exe_for_scan.clone();
            let event_tx = event_tx_for_scan.clone();
            Box::pin(async move {
                sync_orchestrator::run_scan(
                    exe,
                    cancel_rx,
                    pause_rx,
                    prompt_rx,
                    event_tx,
                    event_context,
                )
                .await
            })
        },
    );

    let deps = DaemonDeps {
        configured_serial,
        #[cfg(target_os = "macos")]
        watcher: Box::new(crate::daemon::iokit_watcher::IokitDeviceWatcher::new_production()),
        #[cfg(not(target_os = "macos"))]
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        spawn_backfill,
        spawn_replace_library,
        spawn_scan,
        schedule_minutes,
        preset_event_tx: Some(event_tx),
        config_path: None,
        history_path: None,
        pipe_name: None,
        source_availability: None,
    };
    run_daemon_with_deps(deps).await
}

/// Async closure that runs one sync to completion. Arc-wrapped so the
/// runtime can clone it into a tokio::spawn'd task without consuming the
/// daemon's only copy.
///
/// Args: `(serial, drive, cancel_rx, pause_rx, prompt_decisions_rx,
/// event_context)`. `serial` lets `spawn_sync` resolve per-device settings
/// (Rockbox compat) at spawn time; closures that don't need it
/// (backfill/replace/scan) ignore it. `event_context` is created by admission,
/// so every forwarded line retains the admitted session ID and raw serial.
/// The prompt channel lets `DaemonCommand::DecidePrompt` ferry user replies
/// through to the running subprocess's stdin without blocking the runtime
/// loop. The pause channel lets `DaemonCommand::Pause` request a graceful stop.
pub type SpawnFn = Arc<
    dyn Fn(
            String,
            String,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
            tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>,
            EventContext,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
        + Send
        + Sync,
>;

pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--backfill-rockbox`
    /// subprocess (`sync_orchestrator::run_backfill` in production) instead
    /// of `--apply`. Used by the `DaemonCommand::BackfillRockbox` arm,
    /// which reuses `start_sync_session` — the same admission guard,
    /// cancel/pause/prompt channels, and event relay as a normal sync — so
    /// a backfill and a sync can never run concurrently.
    pub spawn_backfill: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--replace-library --apply`
    /// subprocess (`sync_orchestrator::run_replace_library` in production)
    /// instead of plain `--apply`. Used by the `DaemonCommand::ReplaceLibrary`
    /// arm, which reuses `start_sync_session` — the same admission
    /// guard, cancel/pause/prompt channels, and event relay as a normal
    /// sync — so a replace and a sync (or backfill) can never run
    /// concurrently.
    pub spawn_replace_library: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--scan-library` subprocess
    /// (`sync_orchestrator::run_scan` in production). The `drive` argument is
    /// ignored — a scan touches no device. Used by `DaemonCommand::ScanLibrary`
    /// via `start_scan_session`, which shares the sync guard.
    pub spawn_scan: SpawnFn,
    pub schedule_minutes: u32,
    /// If Some, the runtime uses this pre-built sender instead of
    /// constructing its own. Production passes the same one it gave
    /// to the spawn_sync closure so orchestrator events broadcast on
    /// the same channel UI clients subscribe to.
    pub preset_event_tx: Option<broadcast::Sender<DaemonEvent>>,
    /// Override the persisted-config path. Production passes `None` and
    /// `config_file::default_path()` resolves to
    /// `%APPDATA%\classick\config.toml`. Tests pass a tempdir-managed
    /// path with a known-good config so the suite is deterministic
    /// regardless of the developer's local settings (notably
    /// `subsequent_sync_mode`, which gates auto-sync).
    pub config_path: Option<PathBuf>,
    /// Override the history file path. Production: `None` → default
    /// (`%LOCALAPPDATA%\classick\history.json`). Tests: provide a
    /// temp path so history append/read doesn't pollute the
    /// developer's real history.json.
    pub history_path: Option<PathBuf>,
    /// Override the named-pipe name. Production: `None` → uses the
    /// `\\.\pipe\classick` constant which is the IPC contract with
    /// the UI. Tests pass a unique per-test pipe name like
    /// `\\.\pipe\classick-test-<pid>-<n>` so the suite runs even
    /// while a real daemon is bound to the production pipe.
    pub pipe_name: Option<String>,
    /// Override source mounting for deterministic recovery tests. Production
    /// uses the platform backend (NetFS on macOS, established sessions
    /// elsewhere).
    pub source_availability: Option<SourceAvailabilityService>,
}

/// Internal events posted from background sync tasks back to the runtime
/// loop. The runtime owns state + history; the spawned task only does
/// the actual sync work and ships its outcome here for state-machine
/// mutation + history persistence + broadcast.
enum InternalEvent {
    SyncCompleted {
        session_id: crate::ipc_device::SessionId,
        outcome: Result<OrchestratorOutcome>,
    },
    /// Sent by the iPod-name reader task after itdb_parse completes on
    /// a freshly-plugged device. The runtime applies the name to
    /// `connected` only if both serial and connection generation still match,
    /// then persists it in the device registry and re-broadcasts the updated
    /// identity.
    IpodNameResolved {
        serial: String,
        connection_generation: u64,
        name: Option<String>,
    },
    /// Result of an off-thread source-library walk (see `spawn_library_count`).
    /// Populates the cached `library_count` (Y in "X of Y synced") so the menu
    /// can show a total on a cold start, before any sync has run.
    LibraryCountComputed { count: usize },
    /// A --scan-library subprocess finished. No history entry — a scan is
    /// cache maintenance, not a sync.
    ScanCompleted {
        session_id: crate::ipc_device::SessionId,
        outcome: Result<OrchestratorOutcome>,
    },
    SourceAvailabilityResolved {
        attempt_id: u64,
        result: std::result::Result<ResolvedSource, SourceUnavailable>,
    },
}

#[derive(Debug, Default)]
struct ConfigRevision(u64);

impl ConfigRevision {
    fn current(&self) -> u64 {
        self.0
    }

    fn record_persisted_mutation(&mut self, did_mutate: bool) -> u64 {
        if did_mutate {
            self.0 = self
                .0
                .checked_add(1)
                .expect("daemon config revision space exhausted");
        }
        self.0
    }

    #[cfg(test)]
    fn advance_after_persist(&mut self) -> u64 {
        self.record_persisted_mutation(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSync {
    trigger: SyncTrigger,
    serial: String,
    drive: String,
}

#[derive(Debug, Default)]
struct PendingSourceAction {
    scan_requested: bool,
    sync: Option<PendingSync>,
}

#[derive(Debug)]
struct SourceRecoveryState {
    pending: PendingSourceAction,
    in_flight_attempt: Option<u64>,
    in_flight_interaction: Option<crate::daemon::source_availability::MountInteraction>,
    ui_escalation_pending: bool,
    next_attempt_id: u64,
    retry_request_ids: Vec<String>,
    sync_after_scan: Option<PendingSync>,
    current_state: SourceAvailabilityState,
    available_root: Option<PathBuf>,
}

impl Default for SourceRecoveryState {
    fn default() -> Self {
        Self {
            pending: PendingSourceAction::default(),
            in_flight_attempt: None,
            in_flight_interaction: None,
            ui_escalation_pending: false,
            next_attempt_id: 1,
            retry_request_ids: Vec::new(),
            sync_after_scan: None,
            current_state: SourceAvailabilityState::Unavailable,
            available_root: None,
        }
    }
}

impl SourceRecoveryState {
    fn record_retry(&mut self, allow_ui: bool, request_id: String) {
        self.retry_request_ids.push(request_id);
        if allow_ui
            && self.in_flight_interaction
                == Some(crate::daemon::source_availability::MountInteraction::SuppressUi)
        {
            self.ui_escalation_pending = true;
        }
    }

    fn take_ui_escalation_after_auth(
        &mut self,
        completed_interaction: crate::daemon::source_availability::MountInteraction,
    ) -> bool {
        if completed_interaction != crate::daemon::source_availability::MountInteraction::SuppressUi
            || !self.ui_escalation_pending
        {
            return false;
        }
        self.ui_escalation_pending = false;
        true
    }
}

impl PendingSourceAction {
    fn request_scan(&mut self) {
        self.scan_requested = true;
    }

    fn request_sync(&mut self, sync: PendingSync) {
        if self.sync.is_none() {
            self.sync = Some(sync);
        }
    }
}

fn mount_interaction(allow_ui: bool) -> crate::daemon::source_availability::MountInteraction {
    if allow_ui {
        crate::daemon::source_availability::MountInteraction::AllowUi
    } else {
        crate::daemon::source_availability::MountInteraction::SuppressUi
    }
}

fn persist_resolved_source(
    config_path: &Path,
    location: &crate::source_location::SourceLocation,
    resolved_root: &Path,
) -> Result<()> {
    let mut config = config_file::load(config_path)?.unwrap_or_default();
    let mut location = location.clone();
    location.resolved_path = resolved_root.to_path_buf();
    config.source = Some(resolved_root.to_path_buf());
    config.source_location = Some(location);
    config_file::save(config_path, &config)
}

fn apply_explicit_source_update(config: &mut PersistedConfig, source: PathBuf) -> Result<()> {
    let location = crate::source_location::SourceLocation::discover(source.clone())?;
    config.source = Some(source);
    config.source_location = Some(location);
    Ok(())
}

enum SourceActionRequest {
    Scan,
    Sync(PendingSync),
    Retry { allow_ui: bool, request_id: String },
}

fn request_source_action(
    request: SourceActionRequest,
    recovery: &mut SourceRecoveryState,
    availability: &SourceAvailabilityService,
    event_tx: &broadcast::Sender<DaemonEvent>,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    config_path: &Path,
) {
    let allow_ui = match request {
        SourceActionRequest::Scan => {
            recovery.pending.request_scan();
            false
        }
        SourceActionRequest::Sync(sync) => {
            recovery.pending.request_sync(sync);
            false
        }
        SourceActionRequest::Retry {
            allow_ui,
            request_id,
        } => {
            recovery.record_retry(allow_ui, request_id);
            allow_ui
        }
    };

    let Some(location) = configured_source_location_at(config_path) else {
        recovery.pending = PendingSourceAction::default();
        publish_source_availability(
            recovery,
            event_tx,
            SourceAvailabilityState::Unavailable,
            None,
        );
        return;
    };

    if recovery.in_flight_attempt.is_some() {
        return;
    }

    start_source_availability_attempt(
        recovery,
        availability,
        location,
        mount_interaction(allow_ui),
        event_tx,
        internal_tx,
    );
}

fn start_source_availability_attempt(
    recovery: &mut SourceRecoveryState,
    availability: &SourceAvailabilityService,
    location: crate::source_location::SourceLocation,
    interaction: crate::daemon::source_availability::MountInteraction,
    event_tx: &broadcast::Sender<DaemonEvent>,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) {
    debug_assert!(recovery.in_flight_attempt.is_none());
    let attempt_id = recovery.next_attempt_id;
    recovery.next_attempt_id = recovery
        .next_attempt_id
        .checked_add(1)
        .expect("source recovery attempt id space exhausted");
    recovery.in_flight_attempt = Some(attempt_id);
    recovery.in_flight_interaction = Some(interaction);
    recovery.current_state = SourceAvailabilityState::Remounting;
    recovery.available_root = None;
    let _ = event_tx.send(DaemonEvent::SourceAvailability {
        state: SourceAvailabilityState::Remounting,
        source_root: None,
        acknowledged_request_id: None,
    });

    let availability = availability.clone();
    let internal_tx = internal_tx.clone();
    tokio::spawn(async move {
        let result = availability
            .ensure_source_available(&location, interaction)
            .await;
        let _ = internal_tx.send(InternalEvent::SourceAvailabilityResolved { attempt_id, result });
    });
}

#[allow(clippy::too_many_arguments)]
fn start_available_source_actions(
    recovery: &mut SourceRecoveryState,
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_sync: &SpawnFn,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    registry: &DeviceRegistry,
    config_path: &Path,
    history: &HistoryService,
    library_count_cache: Option<usize>,
) {
    if !state.is_idle() {
        return;
    }
    if recovery.pending.scan_requested {
        recovery.pending.scan_requested = false;
        if recovery.sync_after_scan.is_none() {
            recovery.sync_after_scan = recovery.pending.sync.take();
        }
        start_scan_session(
            state,
            event_tx,
            spawn_scan,
            internal_tx,
            registry,
            config_path,
            history,
            library_count_cache,
        );
    } else if let Some(sync) = recovery.pending.sync.take() {
        start_sync_session(
            sync.trigger,
            sync.serial,
            sync.drive,
            state,
            event_tx,
            spawn_sync,
            internal_tx,
            config_path,
            library_count_cache,
        );
    }
}

fn publish_source_availability(
    recovery: &mut SourceRecoveryState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: SourceAvailabilityState,
    source_root: Option<PathBuf>,
) {
    recovery.ui_escalation_pending = false;
    recovery.current_state = state;
    recovery.available_root = source_root.clone();
    let source_root = source_root.map(|path| path.to_string_lossy().into_owned());
    if recovery.retry_request_ids.is_empty() {
        let _ = event_tx.send(DaemonEvent::SourceAvailability {
            state,
            source_root,
            acknowledged_request_id: None,
        });
        return;
    }
    for request_id in recovery.retry_request_ids.drain(..) {
        let _ = event_tx.send(DaemonEvent::SourceAvailability {
            state,
            source_root: source_root.clone(),
            acknowledged_request_id: Some(request_id),
        });
    }
}

fn invalidate_source_recovery_for_source_change(
    recovery: &mut SourceRecoveryState,
    event_tx: &broadcast::Sender<DaemonEvent>,
) {
    recovery.in_flight_attempt = None;
    recovery.in_flight_interaction = None;
    recovery.ui_escalation_pending = false;
    publish_source_availability(
        recovery,
        event_tx,
        SourceAvailabilityState::Unavailable,
        None,
    );
}

fn source_availability_replay(recovery: &SourceRecoveryState) -> DaemonEvent {
    DaemonEvent::SourceAvailability {
        state: recovery.current_state,
        source_root: recovery
            .available_root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        acknowledged_request_id: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn complete_source_recovery(
    attempt_id: u64,
    result: std::result::Result<ResolvedSource, SourceUnavailable>,
    recovery: &mut SourceRecoveryState,
    availability: &SourceAvailabilityService,
    library_watcher: &mut crate::daemon::library_watcher::LibraryWatcher,
    library_scan_deadline: &mut Option<tokio::time::Instant>,
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_sync: &SpawnFn,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    registry: &DeviceRegistry,
    config_path: &Path,
    history: &HistoryService,
    library_count_cache: Option<usize>,
    config_revision: &mut ConfigRevision,
) {
    if recovery.in_flight_attempt != Some(attempt_id) {
        tracing::debug!(attempt_id, "daemon: ignoring stale source recovery result");
        return;
    }
    recovery.in_flight_attempt = None;
    let completed_interaction = recovery
        .in_flight_interaction
        .take()
        .expect("active source attempt carries its mount interaction");
    match result {
        Ok(resolved) => {
            if resolved.remounted {
                let Some(location) = configured_source_location_at(config_path) else {
                    publish_source_availability(
                        recovery,
                        event_tx,
                        SourceAvailabilityState::Unavailable,
                        None,
                    );
                    return;
                };
                if let Err(error) = persist_resolved_source(config_path, &location, &resolved.root)
                {
                    tracing::warn!("daemon: failed to persist recovered source: {error:#}");
                    publish_source_availability(
                        recovery,
                        event_tx,
                        SourceAvailabilityState::Unavailable,
                        None,
                    );
                    return;
                }
                config_revision.record_persisted_mutation(true);
                let _ = event_tx.send(build_config_update(
                    config_file::load(config_path).ok().flatten(),
                    registry,
                    config_revision.current(),
                    None,
                ));
                library_watcher.rewatch(Some(resolved.root.clone()));
                *library_scan_deadline = None;
                recovery.pending.request_scan();
            } else if !recovery.retry_request_ids.is_empty() {
                recovery.pending.request_scan();
            }
            publish_source_availability(
                recovery,
                event_tx,
                SourceAvailabilityState::Available,
                Some(resolved.root),
            );
            start_available_source_actions(
                recovery,
                state,
                event_tx,
                spawn_sync,
                spawn_scan,
                internal_tx,
                registry,
                config_path,
                history,
                library_count_cache,
            );
        }
        Err(SourceUnavailable::AuthRequired) => {
            if recovery.take_ui_escalation_after_auth(completed_interaction) {
                let Some(location) = configured_source_location_at(config_path) else {
                    publish_source_availability(
                        recovery,
                        event_tx,
                        SourceAvailabilityState::Unavailable,
                        None,
                    );
                    return;
                };
                start_source_availability_attempt(
                    recovery,
                    availability,
                    location,
                    crate::daemon::source_availability::MountInteraction::AllowUi,
                    event_tx,
                    internal_tx,
                );
            } else {
                publish_source_availability(
                    recovery,
                    event_tx,
                    SourceAvailabilityState::AuthRequired,
                    None,
                );
            }
        }
        Err(
            SourceUnavailable::IdentityMismatch
            | SourceUnavailable::MountFailed(_)
            | SourceUnavailable::MissingSubpath(_),
        ) => publish_source_availability(
            recovery,
            event_tx,
            SourceAvailabilityState::Unavailable,
            None,
        ),
    }
}

/// Test-friendly entry. Production wraps real impls and calls this.
pub async fn run_daemon_with_deps(deps: DaemonDeps) -> Result<()> {
    let history_path = match deps.history_path {
        Some(p) => p,
        None => history_file_path()?,
    };
    let history = HistoryService::new(history_path);
    let config_path = match deps.config_path {
        Some(p) => p,
        None => config_file::default_path()?,
    };
    let legacy_identity = config_file::load(&config_path)?
        .and_then(|config| config.ipod_identity)
        .or_else(|| {
            deps.configured_serial
                .as_ref()
                .map(|serial| crate::config_file::IpodIdentity {
                    serial: serial.clone(),
                    model_label: String::new(),
                    name: None,
                    custom_selection: false,
                })
        });
    let mut registry = DeviceRegistry::load_or_migrate(
        config_file::device_registry_path(&config_path),
        legacy_identity.as_ref(),
    )?;
    if let Some(serial) = unique_configured_serial(&registry) {
        history.migrate_legacy_entries(&serial)?;
    }
    let mut state = RuntimeState::new();
    let mut snapshot_publisher = DeviceSnapshotPublisher::default();
    let mut config_revision = ConfigRevision::default();
    let mut scheduler = SyncScheduler::new(deps.schedule_minutes);
    let mut debouncer = Debouncer::new(crate::daemon::DEVICE_DEBOUNCE_WINDOW);
    let source_availability = deps
        .source_availability
        .unwrap_or_else(SourceAvailabilityService::platform_default);
    let mut source_recovery = SourceRecoveryState::default();

    let pipe_name = deps
        .pipe_name
        .clone()
        .unwrap_or_else(crate::daemon::ipc_server::default_pipe_name);
    let (event_tx, mut cmd_rx, mut new_client_rx) = match deps.preset_event_tx {
        Some(tx) => crate::daemon::ipc_server::spawn_server_full_with(tx, &pipe_name).await?,
        None => {
            let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
            let (tx, rx, new_client_rx) =
                crate::daemon::ipc_server::spawn_server_full_with(event_tx, &pipe_name).await?;
            (tx, rx, new_client_rx)
        }
    };
    let mut device_rx = deps.watcher.start();
    // Filesystem watcher over the configured source library. Emits coalesced
    // change ticks; the select loop debounces them and triggers a scan.
    let initial_source = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source);
    let (mut library_watcher, mut library_rx) =
        crate::daemon::library_watcher::LibraryWatcher::spawn(initial_source);
    // Deadline used to debounce a burst of FS events into a single scan.
    let mut library_scan_deadline: Option<tokio::time::Instant> = None;
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let spawn_sync = deps.spawn_sync;
    let spawn_backfill = deps.spawn_backfill;
    let spawn_replace_library = deps.spawn_replace_library;
    let spawn_scan = deps.spawn_scan;
    // Cached source-library track count (Y in "X of Y synced"). Walking the
    // source on every status tick would stall the daemon loop, so it's cached:
    // filled by an off-thread walk at startup + after SaveConfig (see
    // `spawn_library_count`), and also refreshed for free from each sync's
    // already-performed diff (add + modify + unchanged + metadata_only ==
    // current source count). `None` only until the first walk/sync lands.
    let mut library_count_cache: Option<usize> = None;
    tracing::info!(
        "daemon: ready ({} remembered devices)",
        registry.records().len()
    );

    // Proactively count the source library so "X of Y synced" shows a total on
    // a cold start, before any sync has run. Fills `library_count_cache`
    // asynchronously via InternalEvent::LibraryCountComputed.
    spawn_library_count(&config_path, &internal_tx);

    // Refresh the library index once at startup so the browser is current
    // without a user action. Guarded/incremental like any scan.
    if configured_source_location_at(&config_path).is_some() {
        request_source_action(
            SourceActionRequest::Scan,
            &mut source_recovery,
            &source_availability,
            &event_tx,
            &internal_tx,
            &config_path,
        );
        snapshot_publisher.publish(
            &event_tx,
            &registry,
            &state,
            &history,
            &config_path,
            library_count_cache,
        );
    }

    let exit_reason: ExitReason = loop {
        tokio::select! {
            biased;

            client_cmd = cmd_rx.recv() => {
                let Some(client_cmd) = client_cmd else {
                    tracing::info!("daemon: command channel closed; exiting");
                    return Ok(());
                };
                // Only SaveConfig can change the source path; avoid a disk
                // read + rewatch on every other command.
                let is_save_config =
                    matches!(&client_cmd.command, DaemonCommand::SaveConfig { .. });
                let should_exit = handle_client_command(
                    client_cmd,
                    &history,
                    &config_path,
                    &mut state,
                    &event_tx,
                    &mut registry,
                    &spawn_backfill,
                    &spawn_replace_library,
                    &internal_tx,
                    &mut scheduler,
                    &mut library_count_cache,
                    &mut config_revision,
                    &mut source_recovery,
                    &source_availability,
                );
                snapshot_publisher.publish(
                    &event_tx,
                    &registry,
                    &state,
                    &history,
                    &config_path,
                    library_count_cache,
                );
                // A SaveConfig may have changed the source path; re-point the
                // watcher. rewatch() is a no-op when the path is unchanged.
                // A transient config-read failure must NOT disarm the
                // watcher — only a legitimately-absent source should.
                if is_save_config {
                    match config_file::load(&config_path) {
                        Ok(cfg) => library_watcher.rewatch(cfg.and_then(|c| c.source)),
                        Err(e) => tracing::warn!(
                            "daemon: skipping library rewatch, config load failed: {e:#}"
                        ),
                    }
                    request_source_action(
                        SourceActionRequest::Scan,
                        &mut source_recovery,
                        &source_availability,
                        &event_tx,
                        &internal_tx,
                        &config_path,
                    );
                }
                if should_exit { break ExitReason::Shutdown; }
            }

            device_event = device_rx.recv() => {
                let Some(raw) = device_event else {
                    tracing::warn!("daemon: device watcher channel closed");
                    continue;
                };
                let Some(event) = debouncer.admit(raw) else { continue };
                handle_device_event(
                    event,
                    &mut registry,
                    &event_tx,
                    &mut state,
                    &history,
                    &internal_tx,
                    &config_path,
                    config_revision.current(),
                    &mut source_recovery,
                    &source_availability,
                );
                broadcast_status(&event_tx, &state, &registry, &config_path, &history, library_count_cache);
                snapshot_publisher.publish(
                    &event_tx,
                    &registry,
                    &state,
                    &history,
                    &config_path,
                    library_count_cache,
                );
            }

            Some(internal) = internal_rx.recv() => {
                match internal {
                    InternalEvent::SourceAvailabilityResolved { attempt_id, result } => {
                        complete_source_recovery(
                            attempt_id,
                            result,
                            &mut source_recovery,
                            &source_availability,
                            &mut library_watcher,
                            &mut library_scan_deadline,
                            &mut state,
                            &event_tx,
                            &spawn_sync,
                            &spawn_scan,
                            &internal_tx,
                            &registry,
                            &config_path,
                            &history,
                            library_count_cache,
                            &mut config_revision,
                        );
                    }
                    internal => {
                        let completes_active_scan = matches!(
                            &internal,
                            InternalEvent::ScanCompleted { session_id, .. }
                                if state.active_session().is_some_and(|session| session.id == *session_id)
                        );
                        handle_internal_event(
                            internal,
                            &mut state,
                            &event_tx,
                            &history,
                            &mut registry,
                            &config_path,
                            &mut library_count_cache,
                            &mut config_revision,
                        );
                        if completes_active_scan {
                            if let Some(sync) = source_recovery.sync_after_scan.take() {
                                source_recovery.pending.request_sync(sync);
                            }
                        }
                        if state.is_idle()
                            && source_recovery.current_state
                                == SourceAvailabilityState::Available
                        {
                            start_available_source_actions(
                                &mut source_recovery,
                                &mut state,
                                &event_tx,
                                &spawn_sync,
                                &spawn_scan,
                                &internal_tx,
                                &registry,
                                &config_path,
                                &history,
                                library_count_cache,
                            );
                        }
                    }
                }
                snapshot_publisher.publish(
                    &event_tx,
                    &registry,
                    &state,
                    &history,
                    &config_path,
                    library_count_cache,
                );
            }

            Some(new_client) = new_client_rx.recv() => {
                // Initial state is returned as one client-scoped ordered batch.
                // Broadcasting this replay lets an existing UI mistake another
                // client's source snapshot for its own in-flight retry result.
                let mut initial = vec![build_status_update(
                    &state,
                    &registry,
                    &config_path,
                    &history,
                    library_count_cache,
                    None,
                )];
                initial.push(snapshot_publisher.next_event(
                    &registry,
                    &state,
                    &history,
                    &config_path,
                    library_count_cache,
                ));
                if configured_source_location_at(&config_path).is_some() {
                    initial.push(source_availability_replay(&source_recovery));
                }
                let live_events = event_tx.subscribe();
                let _ = new_client.initial.send(InitialClientState {
                    events: initial,
                    live_events,
                });
            }

            _ = scheduler.tick() => {
                // Scheduled syncs also honour the user's auto/manual choice;
                // schedule_minutes is moot when the user opted into manual.
                // The gate reads the CONFIGURED device's settings (not
                // necessarily `connected`, though in practice a scheduled
                // tick only does anything when they match — see the
                // plug-in gate below for the equivalent check on attach).
                let scheduled_device = registry.records().into_iter()
                    .filter(|record| record.configured)
                    .find_map(|record| state.connected_device(&record.serial).cloned());
                if state.is_idle() && scheduled_device.as_ref()
                    .is_some_and(|device| auto_sync_enabled(&config_path, &device.serial)) {
                    if let Some(device) = scheduled_device {
                        request_source_action(
                            SourceActionRequest::Sync(PendingSync {
                                trigger: SyncTrigger::Scheduled,
                                serial: device.serial.clone(),
                                drive: device.drive.clone(),
                            }),
                            &mut source_recovery,
                            &source_availability,
                            &event_tx,
                            &internal_tx,
                            &config_path,
                        );
                        snapshot_publisher.publish(
                            &event_tx,
                            &registry,
                            &state,
                            &history,
                            &config_path,
                            library_count_cache,
                        );
                    }
                }
            }

            Some(()) = library_rx.recv() => {
                // Coalesce: (re)arm the debounce deadline. The timer arm below
                // fires the scan once the source has been quiet for the window.
                library_scan_deadline =
                    Some(tokio::time::Instant::now() + crate::daemon::LIBRARY_DEBOUNCE_WINDOW);
            }

            _ = async {
                match library_scan_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Only scan when idle + a source is configured. If busy (a
                // sync/scan is in flight), re-arm the deadline instead of
                // dropping it, so the pending rescan is retried once the
                // current operation finishes rather than being lost forever.
                let has_source = config_file::load(&config_path).ok().flatten()
                    .and_then(|c| c.source).is_some();
                if !has_source {
                    // Nothing to scan — clear the deadline.
                    library_scan_deadline = None;
                } else if state.is_idle() {
                    // Consume the deadline and run the incremental scan.
                    library_scan_deadline = None;
                    tracing::info!("daemon: library watcher fired a scan after debounce");
                    request_source_action(
                        SourceActionRequest::Scan,
                        &mut source_recovery,
                        &source_availability,
                        &event_tx,
                        &internal_tx,
                        &config_path,
                    );
                    snapshot_publisher.publish(
                        &event_tx,
                        &registry,
                        &state,
                        &history,
                        &config_path,
                        library_count_cache,
                    );
                } else {
                    // Busy (sync/scan in flight): retry after another debounce
                    // window rather than dropping the pending rescan.
                    library_scan_deadline =
                        Some(tokio::time::Instant::now() + crate::daemon::LIBRARY_DEBOUNCE_WINDOW);
                }
            }
        }
    };

    // Graceful shutdown: if a sync is in flight, give the orchestrator a
    // bounded window to drain (it writes Cancel to subprocess stdin and
    // force-kills after SYNC_KILL_GRACE). The kill_on_drop flag on the
    // child Command is the backstop — when this function returns and the
    // tokio runtime tears down, the orchestrator task is dropped and any
    // still-living subprocess gets TerminateProcess'd. Without this drain,
    // the OS would yank the subprocess mid-itdb_write and risk corrupting
    // the iPod's iTunesDB.
    match exit_reason {
        ExitReason::Shutdown => {
            let active_session_id = state.active_session().map(|session| session.id);
            if active_session_id.is_none() {
                tracing::info!("daemon: clean shutdown — no in-flight sync to drain");
            } else {
                if let Some(tx) = active_session_id.and_then(|id| state.take_cancel(id)) {
                    let _ = tx.send(());
                    tracing::info!("daemon: signalled in-flight sync to cancel before exit");
                }
                let drain = tokio::time::timeout(crate::daemon::SHUTDOWN_DRAIN_BUDGET, async {
                    while let Some(internal) = internal_rx.recv().await {
                        match internal {
                            InternalEvent::SyncCompleted { session_id, .. }
                            | InternalEvent::ScanCompleted { session_id, .. }
                                if Some(session_id) == active_session_id =>
                            {
                                return;
                            }
                            _ => {}
                        }
                    }
                })
                .await;
                match drain {
                    Ok(()) => tracing::info!("daemon: in-flight sync drained cleanly"),
                    Err(_) => tracing::warn!(
                        "daemon: shutdown drain timed out after {:?}; subprocess will be killed by kill_on_drop",
                        crate::daemon::SHUTDOWN_DRAIN_BUDGET,
                    ),
                }
            }
        }
    }
    Ok(())
}

/// Reason we exited the main select loop. Currently only Shutdown; the
/// enum exists so a future channel-closed / panic-recovery branch can
/// take a different drain path.
enum ExitReason {
    Shutdown,
}

/// Apply a sync's outcome to state + history, then broadcast a fresh
/// StatusUpdate so UIs flip back to Idle. Called from the runtime
/// loop when a SyncCompleted internal event arrives — i.e., AFTER
/// the spawned orchestrator task has finished.
fn handle_internal_event(
    event: InternalEvent,
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    history: &HistoryService,
    registry: &mut DeviceRegistry,
    config_path: &std::path::Path,
    library_count_cache: &mut Option<usize>,
    config_revision: &mut ConfigRevision,
) {
    match event {
        InternalEvent::SourceAvailabilityResolved { .. } => {
            unreachable!("source recovery results are handled by the runtime loop")
        }
        InternalEvent::LibraryCountComputed { count } => {
            *library_count_cache = Some(count);
            broadcast_status(
                event_tx,
                state,
                registry,
                config_path,
                history,
                *library_count_cache,
            );
        }
        InternalEvent::IpodNameResolved {
            serial,
            connection_generation,
            name,
        } => {
            if state.connection_generation(&serial) != Some(connection_generation) {
                tracing::debug!(
                    "daemon: ignoring stale iPod name for {serial} connection generation {connection_generation}"
                );
                return;
            }
            let Some(name) = name else {
                return;
            };
            let Some(connected) = state.connected_device_mut(&serial) else {
                return;
            };
            if connected.name.as_ref() == Some(&name) {
                return;
            }
            connected.name = Some(name);
            let connected = connected.clone();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let persisted_name_change = match registry.observe(&connected, now) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!("daemon: failed to persist resolved iPod name: {error:#}");
                    false
                }
            };
            config_revision.record_persisted_mutation(persisted_name_change);

            // Re-broadcast DeviceConnected with the now-populated
            // name, and a ConfigUpdate so the popover/title bar
            // refreshes from either path.
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: connected.serial.clone(),
                model_label: connected.model_label.clone(),
                drive: connected.drive.clone(),
                name: connected.name.clone(),
            });
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = event_tx.send(build_config_update(
                cfg,
                registry,
                config_revision.current(),
                None,
            ));
            return;
        }
        InternalEvent::SyncCompleted {
            session_id,
            outcome,
        } => {
            let Some(session) = state.finish(session_id) else {
                tracing::debug!("daemon: stale sync completion for session {session_id}; ignoring");
                return;
            };
            let serial = session
                .serial
                .expect("device sync session must carry its raw serial");

            if let Some(detached) = state.take_detached_terminal_intent(session_id) {
                if !detached.persisted {
                    persist_terminal_entry(state, history, detached.entry);
                }
                broadcast_status(
                    event_tx,
                    state,
                    registry,
                    config_path,
                    history,
                    *library_count_cache,
                );
                return;
            }

            let (history_outcome, error_message, summary, db_restored) = match outcome {
                Ok(OrchestratorOutcome::Completed {
                    outcome: SyncOutcome::Ok,
                    summary,
                    db_restored,
                }) => (SyncOutcome::Ok, None, summary, db_restored),
                Ok(OrchestratorOutcome::Completed {
                    outcome,
                    summary,
                    db_restored,
                }) => (
                    outcome,
                    Some("sync subprocess reported failure".to_string()),
                    summary,
                    db_restored,
                ),
                Ok(OrchestratorOutcome::Aborted { reason, summary }) => {
                    (SyncOutcome::Aborted, Some(reason), summary, false)
                }
                // A graceful pause isn't a failure or a user-driven abort of
                // the *library* — it's recorded as Aborted (reason "paused")
                // so history still reflects "didn't fully complete", while
                // the live "paused" signal itself rode the raw SyncEvent
                // stream the UI's Phase.paused reducer watches directly.
                Ok(OrchestratorOutcome::Paused { summary }) => (
                    SyncOutcome::Aborted,
                    Some("paused".to_string()),
                    summary,
                    false,
                ),
                Err(e) => (
                    SyncOutcome::Error,
                    Some(format!("orchestrator: {e:#}")),
                    None,
                    false,
                ),
            };

            // Refresh the library-count cache from this sync's diff — free,
            // since the apply loop already walked the source to compute it.
            // add + modify + unchanged + metadata_only are the tracks
            // currently present in the source (remove is present in the
            // manifest but gone from source, so it's excluded from "current
            // library size"). metadata_only tracks are already on the iPod
            // (only their tags/art were rewritten) — omitting them here
            // undercounts the total, so the UI's "X of Y synced" could show
            // X > Y after a tag-only sync.
            if let Some(s) = summary.as_ref() {
                *library_count_cache = Some(s.add + s.modify + s.unchanged + s.metadata_only);
            }

            let entry = make_history_entry(
                serial.clone(),
                session.id,
                session.trigger,
                history_outcome,
                error_message,
                summary,
                session.started_at_unix_secs,
                db_restored,
            );
            persist_terminal_entry(state, history, entry);
            broadcast_status(
                event_tx,
                state,
                registry,
                config_path,
                history,
                *library_count_cache,
            );
        }
        InternalEvent::ScanCompleted {
            session_id,
            outcome,
        } => {
            if state.finish(session_id).is_none() {
                tracing::debug!("daemon: stale scan completion for session {session_id}; ignoring");
                return;
            }
            if let Err(e) = &outcome {
                tracing::warn!("daemon: library scan failed: {e:#}");
            }
            // Fresh index on disk: rebroadcast the library and a status
            // update (selection-aware count may have changed).
            let _ = event_tx.send(crate::daemon::library::build_library_update(config_path));
            broadcast_status(
                event_tx,
                state,
                registry,
                config_path,
                history,
                *library_count_cache,
            );
        }
    }
}

/// Whether the daemon should auto-trigger syncs (plug-in + scheduled) for
/// `serial`. Gated on that device's own `auto_sync` toggle ("Sync
/// automatically on plug-in") — per-device since the trust-package settings
/// migration, no longer the global daemon setting. When it's off — or
/// neither the global config nor the device settings can be read — plug-in
/// and scheduled triggers no-op and the only way to start a sync is the
/// tray's Sync Now action. `subsequent_sync_mode` is NOT the gate: it
/// selects apply-vs-review *once a sync runs* (review is v1.1; today every
/// sync applies).
///
/// TODO(windows): the WinUI app still encodes auto-sync on/off in
/// `subsequent_sync_mode` (`WizardViewModel`: `IsAutomatic ? "auto_apply" :
/// "review"`) and always sends `enabled: true`. Until it maps the on/off
/// intent to `enabled` instead, a Windows user who picks Manual mode will
/// still be auto-synced. macOS already writes `enabled` correctly. This
/// still holds under per-device settings: `enabled` is only the seed value
/// the first time a device is seen (see `DeviceSettings::load_or_migrate`).
fn auto_sync_enabled(config_path: &std::path::Path, serial: &str) -> bool {
    let global = config_file::load(config_path)
        .ok()
        .flatten()
        .unwrap_or_default();
    let device = crate::device_config::DeviceSettings::load_or_migrate_in(
        device_state_root(config_path),
        serial,
        &global,
    );
    should_auto_sync(&device)
}

/// Pure decision core of [`auto_sync_enabled`], split out so it can be
/// tested without touching the filesystem.
pub(crate) fn should_auto_sync(settings: &crate::device_config::DeviceSettings) -> bool {
    settings.auto_sync
}

fn handle_device_event(
    event: DeviceEvent,
    registry: &mut DeviceRegistry,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut RuntimeState,
    history: &HistoryService,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    config_path: &std::path::Path,
    config_revision: u64,
    source_recovery: &mut SourceRecoveryState,
    source_availability: &SourceAvailabilityService,
) {
    match event {
        DeviceEvent::Connected(mut ipod) => {
            if ipod.name.is_none() {
                if let Some(record) = registry.record(&ipod.serial) {
                    ipod.name = record.name.clone();
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            if let Err(error) = registry.observe(&ipod, now) {
                tracing::error!(
                    "daemon: failed to observe device {}: {error:#}",
                    ipod.serial
                );
                return;
            }
            state.connect(ipod.clone());
            let connection_generation = state
                .connection_generation(&ipod.serial)
                .expect("connected device has a generation");
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: ipod.serial.clone(),
                model_label: ipod.model_label.clone(),
                drive: ipod.drive.clone(),
                name: ipod.name.clone(),
            });
            // Pair every attach with a ConfigUpdate. Otherwise a cached-name
            // plug-in (where IpodNameResolved dedups on unchanged name and
            // never re-broadcasts) leaves a subscriber that connected before
            // the attach without the persisted iPod identity — so it can't
            // match the connected serial and stays stuck on "Set Up".
            let _ = event_tx.send(build_config_update(
                config_file::load(config_path).ok().flatten(),
                registry,
                config_revision,
                None,
            ));

            // Off-thread iTunesDB read so the daemon loop stays
            // responsive. Result arrives via IpodNameResolved.
            let drive_for_read = ipod.drive.clone();
            let serial_for_read = ipod.serial.clone();
            let tx_for_read = internal_tx.clone();
            tokio::task::spawn_blocking(move || {
                let drive_path = std::path::PathBuf::from(&drive_for_read);
                let started = std::time::Instant::now();
                let name = crate::ipod::db::read_ipod_name(&drive_path);
                tracing::info!(
                    "daemon: read iPod name for {serial_for_read} in {}ms → {:?}",
                    started.elapsed().as_millis(),
                    name,
                );
                let _ = tx_for_read.send(InternalEvent::IpodNameResolved {
                    serial: serial_for_read,
                    connection_generation,
                    name,
                });
            });

            // Auto-sync only fires for the configured serial AND when the
            // user has opted into automatic mode. Manual mode means they
            // want to drive sync explicitly via the tray's Sync Now action.
            let configured_serial = registry
                .record(&ipod.serial)
                .filter(|record| record.configured)
                .map(|record| record.serial.clone());
            if configured_serial
                .as_deref()
                .is_some_and(|serial| state.is_idle() && auto_sync_enabled(config_path, serial))
            {
                let serial = configured_serial.expect("configured serial checked above");
                request_source_action(
                    SourceActionRequest::Sync(PendingSync {
                        trigger: SyncTrigger::PlugIn,
                        serial,
                        drive: ipod.drive.clone(),
                    }),
                    source_recovery,
                    source_availability,
                    event_tx,
                    internal_tx,
                    config_path,
                );
            }
        }
        DeviceEvent::Disconnected { serial } => {
            let was_connected = state.disconnect(&serial).is_some();
            let _ = event_tx.send(DaemonEvent::DeviceDisconnected {
                serial: serial.clone(),
            });
            // If the device we were syncing disappeared, force-finish
            // the session with Aborted. The spawned orchestrator task
            // is still running — its SyncCompleted will arrive later and
            // be silently dropped (handle_internal_event checks for the
            // already-Idle case).
            if let Some(s) = state.active_session().filter(|_| was_connected).cloned() {
                if s.serial
                    .as_deref()
                    .is_some_and(|active| same_serial(active, &serial))
                {
                    state.signal_cancel(s.id);
                    let admitted_serial = s
                        .serial
                        .clone()
                        .expect("device sync session must carry its raw serial");
                    let entry = make_history_entry(
                        admitted_serial,
                        s.id,
                        s.trigger.clone(),
                        SyncOutcome::Aborted,
                        Some("device_detached".to_string()),
                        None,
                        s.started_at_unix_secs,
                        false,
                    );
                    let persisted = persist_terminal_entry(state, history, entry.clone());
                    state.record_detached_terminal_intent(s.id, entry, persisted);
                }
            }
        }
    }
}

/// Kick off a sync as a background task. Updates state to Syncing and
/// emits the Syncing StatusUpdate broadcast immediately, then returns
/// so the runtime loop stays responsive to client commands + device
/// events. The spawned task ships its outcome back via internal_tx,
/// where handle_internal_event picks it up to finalize state + history.
fn start_sync_session(
    trigger: SyncTrigger,
    serial: String,
    drive: String,
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    config_path: &Path,
    library_count_cache: Option<usize>,
) {
    let Ok(session) = state.try_admit_device(trigger, &serial, std::path::Path::new(&drive)) else {
        return;
    };
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: DaemonStateLabel::Syncing,
        configured: true,
        ipod_connected: true,
        last_sync: None,
        next_scheduled_unix_secs: None,
        storage: device_storage::query_storage(&drive),
        synced_count: synced_track_count_at_mount(
            config_path,
            Some(&serial),
            Some(Path::new(&drive)),
        ),
        library_count: library_count_cache,
        acknowledged_request_id: None,
    });

    // Per-sync cancel channel. Sender held by the runtime so the
    // CancelSync IPC command can wake the orchestrator; Receiver
    // passed into the spawn closure.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

    // Per-sync prompt-decision channel. Sender held by the runtime
    // so DaemonCommand::DecidePrompt can ferry user replies through
    // to the subprocess. Receiver passed into the spawn closure for
    // the orchestrator's select loop to read.
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<(u64, i32)>();

    // Per-sync pause channel. Sender held by the runtime so the Pause
    // IPC command can wake the orchestrator; Receiver passed into the
    // spawn closure.
    let (pause_tx, pause_rx) = oneshot::channel::<()>();
    state.install_controls(
        session.id,
        SessionControls::new(cancel_tx, pause_tx, prompt_tx),
    );

    let spawn_sync = spawn_sync.clone();
    let internal_tx = internal_tx.clone();
    let drive_for_task = drive.clone();
    let serial_for_spawn = serial;
    let event_context = EventContext::from(&session);
    let session_id = session.id;
    tokio::spawn(async move {
        let outcome = (spawn_sync)(
            serial_for_spawn,
            drive_for_task,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_context,
        )
        .await;
        let _ = internal_tx.send(InternalEvent::SyncCompleted {
            session_id,
            outcome,
        });
    });
}

/// Start a library-scan subprocess. Mirrors `start_sync_session` minus the
/// device/serial and history bookkeeping — a scan touches no iPod and writes
/// no history. Shares the guard, cancel/pause/prompt channels, and event
/// relay, so a scan and a sync can never run concurrently.
#[allow(clippy::too_many_arguments)]
fn start_scan_session(
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    registry: &DeviceRegistry,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count_cache: Option<usize>,
) {
    let Ok(session) = state.try_admit_scan() else {
        return;
    };
    broadcast_status(
        event_tx,
        state,
        registry,
        config_path,
        history,
        library_count_cache,
    );

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<(u64, i32)>();
    let (pause_tx, pause_rx) = oneshot::channel::<()>();
    state.install_controls(
        session.id,
        SessionControls::new(cancel_tx, pause_tx, prompt_tx),
    );

    let spawn_scan = spawn_scan.clone();
    let internal_tx = internal_tx.clone();
    let event_context = EventContext::from(&session);
    let session_id = session.id;
    tokio::spawn(async move {
        let outcome = (spawn_scan)(
            String::new(),
            String::new(),
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_context,
        )
        .await;
        let _ = internal_tx.send(InternalEvent::ScanCompleted {
            session_id,
            outcome,
        });
    });
}

fn make_history_entry(
    serial: String,
    session_id: crate::ipc_device::SessionId,
    trigger: SyncTrigger,
    outcome: SyncOutcome,
    error_message: Option<String>,
    summary: Option<SyncSummary>,
    started_at_unix_secs: u64,
    db_restored: bool,
) -> HistoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let duration = now.saturating_sub(started_at_unix_secs);
    HistoryEntry {
        serial,
        session_id: Some(session_id),
        timestamp: crate::daemon::format::rfc3339(now),
        duration_secs: duration,
        trigger,
        outcome,
        error_message,
        summary,
        db_restored,
    }
}

fn unique_configured_serial(registry: &DeviceRegistry) -> Option<String> {
    let configured: Vec<_> = registry
        .records()
        .into_iter()
        .filter(|record| record.configured)
        .map(|record| record.serial)
        .collect();
    match configured.as_slice() {
        [serial] => Some(serial.clone()),
        _ => None,
    }
}

fn persist_terminal_entry(
    state: &mut RuntimeState,
    history: &HistoryService,
    entry: HistoryEntry,
) -> bool {
    match history.append(entry.clone()) {
        Ok(()) => {
            state.clear_retained_terminal_attempt(&entry.serial);
            true
        }
        Err(error) => {
            let message = format!("failed to persist sync history: {error:#}");
            tracing::error!("daemon: {message}");
            state.retain_terminal_attempt(entry, message);
            false
        }
    }
}

/// Map the runtime state to the wire's `status_update.state`. A `Scan`
/// session reports `Scanning`; a real sync reports `Syncing`.
fn state_label(state: &RuntimeState) -> DaemonStateLabel {
    match state.active_session() {
        None => DaemonStateLabel::Idle,
        Some(session) if session.kind == SessionKind::Scan => DaemonStateLabel::Scanning,
        Some(_) => DaemonStateLabel::Syncing,
    }
}

fn broadcast_status(
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &RuntimeState,
    registry: &DeviceRegistry,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count: Option<usize>,
) {
    let _ = event_tx.send(build_status_update(
        state,
        registry,
        config_path,
        history,
        library_count,
        None,
    ));
}

fn build_status_update(
    state: &RuntimeState,
    registry: &DeviceRegistry,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count: Option<usize>,
    acknowledged_request_id: Option<String>,
) -> DaemonEvent {
    let records = registry.records();
    let configured = records.iter().any(|record| record.configured);
    let count_serial = state
        .active_session()
        .and_then(|session| session.serial.clone())
        .or_else(|| {
            records
                .iter()
                .filter(|record| record.configured)
                .find(|record| state.connected_device(&record.serial).is_some())
                .map(|record| record.serial.clone())
        })
        .or_else(|| {
            records
                .iter()
                .find(|record| record.configured)
                .map(|record| record.serial.clone())
        });
    let library_count = count_serial
        .as_deref()
        .and_then(|serial| crate::daemon::library::selected_library_count(config_path, serial))
        .or(library_count);
    let entries = history.read_for_v2_wire();
    DaemonEvent::StatusUpdate {
        state: state_label(state),
        configured,
        ipod_connected: state.connected_devices().next().is_some(),
        last_sync: entries.last().cloned(),
        next_scheduled_unix_secs: None,
        storage: count_serial
            .as_deref()
            .and_then(|serial| state.connected_device(serial))
            .and_then(|device| device_storage::query_storage(&device.drive)),
        synced_count: synced_track_count_at_mount(
            config_path,
            count_serial.as_deref(),
            count_serial
                .as_deref()
                .and_then(|serial| state.connected_device(serial))
                .map(|device| Path::new(&device.drive)),
        ),
        library_count,
        acknowledged_request_id,
    }
}

/// Tracks currently on the iPod per the manifest (X in "X of Y synced").
/// Cheap and always fresh — just a JSON read + `Vec::len()`, no source
/// walk. Falls back to 0 if the manifest path can't be resolved or the
/// manifest doesn't exist yet (nothing synced yet is a legitimate 0, not
/// an error worth surfacing on a status tick).
///
/// PER-DEVICE since the trust package: syncs write
/// `devices/<serial>/manifest.json`, but this counter kept reading the
/// legacy flat path — so the daemon reported `synced_count: 0` for a fully
/// synced iPod and the UI showed "Nothing synced yet" over real content
/// (found live 2026-07-18). `serial` = connected device, falling back to
/// the configured (paired) one. The legacy path is used only by callers that
/// are genuinely unscoped; an explicit serial never inherits another
/// device's pre-migration facts.
pub(crate) fn synced_track_count_at_mount(
    config_path: &Path,
    serial: Option<&str>,
    connected_mount: Option<&Path>,
) -> usize {
    let config_root = device_state_root(config_path);
    let legacy_path = config_root.join("manifest.json");
    let source = configured_source_location_at(config_path);
    synced_track_count_in(
        config_root,
        &legacy_path,
        serial,
        connected_mount,
        source.as_ref(),
    )
}

fn synced_track_count_in(
    config_root: &Path,
    legacy_path: &Path,
    serial: Option<&str>,
    connected_mount: Option<&Path>,
    source: Option<&crate::source_location::SourceLocation>,
) -> usize {
    load_manifest_for_device_in(config_root, legacy_path, serial, connected_mount, source)
        .tracks
        .len()
}

fn load_manifest_for_device_at(
    config_path: &Path,
    serial: Option<&str>,
    connected_mount: Option<&Path>,
) -> crate::manifest::Manifest {
    let config_root = device_state_root(config_path);
    let legacy_path = config_root.join("manifest.json");
    let source = configured_source_location_at(config_path);
    load_manifest_for_device_in(
        config_root,
        &legacy_path,
        serial,
        connected_mount,
        source.as_ref(),
    )
}

fn device_state_root(config_path: &Path) -> &Path {
    config_path.parent().unwrap_or_else(|| Path::new("."))
}

fn load_manifest_for_device_in(
    config_root: &Path,
    legacy_path: &Path,
    serial: Option<&str>,
    connected_mount: Option<&Path>,
    source: Option<&crate::source_location::SourceLocation>,
) -> crate::manifest::Manifest {
    let Some(serial) = serial else {
        return crate::manifest::load_or_default(legacy_path)
            .unwrap_or_else(|_| crate::manifest::Manifest::empty());
    };
    let Some(source) = source else {
        return crate::manifest::Manifest::empty();
    };
    let Ok(host_cache) = crate::device_state::device_manifest_path_in(config_root, serial) else {
        return crate::manifest::Manifest::empty();
    };
    let store = crate::manifest_store::ManifestStore::new(
        connected_mount.unwrap_or(config_root).to_path_buf(),
        serial.to_string(),
        host_cache,
        legacy_path.to_path_buf(),
        crate::atomic_file::AtomicFileWriter::new(),
    );
    let loaded = match connected_mount {
        Some(_) => store.load_device_or_host_cache(source),
        None => store.load_host_cache(source),
    };
    loaded
        .map(|loaded| loaded.manifest)
        .unwrap_or_else(|error| {
            tracing::warn!(
                serial,
                "daemon: failed to load manifest authority: {error:#}"
            );
            crate::manifest::Manifest::empty()
        })
}

fn configured_source_location_at(
    config_path: &Path,
) -> Option<crate::source_location::SourceLocation> {
    let persisted = config_file::load(config_path).ok().flatten()?;
    match (persisted.source, persisted.source_location) {
        (Some(source), Some(location)) if source == location.resolved_path => Some(location),
        (Some(source), _) => crate::source_location::SourceLocation::discover(source.clone())
            .ok()
            .or_else(|| {
                Some(crate::source_location::SourceLocation {
                    resolved_path: source.clone(),
                    identity: crate::source_location::SourceIdentity::Local {
                        library_id: format!("legacy-root:{}", source.display()),
                    },
                })
            }),
        (None, Some(location)) => Some(location),
        (None, None) => None,
    }
}

/// Kick off an off-thread walk of the configured source library to fill the
/// cached `library_count` (Y in "X of Y synced"), delivered back via
/// `InternalEvent::LibraryCountComputed`. No-op when no source is configured.
///
/// Runs on `spawn_blocking` because a large library on a slow/spinning volume
/// can take a while to walk — doing it inline would stall the daemon loop.
/// This is what lets "X of Y" appear on a cold start (fresh daemon, before any
/// sync): the walk fills Y proactively instead of waiting for a sync's diff.
/// Called at startup and after `SaveConfig` (the source path may have changed).
fn spawn_library_count(
    config_path: &std::path::Path,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) {
    let config_path = config_path.to_path_buf();
    let tx = internal_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(count) = count_source_library(&config_path) {
            let _ = tx.send(InternalEvent::LibraryCountComputed { count });
        }
    });
}

/// Resolve the configured source path and count its library tracks (Y in
/// "X of Y synced"), applying the same `*.flac` + skip rules as a real sync
/// (`source::walk`). `None` when no source is configured yet, or the walk
/// failed (e.g. the source volume is unreachable — better to keep the last
/// known Y than to flap). Extracted from `spawn_library_count` so the
/// config-resolve + count logic is unit-testable without a tokio runtime.
fn count_source_library(config_path: &std::path::Path) -> Option<usize> {
    let source = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source)?;
    let started = std::time::Instant::now();
    match crate::source::walk(&source) {
        Ok(entries) => {
            tracing::info!(
                "daemon: counted source library ({} tracks) in {}ms",
                entries.len(),
                started.elapsed().as_millis()
            );
            Some(entries.len())
        }
        Err(e) => {
            tracing::warn!("daemon: source-library count walk failed: {e:#}");
            None
        }
    }
}

/// Handle one client command. Returns `true` iff the daemon should exit
/// its main loop (currently only the Shutdown command sets this — the
/// outer loop then runs the graceful-drain sequence so the in-flight
/// sync subprocess doesn't get yanked mid-write).
fn handle_client_command(
    ClientCommand {
        client_id,
        command,
        reply,
    }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut RuntimeState,
    event_tx: &broadcast::Sender<DaemonEvent>,
    registry: &mut DeviceRegistry,
    spawn_backfill: &SpawnFn,
    spawn_replace_library: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    scheduler: &mut SyncScheduler,
    library_count_cache: &mut Option<usize>,
    config_revision: &mut ConfigRevision,
    source_recovery: &mut SourceRecoveryState,
    source_availability: &SourceAvailabilityService,
) -> bool {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    if let Some(reason) = target_rejection(&command, registry, state) {
        let serial = command
            .target_serial()
            .expect("target rejection requires a targeted command")
            .to_string();
        let request_id = command
            .request_id()
            .expect("targeted v2 commands require request correlation")
            .to_string();
        let _ = reply.send(DaemonEvent::SyncRejected {
            reason,
            serial,
            acknowledged_request_id: request_id,
        });
        tracing::warn!("daemon: client {client_id} rejected exact device target");
        return false;
    }
    let target_serial = command.target_serial().map(|requested| {
        registry
            .record(requested)
            .expect("accepted target requires the exact registry record")
            .serial
            .clone()
    });
    match command {
        DaemonCommand::GetStatus { request_id } => {
            let _ = reply.send(build_status_update(
                state,
                registry,
                config_path,
                history,
                *library_count_cache,
                Some(request_id),
            ));
        }
        DaemonCommand::GetConfig { request_id } => {
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = reply.send(build_config_update(
                cfg,
                registry,
                config_revision.current(),
                Some(request_id),
            ));
        }
        DaemonCommand::SaveConfig {
            source,
            daemon,
            ipod,
            request_id,
        } => {
            let mut current = match config_file::load(config_path) {
                Ok(config) => config.unwrap_or_default(),
                Err(error) => {
                    tracing::error!("daemon: failed to load config before save: {error:#}");
                    return false;
                }
            };
            let before = current.clone();
            if let Some(s) = source {
                if let Err(error) = apply_explicit_source_update(&mut current, PathBuf::from(s)) {
                    tracing::error!("daemon: failed to resolve source identity: {error:#}");
                    let _ = event_tx.send(build_config_update(
                        Some(before),
                        registry,
                        config_revision.current(),
                        Some(request_id),
                    ));
                    return false;
                }
            }
            if let Some(d) = daemon {
                current.daemon = Some(d);
            }
            let source_changed = current.source != before.source
                || current.source_location != before.source_location;
            let global_changed = current != before;
            if global_changed {
                if let Err(e) = config_file::save(config_path, &current) {
                    tracing::error!("daemon: failed to save config: {e}");
                    return false;
                }
                if source_changed {
                    invalidate_source_recovery_for_source_change(source_recovery, event_tx);
                }
            }
            let registry_changed = if let Some(mut identity) = ipod {
                let Some(record) = registry.record(&identity.serial) else {
                    tracing::error!(
                        "daemon: cannot configure unknown device {}",
                        identity.serial
                    );
                    let persisted_revision =
                        config_revision.record_persisted_mutation(global_changed);
                    let _ = event_tx.send(build_config_update(
                        Some(current),
                        registry,
                        persisted_revision,
                        Some(request_id),
                    ));
                    return false;
                };
                identity.serial = record.serial.clone();
                if identity.model_label.is_empty() {
                    identity.model_label = record.model_label.clone();
                }
                if identity.name.is_none() {
                    identity.name = record.name.clone();
                }
                match registry.configure_identity(&identity) {
                    Ok(changed) => changed,
                    Err(error) => {
                        tracing::error!(
                            "daemon: failed to configure device {}: {error:#}",
                            identity.serial
                        );
                        let persisted_revision =
                            config_revision.record_persisted_mutation(global_changed);
                        let _ = event_tx.send(build_config_update(
                            Some(current),
                            registry,
                            persisted_revision,
                            Some(request_id),
                        ));
                        return false;
                    }
                }
            } else {
                false
            };
            let persisted_revision =
                config_revision.record_persisted_mutation(global_changed || registry_changed);
            // Invalidate the cached library count — the source path may
            // have changed, which would make the cached Y stale — then kick
            // off a fresh walk so "X of Y" refreshes without waiting for the
            // next sync. (A sync diff also refreshes it; whichever lands first.)
            *library_count_cache = None;
            spawn_library_count(config_path, internal_tx);
            // Live-reload the scheduled-sync interval. The scheduler is built
            // once at startup, so without this a schedule change in Settings
            // wouldn't take effect until the daemon restarted. Only re-arm on
            // an actual change — rearm() resets the countdown, so re-arming on
            // every save would let frequent edits perpetually postpone a tick.
            let new_minutes = current
                .daemon
                .as_ref()
                .map(|d| d.schedule_minutes)
                .unwrap_or(0);
            if new_minutes != scheduler.minutes() {
                tracing::info!(
                    "daemon: schedule interval {} → {} min; re-arming scheduler",
                    scheduler.minutes(),
                    new_minutes,
                );
                scheduler.rearm(new_minutes);
            }
            let _ = event_tx.send(build_config_update(
                Some(current),
                registry,
                persisted_revision,
                Some(request_id),
            ));
        }
        DaemonCommand::ForgetIpod { serial, request_id } => {
            let connected_device = state.connected_device(&serial).cloned();
            let current = config_file::load(config_path)
                .ok()
                .flatten()
                .unwrap_or_default();
            let raw_serial = registry
                .record(&serial)
                .expect("target guard requires the exact registry record")
                .serial
                .clone();
            if let Err(error) = registry.forget(&raw_serial) {
                tracing::error!("daemon: failed to forget device {raw_serial}: {error:#}");
                return false;
            }
            let persisted_revision = config_revision.record_persisted_mutation(true);
            tracing::info!("daemon: client {client_id} forgot device {raw_serial}");
            let _ = event_tx.send(build_config_update(
                Some(current),
                registry,
                persisted_revision,
                Some(request_id),
            ));
            // Re-announce the currently-attached device (if any) so a
            // freshly-opened wizard sees it. Without this re-emit, the
            // device-watcher's polling loop is in steady-state — the
            // device is still physically connected so no transition
            // event fires, and the wizard's DeviceConnected subscriber
            // waits forever.
            if let Some(device) = connected_device {
                let _ = event_tx.send(DaemonEvent::DeviceConnected {
                    serial: raw_serial,
                    model_label: device.model_label,
                    drive: device.drive,
                    name: device.name,
                });
            }
        }
        DaemonCommand::GetHistory { limit, request_id } => {
            let mut entries = history.read_for_v2_wire();
            let start = entries.len().saturating_sub(limit);
            entries.drain(..start);
            let _ = reply.send(DaemonEvent::HistoryUpdate {
                entries,
                acknowledged_request_id: request_id,
            });
        }
        DaemonCommand::TriggerSync {
            source: trigger_source,
            serial: _,
            request_id: _,
        } => {
            let raw_serial = target_serial
                .as_deref()
                .expect("trigger_sync is serial-targeted");
            let device = state
                .connected_device(raw_serial)
                .expect("target guard requires the exact connected device")
                .clone();
            let trigger = match trigger_source {
                TriggerSource::Manual => SyncTrigger::Manual,
                TriggerSource::Scheduled => SyncTrigger::Scheduled,
                TriggerSource::PlugIn => SyncTrigger::PlugIn,
            };
            let _ = history; // history mutations now happen in handle_internal_event
            request_source_action(
                SourceActionRequest::Sync(PendingSync {
                    trigger,
                    serial: raw_serial.to_string(),
                    drive: device.drive.clone(),
                }),
                source_recovery,
                source_availability,
                event_tx,
                internal_tx,
                config_path,
            );
        }
        DaemonCommand::BackfillRockbox { serial: _, .. } => {
            // Mirrors TriggerSync's guard + spawn + relay path exactly,
            // just pointed at `spawn_backfill` (a `--backfill-rockbox`
            // subprocess) instead of `spawn_sync` (`--apply`).
            // `start_sync_session`'s admission check is what makes a
            // backfill and a sync mutually exclusive — whichever gets
            // there first flips state to Syncing and the other is
            // dropped/no-op.
            let raw_serial = target_serial
                .as_deref()
                .expect("backfill_rockbox is serial-targeted");
            let device = state
                .connected_device(raw_serial)
                .expect("target guard requires the exact connected device")
                .clone();
            tracing::info!(
                "daemon: client {client_id} triggered a Rockbox-compat backfill for {}",
                device.serial
            );
            start_sync_session(
                SyncTrigger::Manual,
                raw_serial.to_string(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_backfill,
                internal_tx,
                config_path,
                *library_count_cache,
            );
        }
        DaemonCommand::ReplaceLibrary {
            serial: _,
            request_id: _,
        } => {
            // Mirrors BackfillRockbox's arm exactly, just pointed at
            // `spawn_replace_library` (a `--replace-library --apply`
            // subprocess) instead of `spawn_backfill`. `start_sync_session`'s
            // admission check is what makes a replace mutually exclusive
            // with a sync/backfill — whichever gets there first flips state
            // to Syncing and the other is dropped/no-op. The UI does its own
            // typed confirmation before ever sending this command, so there's
            // no confirmation prompt to relay here (`--apply` already skips
            // the core's interactive one). Unlike BackfillRockbox/ScanLibrary,
            // this command is destructive (wipes the on-device library), so
            // a busy/no-device guard replies with `SyncRejected` (mirroring
            // TriggerSync's reply mechanism) instead of silently dropping —
            // the UI needs a definitive answer before it can retry or warn.
            let raw_serial = target_serial
                .as_deref()
                .expect("replace_library is serial-targeted");
            let device = state
                .connected_device(raw_serial)
                .expect("target guard requires the exact connected device")
                .clone();
            tracing::info!(
                "daemon: client {client_id} triggered a library replace for {}",
                device.serial
            );
            start_sync_session(
                SyncTrigger::Manual,
                raw_serial.to_string(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_replace_library,
                internal_tx,
                config_path,
                *library_count_cache,
            );
        }
        DaemonCommand::CancelSync { .. } => {
            // Wake the orchestrator's cancel arm. The orchestrator
            // writes a Cancel command to subprocess stdin and force-kills
            // after 5s; the SyncCompleted internal event arrives shortly
            // with outcome = Aborted{reason="user_cancelled"}.
            let active_session_id = state.active_session().map(|session| session.id);
            if let Some(tx) = active_session_id.and_then(|id| state.take_cancel(id)) {
                let _ = tx.send(());
                tracing::info!("daemon: client {client_id} cancelled the running sync");
            } else {
                tracing::debug!(
                    "daemon: client {client_id} sent cancel_sync but no sync is in progress"
                );
            }
        }
        DaemonCommand::Pause { .. } => {
            // Wake the orchestrator's pause arm. Unlike CancelSync, this
            // is graceful — no force-kill; the SyncCompleted internal
            // event arrives once the subprocess has drained, checkpointed,
            // emitted "paused", and exited on its own.
            let active_session_id = state.active_session().map(|session| session.id);
            if let Some(tx) = active_session_id.and_then(|id| state.take_pause(id)) {
                let _ = tx.send(());
                tracing::info!("daemon: client {client_id} requested pause of the running sync");
            } else {
                tracing::debug!("daemon: client {client_id} sent pause but no sync is in progress");
            }
        }
        DaemonCommand::DecidePrompt { id, choice, .. } => {
            // Forward the user's reply to the running sync subprocess.
            // The orchestrator writes the prompt_decision line to
            // stdin; the apply loop's await_prompt then returns the
            // chosen PromptOutcome and the sync proceeds.
            let active_session_id = state.active_session().map(|session| session.id);
            if let Some(tx) = active_session_id.and_then(|id| state.prompt_sender(id)) {
                if tx.send((id, choice)).is_err() {
                    tracing::warn!(
                        "daemon: client {client_id} sent decide_prompt id={id} choice={choice} \
                         but the orchestrator channel was already closed (sync probably ended)"
                    );
                } else {
                    tracing::info!(
                        "daemon: client {client_id} answered prompt id={id} → choice={choice}"
                    );
                }
            } else {
                tracing::debug!(
                    "daemon: client {client_id} sent decide_prompt id={id} but no sync is in progress"
                );
            }
        }
        DaemonCommand::SubscribeDeviceEvents => {
            // All clients see all device events on the shared
            // broadcast channel — subscribe is a handshake, not a
            // routing op. BUT a late subscriber misses any
            // DeviceConnected emitted before the subscribe (e.g. the
            // first-poll event on daemon startup, before the wizard
            // opens). Re-broadcast for any currently-attached device
            // so the late subscriber sees the steady state.
            for device in state.connected_devices() {
                let _ = event_tx.send(DaemonEvent::DeviceConnected {
                    serial: device.serial.clone(),
                    model_label: device.model_label.clone(),
                    drive: device.drive.clone(),
                    name: device.name.clone(),
                });
            }
        }
        DaemonCommand::UnsubscribeDeviceEvents => {
            // Symmetric no-op — subscription is implicit, so there's
            // nothing to tear down.
        }
        DaemonCommand::GetLibrary { request_id } => {
            let _ = reply.send(
                crate::daemon::library::build_library_update(config_path)
                    .with_acknowledged_request_id(Some(request_id)),
            );
        }
        DaemonCommand::ScanLibrary { .. } => {
            if !state.is_idle() {
                tracing::debug!("daemon: client {client_id} sent scan_library while busy; dropped");
                return false;
            }
            let has_source = config_file::load(config_path)
                .ok()
                .flatten()
                .and_then(|c| c.source)
                .is_some();
            if !has_source {
                tracing::debug!("daemon: client {client_id} sent scan_library but no source configured; dropped");
                return false;
            }
            tracing::info!("daemon: client {client_id} triggered a library scan");
            request_source_action(
                SourceActionRequest::Scan,
                source_recovery,
                source_availability,
                event_tx,
                internal_tx,
                config_path,
            );
        }
        DaemonCommand::RetrySourceMount {
            allow_ui,
            request_id,
        } => {
            request_source_action(
                SourceActionRequest::Retry {
                    allow_ui,
                    request_id,
                },
                source_recovery,
                source_availability,
                event_tx,
                internal_tx,
                config_path,
            );
        }
        DaemonCommand::PreviewSelection {
            mode,
            rules,
            serial: _,
            request_id,
        } => {
            let raw_serial = target_serial
                .as_deref()
                .expect("preview_selection is serial-targeted");
            let source = config_file::load(config_path)
                .ok()
                .flatten()
                .and_then(|c| c.source);
            let index = match (source, crate::library_index::default_index_path()) {
                (Some(root), Ok(p)) => crate::library_index::load_or_empty(&p, &root),
                _ => crate::library_index::LibraryIndex::empty(std::path::PathBuf::new()),
            };
            let manifest = load_manifest_for_device_at(
                config_path,
                Some(raw_serial),
                state
                    .connected_device(raw_serial)
                    .map(|device| Path::new(&device.drive)),
            );
            let (selected_tracks, selected_bytes, adds, removes) =
                crate::daemon::library::preview(&index, &manifest, mode, &rules);
            let _ = reply.send(DaemonEvent::SelectionPreview {
                selected_tracks,
                selected_bytes,
                adds,
                removes,
                serial: raw_serial.to_string(),
                acknowledged_request_id: request_id,
            });
        }
        DaemonCommand::ListPlaylists { request_id } => {
            let _ = reply.send(
                build_playlists_update(config_path).with_acknowledged_request_id(Some(request_id)),
            );
        }
        DaemonCommand::GetPlaylist { slug, request_id } => {
            let _ = reply.send(build_playlist_detail(&slug, request_id));
        }
        DaemonCommand::SavePlaylist {
            playlist,
            request_id,
        } => {
            let Ok(store) = open_playlist_store() else {
                tracing::warn!(
                    "daemon: client {client_id} save_playlist: could not open playlist store"
                );
                return false;
            };
            let built = match playlist {
                crate::ipc_daemon::PlaylistPayload::Manual { slug, name, tracks } => {
                    let slug = match slug {
                        Some(s) => s,
                        None => match store.unique_slug(&name) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "daemon: client {client_id} save_playlist: could not allocate a slug for {name:?}: {e:#}"
                                );
                                return false;
                            }
                        },
                    };
                    crate::playlist::Playlist::Manual(crate::playlist::ManualPlaylist {
                        slug,
                        name,
                        tracks: tracks.into_iter().map(PathBuf::from).collect(),
                        skipped_unsafe: 0,
                    })
                }
                crate::ipc_daemon::PlaylistPayload::Smart { slug, name, rules } => {
                    let slug = match slug {
                        Some(s) => s,
                        None => match store.unique_slug(&name) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "daemon: client {client_id} save_playlist: could not allocate a slug for {name:?}: {e:#}"
                                );
                                return false;
                            }
                        },
                    };
                    crate::playlist::Playlist::Smart(crate::playlist::SmartPlaylist {
                        slug,
                        name,
                        rules,
                    })
                }
            };
            if let Err(e) = store.save(&built) {
                tracing::error!("daemon: client {client_id} save_playlist failed: {e:#}");
                return false;
            }
            let _ = event_tx.send(
                build_playlists_update(config_path).with_acknowledged_request_id(Some(request_id)),
            );
        }
        DaemonCommand::DeletePlaylist { slug, request_id } => {
            match open_playlist_store() {
                Ok(store) => {
                    if let Err(e) = store.delete(&slug) {
                        tracing::error!(
                            "daemon: client {client_id} delete_playlist({slug}) failed: {e:#}"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("daemon: client {client_id} delete_playlist: could not open playlist store ({e:#})");
                }
            }
            let _ = event_tx.send(
                build_playlists_update(config_path).with_acknowledged_request_id(Some(request_id)),
            );
        }
        DaemonCommand::GetDeviceConfig {
            serial: _,
            request_id,
        } => {
            let raw_serial = target_serial
                .as_deref()
                .expect("get_device_config is serial-targeted");
            let _ = reply.send(build_device_config_update(
                config_path,
                raw_serial,
                request_id,
            ));
        }
        DaemonCommand::SaveDeviceConfig {
            serial: _,
            selection,
            subscriptions,
            settings,
            request_id,
        } => {
            let raw_serial = target_serial
                .as_deref()
                .expect("save_device_config is serial-targeted");
            let mut selection_changed = false;
            let mut subscriptions_changed = false;
            let mut settings_changed = false;
            if let Some(sel) = selection {
                match crate::selection::effective_device_selection_path_in(
                    device_state_root(config_path),
                    raw_serial,
                ) {
                    Ok(path) => {
                        let full = crate::selection::Selection {
                            version: crate::selection::SELECTION_VERSION,
                            mode: sel.mode,
                            rules: sel.rules,
                        };
                        let changed = crate::selection::load_or_all(&path) != full;
                        if changed {
                            match crate::selection::save_atomic(&path, &full) {
                                Ok(()) => selection_changed = true,
                                Err(e) => tracing::error!(
                                    "daemon: failed to save device selection for {raw_serial}: {e:#}"
                                ),
                            }
                        }
                    }
                    Err(e) => tracing::error!(
                        "daemon: cannot resolve device selection path for {raw_serial}: {e:#}"
                    ),
                }
            }
            if let Some(subs) = subscriptions {
                match crate::device_state::device_subscriptions_path_in(
                    device_state_root(config_path),
                    raw_serial,
                ) {
                    Ok(path) => {
                        let full = crate::device_config::Subscriptions {
                            version: crate::device_config::SUBSCRIPTIONS_VERSION,
                            playlists: subs.playlists,
                        };
                        let changed =
                            crate::device_config::Subscriptions::load_or_default(&path) != full;
                        if changed {
                            match crate::device_config::Subscriptions::save_atomic(&path, &full) {
                                Ok(()) => subscriptions_changed = true,
                                Err(e) => tracing::error!(
                                    "daemon: failed to save subscriptions for {raw_serial}: {e:#}"
                                ),
                            }
                        }
                    }
                    Err(e) => tracing::error!(
                        "daemon: cannot resolve subscriptions path for {raw_serial}: {e:#}"
                    ),
                }
            }
            if let Some(set) = settings {
                match crate::device_state::device_settings_path_in(
                    device_state_root(config_path),
                    raw_serial,
                ) {
                    Ok(path) => {
                        let full = crate::device_config::DeviceSettings {
                            version: crate::device_config::DEVICE_SETTINGS_VERSION,
                            auto_sync: set.auto_sync,
                            rockbox_compat: set.rockbox_compat,
                        };
                        let changed =
                            crate::device_config::DeviceSettings::load_or_default(&path) != full;
                        if changed {
                            match crate::device_config::DeviceSettings::save_atomic(&path, &full) {
                                Ok(()) => settings_changed = true,
                                Err(e) => tracing::error!(
                                    "daemon: failed to save settings for {raw_serial}: {e:#}"
                                ),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "daemon: cannot resolve settings path for {raw_serial}: {e:#}"
                        )
                    }
                }
            }
            if let Err(error) = registry.advance_config_revisions(
                raw_serial,
                selection_changed,
                settings_changed,
                subscriptions_changed,
            ) {
                tracing::error!(
                    "daemon: failed to advance config revisions for {raw_serial}: {error:#}"
                );
            }
            let _ = event_tx.send(build_device_config_update(
                config_path,
                raw_serial,
                request_id,
            ));
            broadcast_status(
                event_tx,
                state,
                registry,
                config_path,
                history,
                *library_count_cache,
            );
        }
        DaemonCommand::PreviewDevice {
            serial: _,
            request_id,
        } => {
            let raw_serial = target_serial
                .as_deref()
                .expect("preview_device is serial-targeted");
            let _ = reply.send(build_device_preview(
                config_path,
                state,
                raw_serial,
                request_id,
            ));
        }
        DaemonCommand::ResolveTracks { rules, request_id } => {
            // Synchronous inline reply, same as PreviewDevice above — no
            // spawn/await before sending, so replies stay in request order
            // for the client's FIFO correlation.
            let index = load_cached_index(config_path);
            let tracks = crate::daemon::library::resolve_tracks(&index, &rules);
            let _ = reply.send(DaemonEvent::ResolvedTracks {
                tracks,
                acknowledged_request_id: request_id,
            });
        }
        DaemonCommand::Shutdown => {
            tracing::info!("daemon: shutdown requested by client {client_id}; exiting loop");
            return true;
        }
    }
    false
}

fn build_config_update(
    cfg: Option<PersistedConfig>,
    registry: &DeviceRegistry,
    config_revision: u64,
    acknowledged_request_id: Option<String>,
) -> DaemonEvent {
    let ipod = registry
        .records()
        .into_iter()
        .find(|record| record.configured)
        .map(|record| crate::config_file::IpodIdentity {
            serial: record.serial,
            model_label: record.model_label,
            name: record.name,
            custom_selection: true,
        });
    match cfg {
        Some(c) => DaemonEvent::ConfigUpdate {
            source: c.source.map(|p| p.display().to_string()),
            daemon: c.daemon,
            ipod,
            config_revision,
            acknowledged_request_id,
        },
        None => DaemonEvent::ConfigUpdate {
            source: None,
            daemon: None,
            ipod,
            config_revision,
            acknowledged_request_id,
        },
    }
}

/// The cached library index for the configured source, same pattern as the
/// inline load in `PreviewSelection`'s arm — an empty index (root `""`)
/// when there's no source configured yet or the cache file is missing.
fn load_cached_index(config_path: &std::path::Path) -> crate::library_index::LibraryIndex {
    let source = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source);
    match (source, crate::library_index::default_index_path()) {
        (Some(root), Ok(p)) => crate::library_index::load_or_empty(&p, &root),
        _ => crate::library_index::LibraryIndex::empty(PathBuf::new()),
    }
}

/// Open (creating on demand) the one playlist store at its default root.
fn open_playlist_store() -> Result<crate::playlist::PlaylistStore> {
    crate::playlist::PlaylistStore::default_root().and_then(crate::playlist::PlaylistStore::open)
}

/// `list_playlists` reply / `save_playlist`+`delete_playlist` broadcast
/// payload: every playlist in the store, summarized against the cached
/// index. A store-open failure (e.g. an unwritable config dir) degrades to
/// an empty list with a logged warning rather than failing the arm.
fn build_playlists_update(config_path: &std::path::Path) -> DaemonEvent {
    let index = load_cached_index(config_path);
    match open_playlist_store() {
        Ok(store) => DaemonEvent::PlaylistsUpdate {
            playlists: crate::daemon::library::build_playlist_summaries(&store, &index),
            acknowledged_request_id: None,
        },
        Err(e) => {
            tracing::warn!(
                "daemon: playlists: failed to open store ({e:#}); replying with an empty list"
            );
            DaemonEvent::PlaylistsUpdate {
                playlists: Vec::new(),
                acknowledged_request_id: None,
            }
        }
    }
}

/// `get_playlist` reply: this playlist's full content — the manual track
/// list or the smart rule set — for the editor, unlike
/// `build_playlists_update`'s per-list summaries (track count only). An
/// unopenable store, a missing slug, or an on-disk file that fails to
/// parse all reply with `error` set (and every content field `None`)
/// rather than degrading to an empty result — see the `PlaylistDetail`
/// and `GetPlaylist` doc comments in `ipc_daemon.rs`.
fn build_playlist_detail(slug: &str, acknowledged_request_id: String) -> DaemonEvent {
    let store = match open_playlist_store() {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!("daemon: get_playlist({slug}): could not open playlist store ({e:#})");
            return DaemonEvent::PlaylistDetail {
                slug: slug.to_string(),
                name: None,
                kind: None,
                tracks: None,
                rules: None,
                error: Some(format!("could not open playlist store: {e:#}")),
                acknowledged_request_id,
            };
        }
    };
    match store.load(slug) {
        Ok(Some(crate::playlist::Playlist::Manual(m))) => DaemonEvent::PlaylistDetail {
            slug: m.slug,
            name: Some(m.name),
            kind: Some(PlaylistKind::Manual),
            tracks: Some(
                m.tracks
                    .into_iter()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .collect(),
            ),
            rules: None,
            error: None,
            acknowledged_request_id,
        },
        Ok(Some(crate::playlist::Playlist::Smart(s))) => DaemonEvent::PlaylistDetail {
            slug: s.slug,
            name: Some(s.name),
            kind: Some(PlaylistKind::Smart),
            tracks: None,
            rules: Some(s.rules),
            error: None,
            acknowledged_request_id,
        },
        Ok(None) => {
            tracing::warn!("daemon: get_playlist({slug}): no such playlist");
            DaemonEvent::PlaylistDetail {
                slug: slug.to_string(),
                name: None,
                kind: None,
                tracks: None,
                rules: None,
                error: Some("no such playlist".to_string()),
                acknowledged_request_id,
            }
        }
        Err(e) => {
            tracing::warn!("daemon: get_playlist({slug}): failed to read ({e:#})");
            DaemonEvent::PlaylistDetail {
                slug: slug.to_string(),
                name: None,
                kind: None,
                tracks: None,
                rules: None,
                error: Some(format!("{e:#}")),
                acknowledged_request_id,
            }
        }
    }
}

/// `get_device_config` reply / `save_device_config` broadcast payload: one
/// device's resolved selection + subscriptions + settings. Every part
/// fails open to its type's default (see `selection::load_or_all`,
/// `Subscriptions::load_or_default`, `DeviceSettings::load_or_migrate`) —
/// this never fails the arm, even for a `serial` the daemon has never seen.
fn build_device_config_update(
    config_path: &std::path::Path,
    serial: &str,
    acknowledged_request_id: String,
) -> DaemonEvent {
    let root = device_state_root(config_path);
    let selection = crate::selection::effective_device_selection_path_in(root, serial)
        .map(|p| crate::selection::load_or_all(&p))
        .unwrap_or_else(|_| crate::selection::Selection::all());
    let subscriptions = crate::device_state::device_subscriptions_path_in(root, serial)
        .map(|p| crate::device_config::Subscriptions::load_or_default(&p))
        .unwrap_or_default();
    let global = config_file::load(config_path)
        .ok()
        .flatten()
        .unwrap_or_default();
    let settings = crate::device_config::DeviceSettings::load_or_migrate_in(root, serial, &global);
    DaemonEvent::DeviceConfigUpdate {
        serial: serial.to_string(),
        selection: crate::ipc_daemon::SelectionPayload {
            mode: selection.mode,
            rules: selection.rules,
        },
        subscriptions: crate::ipc_daemon::SubscriptionsPayload {
            playlists: subscriptions.playlists,
        },
        settings: crate::ipc_daemon::DeviceSettingsPayload {
            auto_sync: settings.auto_sync,
            rockbox_compat: settings.rockbox_compat,
        },
        acknowledged_request_id,
    }
}

/// `preview_device` reply: gathers this device's cached index + selection +
/// subscriptions + playlist store, plus a live free-bytes baseline (only
/// when `serial` is the device currently connected), and hands off to the
/// pure `daemon::library::compute_device_preview`.
fn build_device_preview(
    config_path: &std::path::Path,
    state: &RuntimeState,
    serial: &str,
    acknowledged_request_id: String,
) -> DaemonEvent {
    let index = load_cached_index(config_path);
    let root = device_state_root(config_path);
    let selection = crate::selection::effective_device_selection_path_in(root, serial)
        .map(|p| crate::selection::load_or_all(&p))
        .unwrap_or_else(|_| crate::selection::Selection::all());
    let subs = crate::device_state::device_subscriptions_path_in(root, serial)
        .map(|p| crate::device_config::Subscriptions::load_or_default(&p))
        .unwrap_or_default();
    let store = open_playlist_store();
    if let Err(e) = &store {
        tracing::warn!("daemon: preview_device({serial}): failed to open playlist store ({e:#}); playlist subscriptions ignored");
    }
    let current_free_bytes = state
        .connected_device(serial)
        .and_then(|d| device_storage::query_storage(&d.drive))
        .map(|s| s.free_bytes);
    // What's already on the device, from its manifest — so the projection
    // only counts genuinely-new bytes (a fully-synced selection projects no
    // change instead of a phantom "pending" band on the capacity bar).
    let already_synced: std::collections::HashSet<PathBuf> = load_manifest_for_device_at(
        config_path,
        Some(serial),
        state
            .connected_device(serial)
            .map(|device| Path::new(&device.drive)),
    )
    .tracks
    .into_iter()
    .map(|track| track.source_path)
    .collect();
    crate::daemon::library::compute_device_preview(
        &index,
        &selection,
        &subs,
        store.as_ref().ok(),
        current_free_bytes,
        &already_synced,
        serial,
        acknowledged_request_id,
    )
}

fn history_file_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA unavailable"))?
        .join(crate::PROJECT_DIR);
    Ok(base.join("history.json"))
}

// Suppress the unused-import warning when the test build doesn't take this path.
#[allow(dead_code)]
fn _silence_mpsc_warning(_: mpsc::Sender<DaemonEvent>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::{IpodIdentity, PersistedConfig};
    use crate::ipc_daemon::SyncRejectReason;
    use crate::ipod::device::DetectedIpod;

    fn detected(serial: &str) -> DetectedIpod {
        DetectedIpod {
            serial: serial.to_string(),
            model_label: "iPod Classic".to_string(),
            drive: "/Volumes/IPOD".to_string(),
            name: None,
            volume_guid: None,
        }
    }

    fn registry(serials: &[&str]) -> DeviceRegistry {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "classick-runtime-registry-{}-{n}-{}.json",
            std::process::id(),
            serials.join("-")
        ));
        let _ = std::fs::remove_file(&path);
        let legacy = IpodIdentity {
            serial: serials[0].to_string(),
            model_label: "iPod Classic".to_string(),
            name: None,
            custom_selection: false,
        };
        let mut registry = DeviceRegistry::load_or_migrate(path, Some(&legacy)).unwrap();
        for serial in &serials[1..] {
            registry.observe(&detected(serial), 1).unwrap();
            registry.configure(serial).unwrap();
        }
        registry
    }

    #[test]
    fn destructive_replace_rejects_a_different_connected_target() {
        let command = DaemonCommand::ReplaceLibrary {
            serial: "RAW-B".to_string(),
            request_id: "replace-b".to_string(),
        };
        let mut state = RuntimeState::new();
        state.connect(detected(" raw-a "));
        let registry = registry(&["RAW-A"]);

        assert_eq!(
            target_rejection(&command, &registry, &state),
            Some(SyncRejectReason::NotConfigured),
            "replace_library for B must never wipe the currently connected A"
        );
    }

    #[test]
    fn active_controls_only_target_the_syncing_device() {
        let mut state = RuntimeState::new();
        state
            .try_admit_device(
                SyncTrigger::Manual,
                "0xRAW-A",
                std::path::Path::new("/Volumes/IPOD"),
            )
            .unwrap();
        let registry = registry(&["0xRAW-A", "RAW-B"]);
        let wrong = DaemonCommand::CancelSync {
            serial: "RAW-B".to_string(),
            request_id: "cancel-b".to_string(),
        };
        let same_canonical = DaemonCommand::CancelSync {
            serial: "raw-a".to_string(),
            request_id: "cancel-a".to_string(),
        };

        assert_eq!(
            target_rejection(&wrong, &registry, &state),
            Some(SyncRejectReason::AlreadySyncing)
        );
        assert_eq!(target_rejection(&same_canonical, &registry, &state), None);
    }

    #[test]
    fn stale_name_resolution_from_old_connection_cannot_overwrite_reconnected_device() {
        let mut state = RuntimeState::new();
        let mut first = detected("RAW-A");
        first.drive = "/Volumes/OLD".to_string();
        state.connect(first);
        let old_generation = state.connection_generation("RAW-A").unwrap();
        let mut reconnected = detected("RAW-A");
        reconnected.drive = "/Volumes/NEW".to_string();
        reconnected.name = Some("New attachment".to_string());
        state.connect(reconnected);
        let mut registry = registry(&["RAW-A"]);
        let (event_tx, _) = broadcast::channel(8);
        let history = HistoryService::new(std::env::temp_dir().join(format!(
            "classick-stale-name-history-{}.json",
            std::process::id()
        )));
        let config_path = std::env::temp_dir().join(format!(
            "classick-stale-name-config-{}.toml",
            std::process::id()
        ));
        let mut library_count = None;
        let mut revision = ConfigRevision::default();

        handle_internal_event(
            InternalEvent::IpodNameResolved {
                serial: "RAW-A".to_string(),
                connection_generation: old_generation,
                name: Some("Old attachment".to_string()),
            },
            &mut state,
            &event_tx,
            &history,
            &mut registry,
            &config_path,
            &mut library_count,
            &mut revision,
        );

        assert_eq!(
            state.connected_device("RAW-A").unwrap().name.as_deref(),
            Some("New attachment")
        );
        assert_ne!(
            registry.record("RAW-A").unwrap().name.as_deref(),
            Some("Old attachment")
        );
    }

    #[test]
    fn unresolved_ipod_name_does_not_erase_the_cached_registry_name() {
        let mut named = detected("RAW-A");
        named.name = Some("Michael's iPod".to_string());
        let mut state = RuntimeState::new();
        state.connect(named.clone());
        let generation = state.connection_generation("RAW-A").unwrap();
        let mut registry = registry(&["RAW-A"]);
        registry.observe(&named, 1).unwrap();
        let (event_tx, _) = broadcast::channel(8);
        let history = HistoryService::new(std::env::temp_dir().join(format!(
            "classick-unresolved-name-history-{}.json",
            std::process::id()
        )));
        let config_path = std::env::temp_dir().join(format!(
            "classick-unresolved-name-config-{}.toml",
            std::process::id()
        ));
        let mut library_count = None;
        let mut revision = ConfigRevision::default();

        handle_internal_event(
            InternalEvent::IpodNameResolved {
                serial: "RAW-A".to_string(),
                connection_generation: generation,
                name: None,
            },
            &mut state,
            &event_tx,
            &history,
            &mut registry,
            &config_path,
            &mut library_count,
            &mut revision,
        );

        assert_eq!(
            state.connected_device("RAW-A").unwrap().name.as_deref(),
            Some("Michael's iPod")
        );
        assert_eq!(
            registry.record("RAW-A").unwrap().name.as_deref(),
            Some("Michael's iPod")
        );
    }

    #[test]
    fn forget_only_targets_the_configured_device() {
        let command = DaemonCommand::ForgetIpod {
            serial: "RAW-B".to_string(),
            request_id: "forget-b".to_string(),
        };

        assert_eq!(
            target_rejection(&command, &registry(&["RAW-A"]), &RuntimeState::new()),
            Some(SyncRejectReason::NotConfigured)
        );
    }

    #[test]
    fn every_serial_targeted_command_rejects_an_unknown_registry_target() {
        let commands = vec![
            DaemonCommand::ForgetIpod {
                serial: "RAW-B".into(),
                request_id: "1".into(),
            },
            DaemonCommand::TriggerSync {
                source: TriggerSource::Manual,
                serial: "RAW-B".into(),
                request_id: "2".into(),
            },
            DaemonCommand::CancelSync {
                serial: "RAW-B".into(),
                request_id: "3".into(),
            },
            DaemonCommand::Pause {
                serial: "RAW-B".into(),
                request_id: "4".into(),
            },
            DaemonCommand::DecidePrompt {
                id: 7,
                choice: 1,
                serial: "RAW-B".into(),
                request_id: "5".into(),
            },
            DaemonCommand::BackfillRockbox {
                serial: "RAW-B".into(),
                request_id: "6".into(),
            },
            DaemonCommand::ReplaceLibrary {
                serial: "RAW-B".into(),
                request_id: "7".into(),
            },
            DaemonCommand::PreviewSelection {
                mode: crate::selection::SelectionMode::All,
                rules: vec![],
                serial: "RAW-B".into(),
                request_id: "8".into(),
            },
            DaemonCommand::GetDeviceConfig {
                serial: "RAW-B".into(),
                request_id: "9".into(),
            },
            DaemonCommand::SaveDeviceConfig {
                serial: "RAW-B".into(),
                selection: None,
                subscriptions: None,
                settings: None,
                request_id: "10".into(),
            },
            DaemonCommand::PreviewDevice {
                serial: "RAW-B".into(),
                request_id: "11".into(),
            },
        ];
        let mut state = RuntimeState::new();
        state
            .try_admit_device(
                SyncTrigger::Manual,
                "RAW-A",
                std::path::Path::new("/Volumes/IPOD"),
            )
            .unwrap();
        let registry = registry(&["RAW-A"]);

        for command in commands {
            assert!(
                target_rejection(&command, &registry, &state).is_some(),
                "must reject mismatched target: {command:?}"
            );
        }
    }

    #[test]
    fn config_revision_is_monotonic_and_changes_only_after_persist() {
        let mut revision = ConfigRevision::default();
        assert_eq!(revision.current(), 0);
        assert_eq!(
            revision.record_persisted_mutation(false),
            0,
            "reads and failed/no-op paths do not advance"
        );
        assert_eq!(revision.advance_after_persist(), 1);
        assert_eq!(revision.advance_after_persist(), 2);
    }

    #[test]
    fn config_update_acknowledgements_distinguish_pre_and_post_mutation_order() {
        let mut revision = ConfigRevision::default();
        let registry = registry(&["RAW-A"]);
        let before = build_config_update(None, &registry, revision.current(), Some("read".into()));
        let persisted = revision.record_persisted_mutation(true);
        let after = build_config_update(None, &registry, persisted, Some("save".into()));

        assert!(matches!(
            before,
            DaemonEvent::ConfigUpdate {
                config_revision: 0,
                acknowledged_request_id: Some(id),
                ..
            } if id == "read"
        ));
        assert!(matches!(
            after,
            DaemonEvent::ConfigUpdate {
                config_revision: 1,
                acknowledged_request_id: Some(id),
                ..
            } if id == "save"
        ));
    }

    #[test]
    fn history_entries_keep_the_syncing_devices_raw_serial() {
        let entry = make_history_entry(
            " A ".to_string(),
            42,
            SyncTrigger::Manual,
            SyncOutcome::Ok,
            None,
            None,
            0,
            false,
        );

        assert_eq!(entry.serial, " A ");
        assert_eq!(entry.session_id, Some(42));
    }

    #[test]
    fn legacy_history_has_an_authority_only_with_one_configured_device() {
        assert_eq!(
            unique_configured_serial(&registry(&["RAW-A"])),
            Some("RAW-A".to_string())
        );
        assert_eq!(
            unique_configured_serial(&registry(&["RAW-A", "RAW-B"])),
            None
        );

        let mut unconfigured = registry(&["RAW-A"]);
        unconfigured.forget("RAW-A").unwrap();
        assert_eq!(unique_configured_serial(&unconfigured), None);
    }

    fn manifest_with_track_count(count: usize) -> crate::manifest::Manifest {
        let tracks = (0..count)
            .map(|i| crate::manifest::ManifestEntry {
                source_path: PathBuf::from(format!("/music/{i}.flac")),
                source_mtime: 0,
                source_size: 1,
                source_fingerprint: format!("fp-{i}"),
                ipod_dbid: i as u64 + 1,
                ipod_relpath: format!("iPod_Control/Music/F00/{i}.m4a"),
                source_known: true,
                audio_fingerprint: String::new(),
                encoder: "unknown".to_string(),
                encoder_version: String::new(),
                source_format: "flac".to_string(),
            })
            .collect();
        crate::manifest::Manifest {
            version: 1,
            ipod_serial: None,
            last_source_root: None,
            tracks,
        }
    }

    #[test]
    fn explicit_serial_manifest_reads_never_fall_back_to_legacy() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let root = std::env::temp_dir().join(format!(
            "classick-synced-count-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let legacy_path = root.join("manifest.json");
        crate::manifest::save_atomic(&legacy_path, &manifest_with_track_count(1)).unwrap();
        let device_path = crate::device_state::device_manifest_path_in(&root, "SERIAL-1").unwrap();
        crate::manifest::save_atomic(&device_path, &manifest_with_track_count(3)).unwrap();
        let source = crate::source_location::SourceLocation {
            resolved_path: PathBuf::from("/music"),
            identity: crate::source_location::SourceIdentity::Local {
                library_id: "library-test".into(),
            },
        };

        assert_eq!(
            synced_track_count_in(&root, &legacy_path, Some("SERIAL-1"), None, Some(&source),),
            3
        );
        assert_eq!(
            synced_track_count_in(&root, &legacy_path, None, None, Some(&source)),
            1
        );

        let preview_manifest =
            load_manifest_for_device_in(&root, &legacy_path, Some("SERIAL-1"), None, Some(&source));
        assert_eq!(preview_manifest.tracks.len(), 3);
        assert_eq!(
            synced_track_count_in(&root, &legacy_path, Some("SERIAL-B"), None, Some(&source),),
            0,
            "missing B state must not inherit legacy synced facts"
        );
        assert!(
            load_manifest_for_device_in(
                &root,
                &legacy_path,
                Some("SERIAL-B"),
                None,
                Some(&source),
            )
            .tracks
            .is_empty(),
            "explicit B preview must be empty when B has no manifest"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connected_preview_uses_device_authority_and_disconnected_preview_uses_cache() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let root = std::env::temp_dir().join(format!(
            "classick-authority-preview-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mount = root.join("mount");
        std::fs::create_dir_all(&mount).unwrap();
        let legacy_path = root.join("manifest.json");
        let source = crate::source_location::SourceLocation {
            resolved_path: root.join("music"),
            identity: crate::source_location::SourceIdentity::Local {
                library_id: "library-test".into(),
            },
        };
        let mut host = manifest_with_track_count(2);
        host.last_source_root = Some(source.resolved_path.clone());
        for (index, track) in host.tracks.iter_mut().enumerate() {
            track.source_path = source.resolved_path.join(format!("{index}.flac"));
        }
        let mut device = manifest_with_track_count(5);
        device.last_source_root = Some(source.resolved_path.clone());
        for (index, track) in device.tracks.iter_mut().enumerate() {
            track.source_path = source.resolved_path.join(format!("{index}.flac"));
        }
        let host_path = crate::device_state::device_manifest_path_in(&root, "SERIAL-1").unwrap();
        crate::atomic_file::AtomicFileWriter::new()
            .write(&host_path, &host.encode_v2(&source, "SERIAL-1").unwrap())
            .unwrap();
        crate::atomic_file::AtomicFileWriter::new()
            .write(
                &crate::device_state::portable_manifest_path(&mount),
                &device.encode_v2(&source, "SERIAL-1").unwrap(),
            )
            .unwrap();

        let connected = load_manifest_for_device_in(
            &root,
            &legacy_path,
            Some("SERIAL-1"),
            Some(&mount),
            Some(&source),
        );
        let disconnected =
            load_manifest_for_device_in(&root, &legacy_path, Some("SERIAL-1"), None, Some(&source));

        assert_eq!(connected.tracks.len(), 5);
        assert_eq!(disconnected.tracks.len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_label_maps_scan_sessions_to_scanning() {
        let mut sm = RuntimeState::new();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Idle));
        let scan = sm.try_admit_scan().unwrap();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Scanning));
        sm.finish(scan.id);
        sm.try_admit_device(
            SyncTrigger::Manual,
            "RAW-A",
            std::path::Path::new("/Volumes/IPOD"),
        )
        .unwrap();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Syncing));
    }

    // should_auto_sync is the trivial pure seam over per-device settings —
    // migration/seeding from the global `enabled` flag is covered by
    // device_config.rs's `load_or_migrate` tests, not re-tested here.
    #[test]
    fn should_auto_sync_follows_device_setting() {
        use crate::device_config::DeviceSettings;
        assert!(should_auto_sync(&DeviceSettings {
            auto_sync: true,
            ..DeviceSettings::default()
        }));
        assert!(
            !should_auto_sync(&DeviceSettings {
                auto_sync: false,
                ..DeviceSettings::default()
            }),
            "auto-sync must be off when the device setting is off"
        );
    }

    #[test]
    fn pending_source_action_coalesces_scan_and_keeps_one_sync() {
        let first = PendingSync {
            trigger: SyncTrigger::Manual,
            serial: "RAW-A".into(),
            drive: "/Volumes/IPOD-A".into(),
        };
        let second = PendingSync {
            trigger: SyncTrigger::Scheduled,
            serial: "RAW-B".into(),
            drive: "/Volumes/IPOD-B".into(),
        };
        let mut pending = PendingSourceAction::default();

        pending.request_scan();
        pending.request_scan();
        pending.request_sync(first.clone());
        pending.request_sync(second);

        assert!(pending.scan_requested);
        assert_eq!(pending.sync, Some(first));
    }

    #[test]
    fn source_mount_interaction_only_allows_ui_for_explicit_true() {
        assert_eq!(
            mount_interaction(false),
            crate::daemon::source_availability::MountInteraction::SuppressUi
        );
        assert_eq!(
            mount_interaction(true),
            crate::daemon::source_availability::MountInteraction::AllowUi
        );
    }

    #[test]
    fn alternate_mount_persists_legacy_and_logical_source_together() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-recovery-persist-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let config_path = base.join("config.toml");
        let old = PathBuf::from("/Volumes/data/media/music");
        let new = PathBuf::from("/Volumes/data-1/media/music");
        let location = crate::source_location::SourceLocation {
            resolved_path: old.clone(),
            identity: crate::source_location::SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("media/music").unwrap()),
            },
        };
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(old),
                source_location: Some(location.clone()),
                ..Default::default()
            },
        )
        .unwrap();

        persist_resolved_source(&config_path, &location, &new).unwrap();

        let saved = crate::config_file::load(&config_path)
            .unwrap()
            .expect("saved config");
        assert_eq!(saved.source.as_deref(), Some(new.as_path()));
        assert_eq!(
            saved
                .source_location
                .as_ref()
                .map(|source| source.resolved_path.as_path()),
            Some(new.as_path())
        );
        assert_eq!(saved.source_location.unwrap().identity, location.identity);
        let _ = std::fs::remove_dir_all(base);
    }

    fn completed_spawn(counter: Arc<std::sync::atomic::AtomicUsize>) -> SpawnFn {
        Arc::new(move |_, _, _, _, _, _| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(OrchestratorOutcome::Completed {
                    outcome: SyncOutcome::Ok,
                    summary: None,
                    db_restored: false,
                })
            })
        })
    }

    #[tokio::test]
    async fn existing_local_source_admits_sync_without_mount_recovery() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-local-immediate-{}",
            std::process::id()
        ));
        let source = base.join("music");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source.clone()),
                source_location: Some(crate::source_location::SourceLocation {
                    resolved_path: source,
                    identity: crate::source_location::SourceIdentity::Local {
                        library_id: "local-test".into(),
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let mut state = RuntimeState::new();
        let mut recovery = SourceRecoveryState::default();
        let availability = SourceAvailabilityService::platform_default();
        let (event_tx, _) = broadcast::channel(16);
        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
        let sync_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scan_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawn_sync = completed_spawn(sync_count.clone());
        let spawn_scan = completed_spawn(scan_count.clone());
        let registry = registry(&["RAW-A"]);
        let history = HistoryService::new(base.join("history.json"));

        request_source_action(
            SourceActionRequest::Sync(PendingSync {
                trigger: SyncTrigger::Manual,
                serial: "RAW-A".into(),
                drive: "/Volumes/IPOD".into(),
            }),
            &mut recovery,
            &availability,
            &event_tx,
            &internal_tx,
            &config_path,
        );

        assert!(matches!(state_label(&state), DaemonStateLabel::Idle));
        let resolved = internal_rx.recv().await.unwrap();
        let InternalEvent::SourceAvailabilityResolved { attempt_id, result } = resolved else {
            panic!("unexpected internal event")
        };
        let (mut watcher, _watcher_rx) =
            crate::daemon::library_watcher::LibraryWatcher::spawn(None);
        let mut deadline = None;
        let mut revision = ConfigRevision::default();
        complete_source_recovery(
            attempt_id,
            result,
            &mut recovery,
            &availability,
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            Some(11),
            &mut revision,
        );

        assert!(matches!(state_label(&state), DaemonStateLabel::Syncing));
        tokio::task::yield_now().await;
        assert_eq!(scan_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(sync_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(recovery.sync_after_scan.is_none());
        assert!(recovery.in_flight_attempt.is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn existing_replacement_smb_path_cannot_start_scan_or_sync() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-replacement-identity-{}",
            std::process::id()
        ));
        let source = base.join("music");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source.clone()),
                source_location: Some(crate::source_location::SourceLocation {
                    resolved_path: source,
                    identity: crate::source_location::SourceIdentity::Smb {
                        host: "jupiter".into(),
                        share: "data".into(),
                        subpath: None,
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let mut state = RuntimeState::new();
        let mut recovery = SourceRecoveryState::default();
        let availability = SourceAvailabilityService::platform_default();
        let (event_tx, mut event_rx) = broadcast::channel(16);
        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel();
        let sync_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scan_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawn_sync = completed_spawn(sync_count.clone());
        let spawn_scan = completed_spawn(scan_count.clone());
        let registry = registry(&["RAW-A"]);
        let history = HistoryService::new(base.join("history.json"));

        request_source_action(
            SourceActionRequest::Sync(PendingSync {
                trigger: SyncTrigger::Manual,
                serial: "RAW-A".into(),
                drive: "/Volumes/IPOD".into(),
            }),
            &mut recovery,
            &availability,
            &event_tx,
            &internal_tx,
            &config_path,
        );

        assert!(matches!(state_label(&state), DaemonStateLabel::Idle));
        let resolved = tokio::time::timeout(std::time::Duration::from_secs(5), internal_rx.recv())
            .await
            .expect("identity verification result")
            .expect("internal channel open");
        let InternalEvent::SourceAvailabilityResolved { attempt_id, result } = resolved else {
            panic!("unexpected internal event")
        };
        assert_eq!(result, Err(SourceUnavailable::IdentityMismatch));

        let (mut watcher, _watcher_rx) =
            crate::daemon::library_watcher::LibraryWatcher::spawn(None);
        let mut deadline = None;
        let mut revision = ConfigRevision::default();
        complete_source_recovery(
            attempt_id,
            result,
            &mut recovery,
            &availability,
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            None,
            &mut revision,
        );

        assert_eq!(sync_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(scan_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(matches!(state_label(&state), DaemonStateLabel::Idle));
        assert!(matches!(
            event_rx.try_recv(),
            Ok(DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::Remounting,
                ..
            })
        ));
        assert!(matches!(
            event_rx.try_recv(),
            Ok(DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::Unavailable,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn retry_waits_for_in_flight_completion_when_source_appears() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-in-flight-appeared-{}",
            std::process::id()
        ));
        let source = base.join("music");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source.clone()),
                source_location: Some(crate::source_location::SourceLocation {
                    resolved_path: source,
                    identity: crate::source_location::SourceIdentity::Local {
                        library_id: "local-test".into(),
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let state = RuntimeState::new();
        let mut recovery = SourceRecoveryState {
            in_flight_attempt: Some(7),
            in_flight_interaction: Some(
                crate::daemon::source_availability::MountInteraction::SuppressUi,
            ),
            ..SourceRecoveryState::default()
        };
        let (event_tx, mut event_rx) = broadcast::channel(8);
        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        request_source_action(
            SourceActionRequest::Retry {
                allow_ui: true,
                request_id: "retry".into(),
            },
            &mut recovery,
            &SourceAvailabilityService::platform_default(),
            &event_tx,
            &internal_tx,
            &config_path,
        );

        assert_eq!(recovery.in_flight_attempt, Some(7));
        assert_eq!(recovery.retry_request_ids, ["retry"]);
        assert!(recovery.ui_escalation_pending);
        assert!(matches!(state_label(&state), DaemonStateLabel::Idle));
        assert!(event_rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn recovered_alternate_mount_rearms_once_and_scans_before_pending_sync() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-recovery-flow-{}",
            std::process::id()
        ));
        let recovered = base.join("Volumes/data-1/media/music");
        std::fs::create_dir_all(&recovered).unwrap();
        let config_path = base.join("config.toml");
        let location = crate::source_location::SourceLocation {
            resolved_path: base.join("missing/media/music"),
            identity: crate::source_location::SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("media/music").unwrap()),
            },
        };
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(location.resolved_path.clone()),
                source_location: Some(location),
                ..Default::default()
            },
        )
        .unwrap();
        let mut state = RuntimeState::new();
        let mut recovery = SourceRecoveryState::default();
        recovery.in_flight_attempt = Some(7);
        recovery.in_flight_interaction =
            Some(crate::daemon::source_availability::MountInteraction::SuppressUi);
        recovery.pending.request_scan();
        recovery.pending.request_sync(PendingSync {
            trigger: SyncTrigger::Manual,
            serial: "RAW-A".into(),
            drive: "/Volumes/IPOD".into(),
        });
        let (event_tx, _) = broadcast::channel(16);
        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let scan_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawn_scan = completed_spawn(scan_count.clone());
        let spawn_sync = completed_spawn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let registry = registry(&["RAW-A"]);
        let history = HistoryService::new(base.join("history.json"));
        let (mut watcher, mut watcher_rx) =
            crate::daemon::library_watcher::LibraryWatcher::spawn(None);
        let mut deadline = Some(tokio::time::Instant::now());
        let mut revision = ConfigRevision::default();

        complete_source_recovery(
            7,
            Ok(ResolvedSource {
                root: recovered.clone(),
                remounted: true,
            }),
            &mut recovery,
            &SourceAvailabilityService::platform_default(),
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            Some(37),
            &mut revision,
        );
        complete_source_recovery(
            7,
            Ok(ResolvedSource {
                root: recovered.clone(),
                remounted: true,
            }),
            &mut recovery,
            &SourceAvailabilityService::platform_default(),
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            Some(37),
            &mut revision,
        );

        tokio::task::yield_now().await;
        assert_eq!(scan_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(state_label(&state), DaemonStateLabel::Scanning));
        assert!(recovery.sync_after_scan.is_some());
        assert!(deadline.is_none());
        assert_eq!(revision.current(), 1);
        let saved = crate::config_file::load(&config_path).unwrap().unwrap();
        assert_eq!(saved.source.as_deref(), Some(recovered.as_path()));
        assert_eq!(saved.source_location.unwrap().resolved_path, recovered);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::fs::write(recovered.join("watch.flac"), b"x").unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), watcher_rx.recv())
                .await
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn failed_recovery_keeps_cached_library_count_and_index() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-recovery-cache-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let index_path = base.join("library-index.json");
        std::fs::write(&index_path, b"cached-index").unwrap();
        let count = Some(91usize);
        let config_path = base.join("config.toml");
        let missing_source = base.join("missing");
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(missing_source.clone()),
                source_location: Some(crate::source_location::SourceLocation {
                    resolved_path: missing_source,
                    identity: crate::source_location::SourceIdentity::Local {
                        library_id: "unavailable-local-test".into(),
                    },
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let mut recovery = SourceRecoveryState::default();
        recovery.in_flight_attempt = Some(4);
        recovery.in_flight_interaction =
            Some(crate::daemon::source_availability::MountInteraction::SuppressUi);
        recovery.retry_request_ids.push("retry-failed".into());
        let (event_tx, mut event_rx) = broadcast::channel(4);
        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let spawn_sync = completed_spawn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let spawn_scan = completed_spawn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let registry = registry(&["RAW-A"]);
        let history = HistoryService::new(base.join("history.json"));
        let (mut watcher, _watcher_rx) =
            crate::daemon::library_watcher::LibraryWatcher::spawn(None);
        let mut deadline = None;
        let mut state = RuntimeState::new();
        let mut revision = ConfigRevision::default();

        complete_source_recovery(
            4,
            Err(SourceUnavailable::MountFailed(
                "smb://alice:secret@jupiter/data".into(),
            )),
            &mut recovery,
            &SourceAvailabilityService::platform_default(),
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            count,
            &mut revision,
        );

        assert_eq!(count, Some(91));
        assert_eq!(std::fs::read(&index_path).unwrap(), b"cached-index");
        assert!(matches!(
            event_rx.try_recv(),
            Ok(DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::Unavailable,
                source_root: None,
                acknowledged_request_id: Some(request_id),
            }) if request_id == "retry-failed"
        ));
        assert!(recovery.pending.sync.is_none());
        assert_eq!(revision.current(), 0);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn coalesced_explicit_retries_each_receive_one_terminal_ack() {
        let mut recovery = SourceRecoveryState::default();
        recovery.retry_request_ids = vec!["req-a".into(), "req-b".into()];
        let (event_tx, mut event_rx) = broadcast::channel(4);

        publish_source_availability(
            &mut recovery,
            &event_tx,
            SourceAvailabilityState::AuthRequired,
            None,
        );

        let mut acknowledged = [event_rx.try_recv().unwrap(), event_rx.try_recv().unwrap()]
            .into_iter()
            .map(|event| match event {
                DaemonEvent::SourceAvailability {
                    state: SourceAvailabilityState::AuthRequired,
                    source_root: None,
                    acknowledged_request_id: Some(request_id),
                } => request_id,
                other => panic!("unexpected terminal event: {other:?}"),
            })
            .collect::<Vec<_>>();
        acknowledged.sort();
        assert_eq!(acknowledged, ["req-a", "req-b"]);
        assert!(recovery.retry_request_ids.is_empty());
    }

    #[test]
    fn failed_startup_source_state_is_replayable_to_a_new_client() {
        let recovery = SourceRecoveryState {
            current_state: SourceAvailabilityState::AuthRequired,
            available_root: None,
            ..SourceRecoveryState::default()
        };

        assert!(matches!(
            source_availability_replay(&recovery),
            DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::AuthRequired,
                source_root: None,
                acknowledged_request_id: None,
            }
        ));
    }

    #[test]
    fn explicit_ui_retries_behind_suppressed_mount_escalate_once_after_auth() {
        let mut recovery = SourceRecoveryState {
            in_flight_attempt: Some(8),
            in_flight_interaction: Some(
                crate::daemon::source_availability::MountInteraction::SuppressUi,
            ),
            ..SourceRecoveryState::default()
        };

        recovery.record_retry(true, "req-a".into());
        recovery.record_retry(true, "req-b".into());

        assert!(recovery.take_ui_escalation_after_auth(
            crate::daemon::source_availability::MountInteraction::SuppressUi
        ));
        assert!(!recovery.take_ui_escalation_after_auth(
            crate::daemon::source_availability::MountInteraction::SuppressUi
        ));
        assert_eq!(recovery.retry_request_ids, ["req-a", "req-b"]);
    }

    #[test]
    fn non_auth_terminal_clears_pending_ui_escalation() {
        let mut recovery = SourceRecoveryState {
            in_flight_attempt: Some(8),
            in_flight_interaction: Some(
                crate::daemon::source_availability::MountInteraction::SuppressUi,
            ),
            ..SourceRecoveryState::default()
        };
        recovery.record_retry(true, "req-a".into());
        let (event_tx, _event_rx) = broadcast::channel(4);

        publish_source_availability(
            &mut recovery,
            &event_tx,
            SourceAvailabilityState::Unavailable,
            None,
        );

        assert!(!recovery.ui_escalation_pending);
    }

    #[test]
    fn source_change_invalidates_the_old_mount_attempt_and_terminally_acks_retries() {
        let mut recovery = SourceRecoveryState {
            in_flight_attempt: Some(8),
            in_flight_interaction: Some(
                crate::daemon::source_availability::MountInteraction::SuppressUi,
            ),
            ui_escalation_pending: true,
            retry_request_ids: vec!["retry-a".into()],
            current_state: SourceAvailabilityState::Remounting,
            ..SourceRecoveryState::default()
        };
        let (event_tx, mut event_rx) = broadcast::channel(4);

        invalidate_source_recovery_for_source_change(&mut recovery, &event_tx);

        assert!(recovery.in_flight_attempt.is_none());
        assert!(recovery.in_flight_interaction.is_none());
        assert!(!recovery.ui_escalation_pending);
        assert!(matches!(
            event_rx.try_recv(),
            Ok(DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::Unavailable,
                acknowledged_request_id: Some(request_id),
                ..
            }) if request_id == "retry-a"
        ));
    }

    #[tokio::test]
    async fn auth_from_suppressed_attempt_launches_one_allow_ui_attempt_without_acking() {
        #[derive(Clone, Default)]
        struct PendingBackend {
            interactions:
                Arc<std::sync::Mutex<Vec<crate::daemon::source_availability::MountInteraction>>>,
        }
        impl crate::daemon::source_availability::SourceMountBackend for PendingBackend {
            fn mount<'a>(
                &'a self,
                _location: &'a crate::source_location::SourceLocation,
                interaction: crate::daemon::source_availability::MountInteraction,
            ) -> crate::daemon::source_availability::BoxFuture<
                'a,
                std::result::Result<PathBuf, SourceUnavailable>,
            > {
                self.interactions.lock().unwrap().push(interaction);
                Box::pin(std::future::pending())
            }
        }

        let base =
            std::env::temp_dir().join(format!("classick-source-escalation-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let config_path = base.join("config.toml");
        let location = crate::source_location::SourceLocation {
            resolved_path: base.join("missing/media/music"),
            identity: crate::source_location::SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("media/music").unwrap()),
            },
        };
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(location.resolved_path.clone()),
                source_location: Some(location),
                ..Default::default()
            },
        )
        .unwrap();
        let backend = PendingBackend::default();
        let availability = SourceAvailabilityService::new(Arc::new(backend.clone()));
        let mut recovery = SourceRecoveryState {
            in_flight_attempt: Some(1),
            in_flight_interaction: Some(
                crate::daemon::source_availability::MountInteraction::SuppressUi,
            ),
            next_attempt_id: 2,
            ..SourceRecoveryState::default()
        };
        recovery.record_retry(true, "req-a".into());
        recovery.record_retry(true, "req-b".into());
        let (event_tx, mut event_rx) = broadcast::channel(8);
        let (internal_tx, _internal_rx) = mpsc::unbounded_channel();
        let spawn_sync = completed_spawn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let spawn_scan = completed_spawn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let registry = registry(&["RAW-A"]);
        let history = HistoryService::new(base.join("history.json"));
        let (mut watcher, _watcher_rx) =
            crate::daemon::library_watcher::LibraryWatcher::spawn(None);
        let mut deadline = None;
        let mut state = RuntimeState::new();
        let mut revision = ConfigRevision::default();

        complete_source_recovery(
            1,
            Err(SourceUnavailable::AuthRequired),
            &mut recovery,
            &availability,
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            Some(5),
            &mut revision,
        );
        tokio::task::yield_now().await;

        assert_eq!(recovery.in_flight_attempt, Some(2));
        assert_eq!(
            recovery.in_flight_interaction,
            Some(crate::daemon::source_availability::MountInteraction::AllowUi)
        );
        assert_eq!(recovery.retry_request_ids, ["req-a", "req-b"]);
        assert_eq!(
            *backend.interactions.lock().unwrap(),
            [crate::daemon::source_availability::MountInteraction::AllowUi]
        );
        assert!(matches!(
            event_rx.try_recv(),
            Ok(DaemonEvent::SourceAvailability {
                state: SourceAvailabilityState::Remounting,
                acknowledged_request_id: None,
                ..
            })
        ));

        complete_source_recovery(
            1,
            Err(SourceUnavailable::AuthRequired),
            &mut recovery,
            &availability,
            &mut watcher,
            &mut deadline,
            &mut state,
            &event_tx,
            &spawn_sync,
            &spawn_scan,
            &internal_tx,
            &registry,
            &config_path,
            &history,
            Some(5),
            &mut revision,
        );
        assert_eq!(recovery.in_flight_attempt, Some(2));
        assert!(event_rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn explicit_source_change_replaces_share_a_identity_with_share_b() {
        let mut config = PersistedConfig {
            source: Some(PathBuf::from(r"\\jupiter\data\media\music")),
            source_location: Some(
                crate::source_location::SourceLocation::discover(PathBuf::from(
                    r"\\jupiter\data\media\music",
                ))
                .unwrap(),
            ),
            ..Default::default()
        };

        apply_explicit_source_update(&mut config, PathBuf::from(r"\\saturn\archive\lossless"))
            .unwrap();

        assert_eq!(
            config.source_location.unwrap().identity,
            crate::source_location::SourceIdentity::Smb {
                host: "saturn".into(),
                share: "archive".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("lossless").unwrap()),
            }
        );
    }

    #[test]
    fn failed_explicit_source_discovery_leaves_config_unchanged() {
        let mut config = PersistedConfig {
            source: Some(PathBuf::from(r"\\jupiter\data\media\music")),
            source_location: Some(
                crate::source_location::SourceLocation::discover(PathBuf::from(
                    r"\\jupiter\data\media\music",
                ))
                .unwrap(),
            ),
            ..Default::default()
        };
        let before = config.clone();

        assert!(apply_explicit_source_update(&mut config, PathBuf::from(r"\\broken")).is_err());
        assert_eq!(config, before);
    }

    #[test]
    fn stale_persisted_identity_is_not_reused_for_a_changed_source() {
        let base = std::env::temp_dir().join(format!(
            "classick-stale-source-identity-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let config_path = base.join("config.toml");
        crate::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(PathBuf::from(r"\\saturn\archive\lossless")),
                source_location: Some(
                    crate::source_location::SourceLocation::discover(PathBuf::from(
                        r"\\jupiter\data\media\music",
                    ))
                    .unwrap(),
                ),
                ..Default::default()
            },
        )
        .unwrap();

        let resolved = configured_source_location_at(&config_path).unwrap();

        assert_eq!(
            resolved.identity,
            crate::source_location::SourceIdentity::Smb {
                host: "saturn".into(),
                share: "archive".into(),
                subpath: Some(crate::portable_path::PortablePath::parse("lossless").unwrap()),
            }
        );
        let _ = std::fs::remove_dir_all(base);
    }

    // Cold-start "X of Y": count_source_library resolves the config's source
    // and counts flac tracks with the same skip rules as a real sync, so Y is
    // known before any sync runs. Returns None when no source is configured.
    #[test]
    fn count_source_library_counts_flac_respecting_skip_rules() {
        use std::fs;
        let base = std::env::temp_dir().join(format!("classick-libcount-{}", std::process::id()));
        let src = base.join("music");
        fs::create_dir_all(&src).unwrap();
        for n in ["a.flac", "b.flac", "c.flac"] {
            fs::write(src.join(n), b"x").unwrap();
        }
        fs::write(src.join("notes.txt"), b"x").unwrap(); // not flac → ignored
        fs::create_dir_all(src.join("_excluded")).unwrap();
        fs::write(src.join("_excluded/skip.flac"), b"x").unwrap(); // skipped dir → ignored

        let cfg_path = base.join("config.toml");
        let cfg = PersistedConfig {
            source: Some(src.clone()),
            ..Default::default()
        };
        crate::config_file::save(&cfg_path, &cfg).unwrap();
        assert_eq!(count_source_library(&cfg_path), Some(3));

        // No source configured → None (Y stays unknown, menu shows "X synced").
        let empty_path = base.join("empty.toml");
        crate::config_file::save(&empty_path, &PersistedConfig::default()).unwrap();
        assert_eq!(count_source_library(&empty_path), None);

        let _ = fs::remove_dir_all(&base);
    }
}
