//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, device watcher, scheduler, and sync orchestrator.
//!
//! M3 scope: real auto-sync on configured-iPod plug-in, Sync Now via
//! manual TriggerSync, periodic Scheduled triggers from the scheduler,
//! and live DeviceConnected/Disconnected broadcasts. Test-only entry
//! `run_daemon_with_deps` exists so the integration suite can inject
//! a scripted device watcher and a fake spawn-fn.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::device_storage::{self, StorageInfo};
use crate::daemon::device_watcher::{Debouncer, DeviceEvent, DeviceWatcher, PollingDeviceWatcher};
use crate::daemon::history::{HistoryEntry, HistoryService, SyncOutcome, SyncSummary, SyncTrigger};
use crate::daemon::ipc_server::{spawn_server, ClientCommand};
use crate::daemon::scheduler::SyncScheduler;
use crate::daemon::state::{DaemonState, StateMachine, TriggerOutcome};
use crate::daemon::sync_orchestrator::{self, OrchestratorOutcome};
use crate::ipc_daemon::{
    DaemonCommand, DaemonEvent, DaemonStateLabel, SyncRejectReason, TriggerSource,
};
use crate::ipod::device::DetectedIpod;
use anyhow::Result;
use std::path::PathBuf;
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
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(crate::daemon::BROADCAST_CHANNEL_CAPACITY);
    let exe = std::env::current_exe()?;
    let event_tx_for_spawn = event_tx.clone();
    let spawn_sync: SpawnFn = Arc::new(move |drive: String, cancel_rx: oneshot::Receiver<()>| {
        let exe = exe.clone();
        let event_tx = event_tx_for_spawn.clone();
        Box::pin(async move {
            sync_orchestrator::run(exe, drive, cancel_rx, event_tx).await
        })
    });

    let deps = DaemonDeps {
        configured_serial,
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        schedule_minutes,
        preset_event_tx: Some(event_tx),
    };
    run_daemon_with_deps(deps).await
}

/// Async closure that runs one sync to completion. Arc-wrapped so the
/// runtime can clone it into a tokio::spawn'd task without consuming the
/// daemon's only copy.
pub type SpawnFn = Arc<
    dyn Fn(String, oneshot::Receiver<()>) -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
        + Send
        + Sync,
>;

pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    pub schedule_minutes: u32,
    /// If Some, the runtime uses this pre-built sender instead of
    /// constructing its own. Production passes the same one it gave
    /// to the spawn_sync closure so orchestrator events broadcast on
    /// the same channel UI clients subscribe to.
    pub preset_event_tx: Option<broadcast::Sender<DaemonEvent>>,
}

/// Internal events posted from background sync tasks back to the runtime
/// loop. The runtime owns state + history; the spawned task only does
/// the actual sync work and ships its outcome here for state-machine
/// mutation + history persistence + broadcast.
enum InternalEvent {
    SyncCompleted {
        trigger: SyncTrigger,
        serial: String,
        started_at_unix_secs: u64,
        outcome: Result<OrchestratorOutcome>,
    },
    /// Sent by the iPod-name reader task after itdb_parse completes on
    /// a freshly-plugged device. The runtime applies the name to
    /// `connected` (if the serial still matches the current device),
    /// persists it into config so the UI sees it across daemon
    /// restarts, then re-broadcasts a DeviceConnected + ConfigUpdate
    /// so the popover updates immediately.
    IpodNameResolved {
        serial: String,
        name: Option<String>,
    },
}

