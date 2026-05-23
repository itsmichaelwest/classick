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

    let exe = std::env::current_exe()?;
    let spawn_sync: SpawnFn = Box::new(move |drive: String| {
        let exe = exe.clone();
        // Wrap the daemon-orchestrator call. The broadcast tx for
        // forwarding live IPC events is injected by run_daemon_with_deps
        // via a closure in production; here we pass a dummy because the
        // orchestrator currently doesn't actually forward in M3 (M4
        // wires the full UI event stream).
        Box::pin(async move {
            let (tx, _rx) = broadcast::channel::<DaemonEvent>(1);
            sync_orchestrator::run(exe, drive, tx).await
        })
    });

    let deps = DaemonDeps {
        configured_serial,
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        schedule_minutes,
    };
    run_daemon_with_deps(deps).await
}

pub type SpawnFn = Box<
    dyn Fn(String) -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
        + Send
        + Sync,
>;

pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    pub schedule_minutes: u32,
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

    let (event_tx, mut cmd_rx) = spawn_server().await?;
    let mut device_rx = deps.watcher.start();

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
                    &deps.spawn_sync,
                    &configured_serial,
                ).await;
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
                    &deps.spawn_sync,
                    configured_serial.as_deref(),
                ).await;
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }

            _ = scheduler.tick() => {
                if connected.is_some() && state.is_idle() {
                    if let Some(drive) = connected.as_ref().map(|d| d.drive.clone()) {
                        spawn_sync_session(
                            SyncTrigger::Scheduled,
                            connected.as_ref().unwrap().serial.clone(),
                            drive,
                            &mut state,
                            &event_tx,
                            &history,
                            &deps.spawn_sync,
                        ).await;
                    }
                }
            }
        }
    }
}

async fn handle_device_event(
    event: DeviceEvent,
    connected: &mut Option<DetectedIpod>,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut StateMachine,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
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
                spawn_sync_session(
                    SyncTrigger::PlugIn,
                    ipod.serial.clone(),
                    ipod.drive.clone(),
                    state,
                    event_tx,
                    history,
                    spawn_sync,
                ).await;
            }
        }
        DeviceEvent::Disconnected { serial } => {
            *connected = None;
            let _ = event_tx.send(DaemonEvent::DeviceDisconnected { serial: serial.clone() });
            // If the device we were syncing disappeared, force-finish
            // the session with Aborted.
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

async fn spawn_sync_session(
    trigger: SyncTrigger,
    serial: String,
    drive: String,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
) {
    if state.try_start_sync_for_device(trigger.clone(), serial.clone(), drive.clone())
        != TriggerOutcome::Accepted
    {
        return;
    }
    let started_at = match state.state() {
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

    // Run the orchestrator inline. (M3 keeps it inline; M4 may move to
    // a separate task so the runtime keeps processing commands during
    // sync. For M3, the state machine already drops concurrent triggers
    // via DroppedAlreadySyncing.)
    let outcome = (spawn_sync)(drive.clone()).await;

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
        trigger, history_outcome, error_message, summary, started_at,
    );
    let last_sync = Some(entry.clone());
    let _ = history.append(entry);
    state.finish_sync();

    // Tell UIs the sync is over so the tray icon + tooltip flip back to
    // Idle. Without this, manual TriggerSync leaves the tray stuck in
    // "Syncing..." until the next device-event arm fires broadcast_status.
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: DaemonStateLabel::Idle,
        configured: true,
        ipod_connected: true,
        last_sync,
        next_scheduled_unix_secs: None,
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
        timestamp: format_iso8601(now),
        duration_secs: duration,
        trigger,
        outcome,
        error_message,
        summary,
    }
}

fn format_iso8601(unix_secs: u64) -> String {
    // Minimal ISO8601 without a chrono dep; UTC.
    use std::time::{Duration, UNIX_EPOCH};
    let _ = UNIX_EPOCH + Duration::from_secs(unix_secs);
    // Just emit the unix ts as a placeholder string. UI displays
    // history.timestamp verbatim; M4 popover will format properly.
    format!("@{unix_secs}")
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

async fn handle_client_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    connected: &Option<DetectedIpod>,
    spawn_sync: &SpawnFn,
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
            spawn_sync_session(
                trigger,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                history,
                spawn_sync,
            ).await;
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
