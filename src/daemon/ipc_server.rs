//! Multi-instance named-pipe IPC server on Windows. Accepts UI client
//! connections, broadcasts daemon events to all clients, routes client
//! commands to a central handler.
//!
//! Pipe path: `\\.\pipe\ipod-sync`. Wire format: newline-delimited JSON
//! per `docs/ipc-protocol.md` (v1.1.0).

use crate::ipc_daemon::{DaemonCommand, DaemonEvent, DAEMON_PROTOCOL_VERSION};
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{broadcast, mpsc};

pub const PIPE_NAME: &str = r"\\.\pipe\ipod-sync";

/// Incoming command from a connected client, tagged with the client id
/// so the handler can reply back via the per-client sender.
pub struct ClientCommand {
    pub client_id: u64,
    pub command: DaemonCommand,
    pub reply: mpsc::UnboundedSender<DaemonEvent>,
}

/// Test-friendly entry: creates a fresh broadcast channel. Uses
/// the production [`PIPE_NAME`] — only one such test can run at a
/// time, and never while a real daemon is up. Tests that need
/// isolation should call [`spawn_server_with`] with a per-test pipe.
pub async fn spawn_server() -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
)> {
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let (sender, cmd_rx, _new_client_rx) =
        spawn_server_full_with(event_tx.clone(), PIPE_NAME).await?;
    Ok((sender, cmd_rx))
}

/// Production entry: caller supplies the broadcast sender so it can
/// be shared with the sync orchestrator (which also publishes to it).
/// Returns an extra mpsc receiver that fires once per new client
/// connection — the runtime uses this to publish a snapshot
/// StatusUpdate so newly-connected UIs don't miss earlier broadcasts.
pub async fn spawn_server_full(
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
    mpsc::UnboundedReceiver<()>,
)> {
    spawn_server_full_with(event_tx, PIPE_NAME).await
}

/// Underlying impl that accepts an arbitrary pipe name. Production calls
/// it with [`PIPE_NAME`] (the IPC contract with the UI). Tests can pass
/// `\\.\pipe\ipod-sync-test-<pid>-<n>` to avoid colliding with each
/// other AND with a real running daemon on the developer's machine.
pub async fn spawn_server_full_with(
    event_tx: broadcast::Sender<DaemonEvent>,
    pipe_name: &str,
) -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
    mpsc::UnboundedReceiver<()>,
)> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
    let (new_client_tx, new_client_rx) = mpsc::unbounded_channel::<()>();

    let event_tx_clone = event_tx.clone();
    let new_client_tx_clone = new_client_tx.clone();
    let pipe_name = pipe_name.to_string();
    tokio::spawn(async move {
        let mut next_client_id: u64 = 1;
        // Create the first instance up-front.
        let mut server = match ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("ipc-server: failed to create initial named pipe {pipe_name}: {e}");
                return;
            }
        };
        tracing::info!("ipc-server: listening on {pipe_name}");

        loop {
            if let Err(e) = server.connect().await {
                tracing::warn!("ipc-server: connect failed: {e}");
                continue;
            }
            let connected = server;

            // Create the next instance immediately so the next client
            // connecting doesn't see "no instances available."
            server = match ServerOptions::new().create(&pipe_name) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("ipc-server: failed to create next pipe instance: {e}");
                    return;
                }
            };

            let client_id = next_client_id;
            next_client_id += 1;
            let event_rx = event_tx_clone.subscribe();
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx_clone.clone();
            tokio::spawn(handle_client(client_id, connected, event_rx, cmd_tx, new_client_tx));
        }
    });

    Ok((event_tx, cmd_rx, new_client_rx))
}

async fn handle_client(
    client_id: u64,
    pipe: NamedPipeServer,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<()>,
) {
    tracing::info!("ipc-server: client {client_id} connected");
    let (reader_half, mut writer_half) = tokio::io::split(pipe);

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<DaemonEvent>();

    // Send the Hello event first.
    let hello = DaemonEvent::Hello {
        protocol_version: DAEMON_PROTOCOL_VERSION.to_string(),
        core_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if write_event(&mut writer_half, &hello).await.is_err() {
        return;
    }

    // Signal the runtime to broadcast a snapshot StatusUpdate so this
    // newly-connected client sees current state without needing to
    // race against any in-flight broadcasts.
    let _ = new_client_tx.send(());

    let mut reader = BufReader::new(reader_half);
    let mut line_buf = String::new();
    loop {
        tokio::select! {
            read_result = reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => {
                        tracing::info!("ipc-server: client {client_id} disconnected");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if !trimmed.is_empty() {
                            match serde_json::from_str::<DaemonCommand>(trimmed) {
                                Ok(cmd) => {
                                    let _ = cmd_tx.send(ClientCommand {
                                        client_id,
                                        command: cmd,
                                        reply: reply_tx.clone(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "ipc-server: client {client_id} sent unparseable command {trimmed:?}: {e}"
                                    );
                                }
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        tracing::warn!("ipc-server: client {client_id} read error: {e}");
                        break;
                    }
                }
            }
            broadcast_event = event_rx.recv() => {
                match broadcast_event {
                    Ok(event) => {
                        if write_event(&mut writer_half, &event).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::warn!("ipc-server: client {client_id} lagged broadcast");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            reply_event = reply_rx.recv() => {
                match reply_event {
                    Some(event) => {
                        if write_event(&mut writer_half, &event).await.is_err() { break; }
                    }
                    None => break,
                }
            }
        }
    }
}

async fn write_event<W>(writer: &mut W, event: &DaemonEvent) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let json = serde_json::to_string(event).context("serialize event")?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