/// Test-friendly entry. Production wraps real impls and calls this.
pub async fn run_daemon_with_deps(deps: DaemonDeps) -> Result<()> {
    let history_path = history_file_path()?;
    let history = HistoryService::new(history_path);
    let config_path = config_file::default_path()?;
    let mut state = StateMachine::new();
    let mut scheduler = SyncScheduler::new(deps.schedule_minutes);
    let mut debouncer = Debouncer::new(crate::daemon::DEVICE_DEBOUNCE_WINDOW);
    let mut connected: Option<DetectedIpod> = None;
    let configured_serial = deps.configured_serial;

    let (event_tx, mut cmd_rx, mut new_client_rx) = match deps.preset_event_tx {
        Some(tx) => crate::daemon::ipc_server::spawn_server_full(tx).await?,
        None => {
            let (tx, rx) = spawn_server().await?;
            // Test path: synthesize an empty new-client channel that
            // never fires. The integration tests don't exercise snapshot
            // semantics; production goes through spawn_server_full.
            let (_dummy_tx, dummy_rx) = mpsc::unbounded_channel::<()>();
            (tx, rx, dummy_rx)
        }
    };
    let mut device_rx = deps.watcher.start();
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let spawn_sync = deps.spawn_sync;
    // Cancellation signal for the currently-running sync (if any). Set
    // by start_sync_session; taken + sent by handle_client_command's
    // CancelSync arm; cleared in handle_internal_event after completion.
    let mut cancel_tx_holder: Option<oneshot::Sender<()>> = None;
    let mut configured_serial = configured_serial;

    tracing::info!("daemon: ready (configured_serial={configured_serial:?})");

    let exit_reason: ExitReason = loop {
        tokio::select! {
            biased;

            client_cmd = cmd_rx.recv() => {
                let Some(client_cmd) = client_cmd else {
                    tracing::info!("daemon: command channel closed; exiting");
                    return Ok(());
                };
                let should_exit = handle_client_command(
                    client_cmd,
                    &history,
                    &config_path,
                    &mut state,
                    &event_tx,
                    &connected,
                    &spawn_sync,
                    &internal_tx,
                    &mut cancel_tx_holder,
                    &mut configured_serial,
                );
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
                    &mut connected,
                    &event_tx,
                    &mut state,
                    &history,
                    &spawn_sync,
                    &internal_tx,
                    &mut cancel_tx_holder,
                    configured_serial.as_deref(),
                    &config_path,
                );
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }

            Some(internal) = internal_rx.recv() => {
                // Only the SyncCompleted variant clears the cancel-tx
                // holder. IpodNameResolved is unrelated to sync lifecycle.
                let is_sync_completion = matches!(internal, InternalEvent::SyncCompleted { .. });
                if is_sync_completion { cancel_tx_holder = None; }
                handle_internal_event(internal, &mut state, &event_tx, &history, &mut connected, &config_path);
            }

            Some(()) = new_client_rx.recv() => {
                // A fresh UI connected. Publish a snapshot StatusUpdate
                // so the new subscriber's tray + popover initialize
                // with current state, even if earlier broadcasts (e.g.
                // DeviceConnected from polling at daemon startup) went
                // out before they subscribed.
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }

            _ = scheduler.tick() => {
                // Scheduled syncs also honour the user's auto/manual choice;
                // schedule_minutes is moot when the user opted into manual.
                if connected.is_some() && state.is_idle() && auto_sync_enabled(&config_path) {
                    if let Some(drive) = connected.as_ref().map(|d| d.drive.clone()) {
                        start_sync_session(
                            SyncTrigger::Scheduled,
                            connected.as_ref().unwrap().serial.clone(),
                            drive,
                            &mut state,
                            &event_tx,
                            &spawn_sync,
                            &internal_tx,
                            &mut cancel_tx_holder,
                        );
                    }
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
            if cancel_tx_holder.is_none() && state.is_idle() {
                tracing::info!("daemon: clean shutdown — no in-flight sync to drain");
            } else {
                if let Some(tx) = cancel_tx_holder.take() {
                    let _ = tx.send(());
                    tracing::info!("daemon: signalled in-flight sync to cancel before exit");
                }
                let drain = tokio::time::timeout(
                    crate::daemon::SHUTDOWN_DRAIN_BUDGET,
                    async {
                        while let Some(internal) = internal_rx.recv().await {
                            if matches!(internal, InternalEvent::SyncCompleted { .. }) {
                                return;
                            }
                        }
                    },
                ).await;
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
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    history: &HistoryService,
    connected: &mut Option<DetectedIpod>,
    config_path: &std::path::Path,
) {
    match event {
        InternalEvent::IpodNameResolved { serial, name } => {
            // Apply only if the resolved-name's serial still matches
            // the currently-connected device. (Device could've been
            // detached during the iTunesDB parse.)
            let Some(c) = connected.as_mut() else { return };
            if c.serial != serial { return }
            if c.name == name { return }
            c.name = name.clone();

            // Persist the name into the user's IpodIdentity so it
            // survives daemon restarts (read_ipod_name is a 100-500ms
            // op we don't want to repeat unnecessarily).
            if let Ok(Some(mut cfg)) = config_file::load(config_path) {
                if let Some(id) = cfg.ipod_identity.as_mut() {
                    if id.serial == serial && id.name != name {
                        id.name = name.clone();
                        if let Err(e) = config_file::save(config_path, &cfg) {
                            tracing::warn!("daemon: failed to persist iPod name to config: {e}");
                        }
                    }
                }
            }

            // Re-broadcast DeviceConnected with the now-populated
            // name, and a ConfigUpdate so the popover/title bar
            // refreshes from either path.
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: c.serial.clone(),
                model_label: c.model_label.clone(),
                drive: c.drive.clone(),
                name: c.name.clone(),
            });
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = event_tx.send(build_config_update(cfg));
            return;
        }
        InternalEvent::SyncCompleted { trigger, serial, started_at_unix_secs, outcome } => {
            // If the device was detached mid-sync, the Disconnected handler
            // already wrote an Aborted history entry and called finish_sync.
            // In that case state is Idle and serial != this sync's serial —
            // silently drop this completion to avoid a duplicate history entry.
            if state.is_idle() {
                tracing::debug!("daemon: sync completion arrived but state is already Idle (likely device-detached mid-sync); ignoring");
                return;
            }

            let (history_outcome, error_message, summary) = match outcome {
                Ok(OrchestratorOutcome::Completed { outcome: SyncOutcome::Ok, summary }) => {
                    (SyncOutcome::Ok, None, summary)
                }
                Ok(OrchestratorOutcome::Completed { outcome, summary }) => {
                    (outcome, Some("sync subprocess reported failure".to_string()), summary)
                }
                Ok(OrchestratorOutcome::Aborted { reason, summary }) => {
                    (SyncOutcome::Aborted, Some(reason), summary)
                }
                Err(e) => {
                    (SyncOutcome::Error, Some(format!("orchestrator: {e:#}")), None)
                }
            };

            let entry = make_history_entry(
                trigger, history_outcome, error_message, summary, started_at_unix_secs,
            );
            let last_sync = Some(entry.clone());
            let _ = history.append(entry);
            state.finish_sync();

            let _ = serial;  // recorded in history via trigger context above
            let _ = event_tx.send(DaemonEvent::StatusUpdate {
                state: DaemonStateLabel::Idle,
                configured: true,
                ipod_connected: connected.is_some(),
                last_sync,
                next_scheduled_unix_secs: None,
                storage: current_storage(connected),
            });
        }
    }
}

/// True when `daemon.subsequent_sync_mode` is set to `auto_apply` (or the
/// config simply isn't there yet, in which case the daemon's
/// `DaemonSettings::default()` already chooses `AutoApply`). When the user
/// has explicitly picked Manual mode in the wizard or settings, plug-in
/// and scheduled triggers no-op and the only way to start a sync is the
/// tray's Sync Now action.
fn auto_sync_enabled(config_path: &std::path::Path) -> bool {
    let Some(cfg) = config_file::load(config_path).ok().flatten() else { return true };
    let Some(daemon) = cfg.daemon else { return true };
    daemon.subsequent_sync_mode == config_file::SyncMode::AutoApply
}

fn handle_device_event(
    event: DeviceEvent,
    connected: &mut Option<DetectedIpod>,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut StateMachine,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    configured_serial: Option<&str>,
    config_path: &std::path::Path,
) {
    match event {
        DeviceEvent::Connected(mut ipod) => {
            // Seed name from persisted config so the UI doesn't flash
            // "iPod (Classic 7G)" then snap to "Michael's iPod" — if
            // we read the name on a previous plug-in we already have it.
            if ipod.name.is_none() {
                if let Ok(Some(cfg)) = config_file::load(config_path) {
                    if let Some(id) = cfg.ipod_identity.as_ref() {
                        if id.serial == ipod.serial { ipod.name = id.name.clone(); }
                    }
                }
            }

            *connected = Some(ipod.clone());
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: ipod.serial.clone(),
                model_label: ipod.model_label.clone(),
                drive: ipod.drive.clone(),
                name: ipod.name.clone(),
            });

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
                    name,
                });
            });

            // Auto-sync only fires for the configured serial AND when the
            // user has opted into automatic mode. Manual mode means they
            // want to drive sync explicitly via the tray's Sync Now action.
            if configured_serial == Some(ipod.serial.as_str())
                && state.is_idle()
                && auto_sync_enabled(config_path)
            {
                start_sync_session(
                    SyncTrigger::PlugIn,
                    ipod.serial.clone(),
                    ipod.drive.clone(),
                    state,
                    event_tx,
                    spawn_sync,
                    internal_tx,
                    cancel_tx_holder,
                );
            }
        }
        DeviceEvent::Disconnected { serial } => {
            *connected = None;
            let _ = event_tx.send(DaemonEvent::DeviceDisconnected { serial: serial.clone() });
            // If the device we were syncing disappeared, force-finish
            // the session with Aborted. The spawned orchestrator task
            // is still running — its SyncCompleted will arrive later and
            // be silently dropped (handle_internal_event checks for the
            // already-Idle case).
            if let DaemonState::Syncing(s) = state.state() {
                if s.serial.as_deref() == Some(&serial) {
                    let _ = history.append(make_history_entry(
                        s.trigger.clone(),
                        SyncOutcome::Aborted,
                        Some("device_detached".to_string()),
                        None,
                        s.started_at_unix_secs,
                    ));
                    state.finish_sync();
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
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
) {
    if state.try_start_sync_for_device(trigger.clone(), serial.clone(), drive.clone())
        != TriggerOutcome::Accepted
    {
        return;
    }
    let started_at_unix_secs = match state.state() {
        DaemonState::Syncing(s) => s.started_at_unix_secs,
        _ => 0,
    };
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: DaemonStateLabel::Syncing,
        configured: true,
        ipod_connected: true,
        last_sync: None,
        next_scheduled_unix_secs: None,
        storage: device_storage::query_storage(&drive),
    });

    // Per-sync cancel channel. Sender is held by the runtime so the
    // CancelSync IPC command can wake the orchestrator; Receiver is
    // passed into the spawn closure.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    *cancel_tx_holder = Some(cancel_tx);

    let spawn_sync = spawn_sync.clone();
    let internal_tx = internal_tx.clone();
    let drive_for_task = drive.clone();
    let trigger_for_task = trigger.clone();
    let serial_for_task = serial.clone();
    tokio::spawn(async move {
        let outcome = (spawn_sync)(drive_for_task, cancel_rx).await;
        let _ = internal_tx.send(InternalEvent::SyncCompleted {
            trigger: trigger_for_task,
            serial: serial_for_task,
            started_at_unix_secs,
            outcome,
        });
    });
}

