//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, device watcher, scheduler, and sync orchestrator.
//!
//! M3 scope: real auto-sync on configured-iPod plug-in, Sync Now via
//! manual TriggerSync, periodic Scheduled triggers from the scheduler,
//! and live DeviceConnected/Disconnected broadcasts. Test-only entry
//! `run_daemon_with_deps` exists so the integration suite can inject
//! a scripted device watcher and a fake spawn-fn.

use crate::config_file::{self, PersistedConfig};
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
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

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
        .unwrap_or(30);

    // Build the broadcast event_tx FIRST so the spawn_sync closure can
    // capture a clone — that way orchestrator events flow through the
    // same channel UI clients are subscribed to.
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let exe = std::env::current_exe()?;
    let event_tx_for_spawn = event_tx.clone();
    let spawn_sync: SpawnFn = Arc::new(move |drive: String| {
        let exe = exe.clone();
        let event_tx = event_tx_for_spawn.clone();
        Box::pin(async move {
            sync_orchestrator::run(exe, drive, event_tx).await
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

/// T2 stub: forwards to the test `spawn_server` for now. T3 replaces
/// this with the real impl that wires `ipc_server` with the supplied
/// event_tx + the new-client signal channel.
async fn spawn_server_with_event_tx(
    _preset: broadcast::Sender<DaemonEvent>,
) -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
)> {
    spawn_server().await
}

/// Async closure that runs one sync to completion. Arc-wrapped so the
/// runtime can clone it into a tokio::spawn'd task without consuming the
/// daemon's only copy.
pub type SpawnFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
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
}

/// Test-friendly entry. Production wraps real impls and calls this.
pub async fn run_daemon_with_deps(deps: DaemonDeps) -> Result<()> {
    let history_path = history_file_path()?;
    let history = HistoryService::new(history_path);
    let config_path = config_file::default_path()?;
    let mut state = StateMachine::new();
    let mut scheduler = SyncScheduler::new(deps.schedule_minutes);
    let mut debouncer = Debouncer::new(Duration::from_millis(500));
    let mut connected: Option<DetectedIpod> = None;
    let configured_serial = deps.configured_serial;

    let (event_tx, mut cmd_rx) = match deps.preset_event_tx {
        Some(tx) => {
            // Production: reuse the channel that spawn_sync already
            // captured a clone of. ipc_server::spawn_server needs to
            // share the same sender — pass it in.
            spawn_server_with_event_tx(tx).await?
        }
        None => spawn_server().await?, // test path
    };
    let mut device_rx = deps.watcher.start();
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let spawn_sync = deps.spawn_sync;

    tracing::info!("daemon: ready (configured_serial={configured_serial:?})");

    loop {
        tokio::select! {
            biased;

            client_cmd = cmd_rx.recv() => {
                let Some(client_cmd) = client_cmd else {
                    tracing::info!("daemon: command channel closed; exiting");
                    return Ok(());
                };
                handle_client_command(
                    client_cmd,
                    &history,
                    &config_path,
                    &mut state,
                    &event_tx,
                    &connected,
                    &spawn_sync,
                    &internal_tx,
                    &configured_serial,
                );
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
                    configured_serial.as_deref(),
                );
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }

            Some(internal) = internal_rx.recv() => {
                handle_internal_event(internal, &mut state, &event_tx, &history, &connected);
            }

            _ = scheduler.tick() => {
                if connected.is_some() && state.is_idle() {
                    if let Some(drive) = connected.as_ref().map(|d| d.drive.clone()) {
                        start_sync_session(
                            SyncTrigger::Scheduled,
                            connected.as_ref().unwrap().serial.clone(),
                            drive,
                            &mut state,
                            &event_tx,
                            &spawn_sync,
                            &internal_tx,
                        );
                    }
                }
            }
        }
    }
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
    connected: &Option<DetectedIpod>,
) {
    match event {
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
            });
        }
    }
}

fn handle_device_event(
    event: DeviceEvent,
    connected: &mut Option<DetectedIpod>,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut StateMachine,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    configured_serial: Option<&str>,
) {
    match event {
        DeviceEvent::Connected(ipod) => {
            *connected = Some(ipod.clone());
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: ipod.serial.clone(),
                model_label: ipod.model_label.clone(),
                drive: ipod.drive.clone(),
            });
            // Auto-sync only fires for the configured serial.
            if configured_serial == Some(ipod.serial.as_str()) && state.is_idle() {
                start_sync_session(
                    SyncTrigger::PlugIn,
                    ipod.serial.clone(),
                    ipod.drive.clone(),
                    state,
                    event_tx,
                    spawn_sync,
                    internal_tx,
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
    });

    let spawn_sync = spawn_sync.clone();
    let internal_tx = internal_tx.clone();
    let drive_for_task = drive.clone();
    let trigger_for_task = trigger.clone();
    let serial_for_task = serial.clone();
    tokio::spawn(async move {
        let outcome = (spawn_sync)(drive_for_task).await;
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
    });
}

fn handle_client_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    connected: &Option<DetectedIpod>,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    configured_serial: &Option<String>,
) {
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
            if let Some(i) = ipod { current.ipod_identity = Some(i); }
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config: {e}");
                return;
            }
            let _ = event_tx.send(build_config_update(Some(current)));
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
                return;
            }
            let Some(device) = connected.as_ref() else {
                let _ = reply.send(DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod });
                return;
            };
            if configured_serial.is_none() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::NotConfigured,
                });
                return;
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
            );
        }
        DaemonCommand::SubscribeDeviceEvents | DaemonCommand::UnsubscribeDeviceEvents => {
            // M3: all clients see device events (simpler than per-client
            // filtering). Subscribe is a no-op handshake.
        }
        DaemonCommand::Shutdown => {
            tracing::info!("daemon: shutdown requested by client {client_id}; exiting loop");
            std::process::exit(0);
        }
    }
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
        .join("ipod-sync");
    Ok(base.join("history.json"))
}

// Suppress the unused-import warning when the test build doesn't take this path.
#[allow(dead_code)]
fn _silence_mpsc_warning(_: mpsc::Sender<DaemonEvent>) {}
