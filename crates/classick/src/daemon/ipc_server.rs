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

use crate::ipc_daemon::{DaemonCommand, DaemonEvent};
use crate::wire::{
    decode_admitted_message, AdmittedStream, CapabilityName, DecodedWireMessage, EndpointRole,
    WireHello, WireMessage,
};
use crate::PROJECT_DIR;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};

#[cfg(unix)]
use crate::daemon::unix_socket::UnixSocketLease;
#[cfg(windows)]
use tokio::net::windows::named_pipe::ServerOptions;

#[cfg(target_os = "macos")]
extern "C" {
    fn confstr(name: std::os::raw::c_int, buf: *mut std::os::raw::c_char, len: usize) -> usize;
}

/// macOS: the Apple-sanctioned per-user runtime dir (`$TMPDIR` points here,
/// but confstr is robust against an unset/overridden env var). Stable
/// per-UID across reboots; ~60-char path stays under the 104-byte sun_path
/// limit. This is the IPC contract the SwiftUI client must match via
/// NSTemporaryDirectory().
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<std::path::PathBuf> {
    use std::os::raw::c_char;
    const CS_DARWIN_USER_TEMP_DIR: std::os::raw::c_int = 65537; // _CS_DARWIN_USER_TEMP_DIR
    let need = unsafe { confstr(CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if need == 0 {
        return None;
    }
    let mut buf = vec![0 as c_char; need];
    let got = unsafe { confstr(CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), need) };
    if got == 0 || got > need {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(std::path::PathBuf::from(
        cstr.to_string_lossy().into_owned(),
    ))
}

/// Resolve the platform-default IPC transport address. Derived from
/// [`PROJECT_DIR`]; the .NET side mirrors this in
/// `Classick.UI.Core.AppIdentity`. The two MUST stay in sync — the
/// pipe label is the IPC contract.
///
/// - **Windows:** `\\.\pipe\<PROJECT_DIR>`.
/// - **macOS:** the Darwin per-user temp dir via
///   `confstr(_CS_DARWIN_USER_TEMP_DIR)` (same directory `$TMPDIR` points at
///   and that Swift resolves via `NSTemporaryDirectory()`), falling back to
///   the `$XDG_RUNTIME_DIR`/`$TMPDIR`/`/tmp` chain below if `confstr` fails.
/// - **Unix (other):** a socket path under `$XDG_RUNTIME_DIR` if set and the
///   directory exists, else `$TMPDIR`, else `/tmp`. File name is
///   `<PROJECT_DIR>.sock`.
pub fn default_pipe_name() -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\{PROJECT_DIR}")
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        if let Some(dir) = darwin_user_temp_dir() {
            return dir
                .join(format!("{PROJECT_DIR}.sock"))
                .to_string_lossy()
                .into_owned();
        }

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

/// A newly connected client waiting for its ordered initial state snapshot.
pub struct NewClient {
    pub initial: oneshot::Sender<InitialClientState>,
}

pub struct InitialClientState {
    pub events: Vec<DaemonEvent>,
    pub live_events: broadcast::Receiver<DaemonEvent>,
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
    let (sender, cmd_rx, _new_client_rx) = spawn_server_full_with(event_tx.clone(), &pipe).await?;
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
    mpsc::UnboundedReceiver<NewClient>,
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
    mpsc::UnboundedReceiver<NewClient>,
)> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
    let (new_client_tx, new_client_rx) = mpsc::unbounded_channel::<NewClient>();

    spawn_accept_loop(pipe_name.to_string(), cmd_tx, new_client_tx)?;

    Ok((event_tx, cmd_rx, new_client_rx))
}

#[cfg(windows)]
fn spawn_accept_loop(
    pipe_name: String,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<NewClient>,
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
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx.clone();
            let (reader, writer) = tokio::io::split(connected);
            tokio::spawn(handle_client(
                client_id,
                reader,
                writer,
                cmd_tx,
                new_client_tx,
            ));
        }
    });
    Ok(())
}