fn make_history_entry(
    trigger: SyncTrigger,
    outcome: SyncOutcome,
    error_message: Option<String>,
    summary: Option<SyncSummary>,
    started_at_unix_secs: u64,
) -> HistoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let duration = now.saturating_sub(started_at_unix_secs);
    HistoryEntry {
        timestamp: crate::daemon::format::rfc3339(now),
        duration_secs: duration,
        trigger,
        outcome,
        error_message,
        summary,
    }
}

fn broadcast_status(
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &StateMachine,
    connected: &Option<DetectedIpod>,
    config_path: &std::path::Path,
    history: &HistoryService,
) {
    let configured = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.ipod_identity)
        .is_some();
    let state_label = match state.state() {
        DaemonState::Idle => DaemonStateLabel::Idle,
        DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
    };
    let entries = history.read();
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: state_label,
        configured,
        ipod_connected: connected.is_some(),
        last_sync: entries.last().cloned(),
        next_scheduled_unix_secs: None,
        storage: current_storage(connected),
    });
}

/// Query free + total bytes for the connected iPod's drive. `None` when
/// no device is connected OR when the volume query failed (drive may
/// have been unplugged mid-tick). UI treats absence as "no info yet".
fn current_storage(connected: &Option<DetectedIpod>) -> Option<StorageInfo> {
    connected
        .as_ref()
        .and_then(|d| device_storage::query_storage(&d.drive))
}

