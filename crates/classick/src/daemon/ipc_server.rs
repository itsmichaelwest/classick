//! IPC server for UI ↔ daemon traffic.
//!
//! Windows: multi-instance named-pipe server on `\\.\pipe\<PROJECT_DIR>`.
//! Unix:    Unix-domain-socket server on `$XDG_RUNTIME_DIR/<PROJECT_DIR>.sock`
//!          (with fallback to `$TMPDIR` and finally `/tmp`).
//!
//! Wire format is newline-delimited JSON per `docs/ipc-protocol.md`
//! (v1.1.0); identical on both transports. The per-client handler is
//! generic over `AsyncRead`/`AsyncWrite`, so the only platform-specific
//! code is the accept loop.

use crate::ipc_daemon::{DaemonCommand, DaemonEvent, DAEMON_PROTOCOL_VERSION};
use crate::PROJECT_DIR;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

#[cfg(windows)]
use tokio::net::windows::named_pipe::ServerOptions;
#[cfg(unix)]
use tokio::net::UnixListener;

/// Resolve the platform-default IPC transport address. Derived from
/// [`PROJECT_DIR`]; the .NET side mirrors this in
/// `Classick.UI.Core.AppIdentity`. The two MUST stay in sync — the
/// pipe label is the IPC contract.
///
/// - **Windows:** `\\.\pipe\<PROJECT_DIR>`.
/// - **Unix:** a socket path under `$XDG_RUNTIME_DIR` if set and the
///   directory exists, else `$TMPDIR`, else `/tmp`. File name is
///   `<PROJECT_DIR>.sock`.
pub fn default_pipe_name() -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\{PROJECT_DIR}")
    }
    #[cfg(unix)]
    {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::var_os("TMPDIR").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        dir.join(format!("{PROJECT_DIR}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// Incoming command from a connected client, tagged with the client id
/// so the handler can reply back via the per-client sender.
pub struct ClientCommand {
    pub client_id: u64,
    pub command: DaemonCommand,
    pub reply: mpsc::UnboundedSender<DaemonEvent>,
}

/// Test-friendly entry: creates a fresh broadcast channel. Uses
/// the platform default transport address — only one such test can run
/// at a time, and never while a real daemon is up. Tests that need
/// isolation should call [`spawn_server_with`] with a per-test path.
pub async fn spawn_server() -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
)> {
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let pipe = default_pipe_name();
    let (sender, cmd_rx, _new_client_rx) =
        spawn_server_full_with(event_tx.clone(), &pipe).await?;
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
    let pipe = default_pipe_name();
    spawn_server_full_with(event_tx, &pipe).await
}

/// Underlying impl that accepts an arbitrary transport address.
/// Production calls it with [`default_pipe_name`] (the IPC contract
/// with the UI). Tests pass a unique per-test address — on Windows
/// `\\.\pipe\classick-test-<pid>-<n>`, on Unix a path under a
/// tempdir — so the suite runs alongside a real daemon.
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

    spawn_accept_loop(
        event_tx.clone(),
        pipe_name.to_string(),
        cmd_tx,
        new_client_tx,
    )?;

    Ok((event_tx, cmd_rx, new_client_rx))
}

#[cfg(windows)]
fn spawn_accept_loop(
    event_tx: broadcast::Sender<DaemonEvent>,
    pipe_name: String,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<()>,
) -> Result<()> {
    tokio::spawn(async move {
        let mut next_client_id: u64 = 1;
        // Create the first instance up-front so a fast-following
        // client sees a listening pipe immediately.
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
            let event_rx = event_tx.subscribe();
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx.clone();
            let (reader, writer) = tokio::io::split(connected);
            tokio::spawn(handle_client(
                client_id,
                reader,
                writer,
                event_rx,
                cmd_tx,
                new_client_tx,
            ));
        }
    });
    Ok(())
}

#[cfg(unix)]
fn spawn_accept_loop(
    event_tx: broadcast::Sender<DaemonEvent>,
    pipe_name: String,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<()>,
) -> Result<()> {
    // UnixListener::bind errors if the path already exists. A stale
    // socket from a previously-crashed daemon is the common case;
    // remove it. If a real daemon is currently bound we'll trample
    // its socket, but a real daemon would also be holding a process
    // lock somewhere — the named-pipe `first_pipe_instance(true)`
    // analogue. For now: trust the operator to run only one daemon.
    let _ = std::fs::remove_file(&pipe_name);
    let listener = UnixListener::bind(&pipe_name)
        .with_context(|| format!("bind unix socket {pipe_name}"))?;
    tokio::spawn(async move {
        tracing::info!("ipc-server: listening on {pipe_name}");
        let mut next_client_id: u64 = 1;
        loop {
            let stream = match listener.accept().await {
                Ok((s, _addr)) => s,
                Err(e) => {
                    tracing::warn!("ipc-server: accept failed: {e}");
                    continue;
                }
            };
            let client_id = next_client_id;
            next_client_id += 1;
            let event_rx = event_tx.subscribe();
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx.clone();
            let (reader, writer) = tokio::io::split(stream);
            tokio::spawn(handle_client(
                client_id,
                reader,
                writer,
                event_rx,
                cmd_tx,
                new_client_tx,
            ));
        }
    });
    Ok(())
}

async fn handle_client<R, W>(
    client_id: u64,
    reader_half: R,
    mut writer_half: W,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<()>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tracing::info!("ipc-server: client {client_id} connected");

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