#[cfg(unix)]
fn spawn_accept_loop(
    pipe_name: String,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<NewClient>,
) -> Result<()> {
    let (listener, lease) = UnixSocketLease::bind(std::path::Path::new(&pipe_name))?;
    tokio::spawn(async move {
        let _lease = lease;
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
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx.clone();
            let (reader, writer) = tokio::io::split(stream);
            tokio::spawn(handle_client(
                client_id,
                reader,
                writer,
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
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<NewClient>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tracing::info!("ipc-server: client {client_id} connected");

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<DaemonEvent>();

    let hello = WireHello::new(
        EndpointRole::Daemon,
        env!("CARGO_PKG_VERSION"),
        [
            "device_inventory",
            "portable_profile",
            "typed_sync_progress",
        ]
        .into_iter()
        .map(CapabilityName::parse)
        .collect::<Result<Vec<_>>>()
        .expect("daemon capability names are valid"),
    )
    .expect("daemon hello is valid");
    if write_message(&mut writer_half, &WireMessage::Hello(hello))
        .await
        .is_err()
    {
        return;
    }

    // Block live broadcast delivery until the runtime returns this client's
    // complete initial batch. This guarantees hello → status → inventory →
    // source ordering and prevents another UI from observing the replay.
    let (initial_tx, initial_rx) = oneshot::channel();
    let _ = new_client_tx.send(NewClient {
        initial: initial_tx,
    });
    let Ok(initial_state) = initial_rx.await else {
        return;
    };
    for event in initial_state.events {
        if write_event(&mut writer_half, &event).await.is_err() {
            return;
        }
    }
    let mut event_rx = initial_state.live_events;

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
                            match decode_admitted_message(
                                trimmed,
                                &AdmittedStream::DaemonReceivingDesktopCommands,
                            ) {
                                Ok(DecodedWireMessage::Known(message)) => {
                                    let WireMessage::Command(command) = *message else {
                                        unreachable!("desktop command admission returned a non-command");
                                    };
                                    let _ = cmd_tx.send(ClientCommand {
                                        client_id,
                                        command: DaemonCommand::Protocol3(Box::new(command)),
                                        reply: reply_tx.clone(),
                                    });
                                }
                                Ok(DecodedWireMessage::IgnoredUnknownEvent { .. }) => unreachable!(),
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
    let Some(event) = crate::daemon::protocol_v3::event_to_wire(event)? else {
        return Ok(());
    };
    write_message(writer, &WireMessage::Event(event)).await
}

async fn write_message<W>(writer: &mut W, message: &WireMessage) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let json = serde_json::to_string(message).context("serialize wire message")?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn library_update_with_untagged_labels_is_written_to_the_client() {
        let (client, mut server) = tokio::io::duplex(4096);
        let event = DaemonEvent::LibraryUpdate {
            source_root: Some("smb://library/music".to_owned()),
            scanned_at_unix_secs: Some(1),
            artists: vec![crate::ipc_daemon::LibraryArtist {
                name: String::new(),
                albums: vec![crate::ipc_daemon::LibraryAlbum {
                    name: String::new(),
                    genre: Some(String::new()),
                    tracks: 1,
                    bytes: 42,
                }],
            }],
            genres: vec![crate::ipc_daemon::LibraryGenre {
                name: String::new(),
                tracks: 1,
                bytes: 42,
            }],
            total_tracks: 1,
            total_bytes: 42,
            acknowledged_request_id: Some("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8807".to_owned()),
        };

        write_event(&mut server, &event)
            .await
            .expect("untagged display buckets remain valid library data");

        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let message = serde_json::from_str::<serde_json::Value>(&line).unwrap();
        assert_eq!(message["type"], "library");
        assert_eq!(message["artists"][0]["name"], "");
        assert_eq!(message["genres"][0]["name"], "");
    }

    #[tokio::test]
    async fn initial_snapshot_drops_broadcasts_queued_before_its_cutover() {
        let (event_tx, _) = broadcast::channel(8);
        let _keepalive_rx = event_tx.subscribe();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (new_client_tx, mut new_client_rx) = mpsc::unbounded_channel();
        let (client, server) = tokio::io::duplex(4096);
        let (server_reader, server_writer) = tokio::io::split(server);
        let handler = tokio::spawn(handle_client(
            1,
            server_reader,
            server_writer,
            cmd_tx,
            new_client_tx,
        ));
        let (client_reader, _client_writer) = tokio::io::split(client);
        let mut reader = BufReader::new(client_reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["type"],
            "hello"
        );

        event_tx
            .send(DaemonEvent::SourceAvailability {
                state: crate::ipc_daemon::SourceAvailabilityState::AuthRequired,
                source_root: None,
                acknowledged_request_id: None,
            })
            .unwrap();
        let connected = new_client_rx.recv().await.unwrap();
        assert!(connected
            .initial
            .send(InitialClientState {
                events: vec![DaemonEvent::SourceAvailability {
                    state: crate::ipc_daemon::SourceAvailabilityState::Remounting,
                    source_root: None,
                    acknowledged_request_id: None,
                }],
                live_events: event_tx.subscribe(),
            })
            .is_ok());

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        let initial = serde_json::from_str::<serde_json::Value>(&line).unwrap();
        assert_eq!(initial["state"], "remounting");
        line.clear();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                reader.read_line(&mut line),
            )
            .await
            .is_err(),
            "auth_required queued before the remounting snapshot must be discarded"
        );
        handler.abort();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_pipe_name_is_absolute_sock_under_temp() {
        let p = default_pipe_name();
        assert!(p.starts_with('/'), "must be absolute: {p}");
        assert!(p.ends_with(".sock"), "must be a .sock: {p}");
    }
}