/// Handle one client command. Returns `true` iff the daemon should exit
/// its main loop (currently only the Shutdown command sets this — the
/// outer loop then runs the graceful-drain sequence so the in-flight
/// sync subprocess doesn't get yanked mid-write).
fn handle_client_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    connected: &Option<DetectedIpod>,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    configured_serial: &mut Option<String>,
) -> bool {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    match command {
        DaemonCommand::GetStatus => {
            let configured = configured_serial.is_some();
            let state_label = match state.state() {
                DaemonState::Idle => DaemonStateLabel::Idle,
                DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
            };
            let entries = history.read();
            let _ = reply.send(DaemonEvent::StatusUpdate {
                state: state_label,
                configured,
                ipod_connected: connected.is_some(),
                last_sync: entries.last().cloned(),
                next_scheduled_unix_secs: None,
                storage: current_storage(connected),
            });
        }
        DaemonCommand::GetConfig => {
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = reply.send(build_config_update(cfg));
        }
        DaemonCommand::SaveConfig { source, daemon, ipod } => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            if let Some(s) = source { current.source = Some(PathBuf::from(s)); }
            if let Some(d) = daemon { current.daemon = Some(d); }
            if let Some(mut i) = ipod {
                // Wizard / settings clients don't know the iPod's
                // firmware name — preserve it across saves so the user
                // doesn't lose "Michael's iPod" the moment they re-run
                // the wizard for the same device. Only carry it over
                // when serials match (different iPod = clean slate).
                if i.name.is_none() {
                    if let Some(prev) = current.ipod_identity.as_ref() {
                        if prev.serial == i.serial { i.name = prev.name.clone(); }
                    }
                }
                current.ipod_identity = Some(i);
            }
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config: {e}");
                return false;
            }
            // Mirror the persisted ipod identity into the in-memory
            // `configured_serial` so subsequent plug-in / TriggerSync
            // checks see the post-wizard state without needing a daemon
            // restart. Without this, the wizard's first SaveConfig is
            // invisible to the daemon's auto-sync gate.
            *configured_serial = current.ipod_identity.as_ref().map(|id| id.serial.clone());
            let _ = event_tx.send(build_config_update(Some(current)));
        }
        DaemonCommand::ForgetIpod => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            current.ipod_identity = None;
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config after forget_ipod: {e}");
                return false;
            }
            *configured_serial = None;
            tracing::info!("daemon: client {client_id} cleared the persisted iPod identity");
            let _ = event_tx.send(build_config_update(Some(current)));
            // Re-announce the currently-attached device (if any) so a
            // freshly-opened wizard sees it. Without this re-emit, the
            // device-watcher's polling loop is in steady-state — the
            // device is still physically connected so no transition
            // event fires, and the wizard's DeviceConnected subscriber
            // waits forever.
            if let Some(device) = connected.as_ref() {
                let _ = event_tx.send(DaemonEvent::DeviceConnected {
                    serial: device.serial.clone(),
                    model_label: device.model_label.clone(),
                    drive: device.drive.clone(),
                    name: device.name.clone(),
                });
            }
        }
        DaemonCommand::GetHistory { limit } => {
            let mut entries = history.read();
            let start = entries.len().saturating_sub(limit);
            entries.drain(..start);
            let _ = reply.send(DaemonEvent::HistoryUpdate { entries });
        }
        DaemonCommand::TriggerSync { source: trigger_source } => {
            if !state.is_idle() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::AlreadySyncing,
                });
                return false;
            }
            let Some(device) = connected.as_ref() else {
                let _ = reply.send(DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod });
                return false;
            };
            if configured_serial.is_none() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::NotConfigured,
                });
                return false;
            }
            let trigger = match trigger_source {
                TriggerSource::Manual => SyncTrigger::Manual,
                TriggerSource::Scheduled => SyncTrigger::Scheduled,
                TriggerSource::PlugIn => SyncTrigger::PlugIn,
            };
            let _ = history;  // history mutations now happen in handle_internal_event
            start_sync_session(
                trigger,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_sync,
                internal_tx,
                cancel_tx_holder,
            );
        }
        DaemonCommand::CancelSync => {
            // Wake the orchestrator's cancel arm. The orchestrator
            // writes a Cancel command to subprocess stdin and force-kills
            // after 5s; the SyncCompleted internal event arrives shortly
            // with outcome = Aborted{reason="user_cancelled"}.
            if let Some(tx) = cancel_tx_holder.take() {
                let _ = tx.send(());
                tracing::info!("daemon: client {client_id} cancelled the running sync");
            } else {
                tracing::debug!("daemon: client {client_id} sent cancel_sync but no sync is in progress");
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
            if let Some(device) = connected.as_ref() {
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
        DaemonCommand::Shutdown => {
            tracing::info!("daemon: shutdown requested by client {client_id}; exiting loop");
            return true;
        }
    }
    false
}

fn build_config_update(cfg: Option<PersistedConfig>) -> DaemonEvent {
    match cfg {
        Some(c) => DaemonEvent::ConfigUpdate {
            source: c.source.map(|p| p.display().to_string()),
            daemon: c.daemon,
            ipod: c.ipod_identity,
        },
        None => DaemonEvent::ConfigUpdate { source: None, daemon: None, ipod: None },
    }
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
