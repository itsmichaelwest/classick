//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, and dispatches client commands.
//!
//! M2 scope: respond to GetStatus / GetConfig / SaveConfig / GetHistory
//! / Subscribe-/UnsubscribeDeviceEvents / Shutdown. TriggerSync replies
//! with `sync_rejected { reason: not_configured }` until M3 wires the
//! sync orchestrator.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::history::HistoryService;
use crate::daemon::ipc_server::{spawn_server, ClientCommand};
use crate::daemon::state::StateMachine;
use crate::ipc_daemon::{DaemonCommand, DaemonEvent, DaemonStateLabel, SyncRejectReason};
use anyhow::Result;
use std::sync::Mutex;

pub async fn run_daemon() -> Result<()> {
    tracing::info!("daemon: starting");

    let history_path = history_file_path()?;
    let history = HistoryService::new(history_path.clone());
    let config_path = config_file::default_path()?;
    let state = Mutex::new(StateMachine::new());

    let (event_tx, mut cmd_rx) = spawn_server().await?;

    tracing::info!("daemon: ready");

    while let Some(client_cmd) = cmd_rx.recv().await {
        handle_command(client_cmd, &history, &config_path, &state, &event_tx).await;
    }

    tracing::info!("daemon: exiting (command channel closed)");
    Ok(())
}

async fn handle_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &Mutex<StateMachine>,
    event_tx: &tokio::sync::broadcast::Sender<DaemonEvent>,
) {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    match command {
        DaemonCommand::GetStatus => {
            let configured = config_file::load(config_path)
                .ok()
                .flatten()
                .and_then(|c| c.ipod_identity)
                .is_some();
            let state_label = match state.lock().unwrap().state() {
                crate::daemon::state::DaemonState::Idle => DaemonStateLabel::Idle,
                crate::daemon::state::DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
            };
            let entries = history.read();
            let last_sync = entries.last().cloned();
            let _ = reply.send(DaemonEvent::StatusUpdate {
                state: state_label,
                configured,
                ipod_connected: false, // M3 wires this
                last_sync,
                next_scheduled_unix_secs: None, // M3 wires this
            });
        }
        DaemonCommand::GetConfig => {
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = reply.send(build_config_update(cfg));
        }
        DaemonCommand::SaveConfig { source, daemon, ipod } => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            if let Some(s) = source {
                current.source = Some(std::path::PathBuf::from(s));
            }
            if let Some(d) = daemon {
                current.daemon = Some(d);
            }
            if let Some(i) = ipod {
                current.ipod_identity = Some(i);
            }
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
        DaemonCommand::TriggerSync { .. } => {
            // M2: real sync is M3. Reject with NotConfigured until then.
            let _ = reply.send(DaemonEvent::SyncRejected {
                reason: SyncRejectReason::NotConfigured,
            });
        }
        DaemonCommand::SubscribeDeviceEvents | DaemonCommand::UnsubscribeDeviceEvents => {
            // M2: no DeviceWatcher yet (M3). No-op; wizard uses local-side
            // drive scan via C# (not daemon-emitted events) for M2.
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

fn history_file_path() -> Result<std::path::PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA unavailable"))?
        .join("ipod-sync");
    Ok(base.join("history.json"))
}
