//! Real daemon-socket coverage for durable library-drop commands.

use classick::config_file::{DaemonSettings, IpodIdentity, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::SyncSummary;
use classick::daemon::lifecycle::ShutdownReason;
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps, SpawnFn};
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use classick::library_index::{IndexedTrack, LibraryIndex, INDEX_VERSION};
use classick::playlist::{ManualPlaylist, Playlist, PlaylistStore};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

static SERIAL_TESTS: Mutex<()> = Mutex::new(());
const DEVICE_A: &str = "000A27002138B0A8";
const DEVICE_B: &str = "000A27002138B0B9";

struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);

impl DeviceWatcher for ScriptedWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        self.0
    }
}

struct Client {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
}

impl Client {
    async fn connect(address: &str) -> Self {
        let stream = connect_transport(address).await;
        let (reader, writer) = tokio::io::split(stream);
        let mut client = Self {
            reader: BufReader::new(Box::new(reader)),
            writer: Box::new(writer),
        };
        assert_eq!(client.next().await["type"], "hello");
        client
    }

    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut line = String::new();
            assert_ne!(self.reader.read_line(&mut line).await.unwrap(), 0);
            serde_json::from_str(line.trim()).unwrap()
        })
        .await
        .expect("daemon event within five seconds")
    }

    async fn next_type(&mut self, kind: &str) -> Value {
        loop {
            let event = self.next().await;
            if event["type"] == kind {
                return event;
            }
        }
    }
}

#[cfg(unix)]
async fn connect_transport(address: &str) -> tokio::net::UnixStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::net::UnixStream::connect(address).await {
            Ok(stream) => return stream,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("connect to {address}: {error}"),
        }
    }
}

#[cfg(windows)]
async fn connect_transport(address: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(address) {
            Ok(client) => return client,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("connect to {address}: {error}"),
        }
    }
}

struct Sandbox {
    root: PathBuf,
    address: String,
    _device_tx: mpsc::Sender<DeviceEvent>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
    _shutdown_tx: mpsc::UnboundedSender<ShutdownReason>,
}

impl Sandbox {
    async fn start() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("library-drop-daemon-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let config = root.join("config.toml");
        classick::config_file::save(
            &config,
            &PersistedConfig {
                source: Some(source.clone()),
                daemon: Some(DaemonSettings {
                    enabled: false,
                    schedule_minutes: 0,
                    ..Default::default()
                }),
                ipod_identity: Some(identity(DEVICE_A)),
                ..Default::default()
            },
        )
        .unwrap();
        write_registry(&config);
        write_index(&config, source);
        let store = PlaylistStore::open(root.join("playlists")).unwrap();
        store
            .save(&Playlist::Manual(ManualPlaylist {
                slug: "favorites".into(),
                name: "Favorites".into(),
                tracks: Vec::new(),
                skipped_unsafe: 0,
            }))
            .unwrap();
        for serial in [DEVICE_A, DEVICE_B] {
            let path =
                classick::selection::effective_device_selection_path_in(&root, serial).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, br#"{"version":1,"mode":"include","rules":[]}"#).unwrap();
        }

        let (device_tx, device_rx) = mpsc::channel(4);
        let spawn_sync: SpawnFn = Arc::new(|_, _, _, _, _, _| Box::pin(async { Ok(completed()) }));
        let spawn_scan: SpawnFn = Arc::new(|_, _, _, _, _, _| Box::pin(async { Ok(completed()) }));
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let address = unique_address(&root, n);
        let deps = DaemonDeps {
            configured_serial: Some(DEVICE_A.into()),
            watcher: Box::new(ScriptedWatcher(device_rx)),
            spawn_sync: spawn_sync.clone(),
            spawn_backfill: spawn_sync.clone(),
            spawn_replace_library: spawn_sync,
            spawn_scan,
            schedule_minutes: 0,
            preset_event_tx: None,
            config_path: Some(config.clone()),
            history_path: Some(root.join("history.json")),
            pipe_name: Some(address.clone()),
            source_availability: None,
            shutdown_rx,
        };
        let runtime = tokio::spawn(run_daemon_with_deps(deps));
        Self {
            root,
            address,
            _device_tx: device_tx,
            runtime,
            _shutdown_tx: shutdown_tx,
        }
    }

    async fn connect(&self) -> Client {
        Client::connect(&self.address).await
    }

    async fn shutdown(self) {
        self.runtime.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.address);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn identity(serial: &str) -> IpodIdentity {
    IpodIdentity {
        serial: serial.into(),
        model_label: "iPod Classic".into(),
        name: Some(format!("Device {serial}")),
        custom_selection: true,
    }
}

fn write_registry(config: &Path) {
    let path = classick::config_file::device_registry_path(config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records: Vec<_> = [DEVICE_A, DEVICE_B]
        .into_iter()
        .map(|serial| {
            json!({
                "serial": serial, "model_label": "iPod Classic",
                "name": format!("Device {serial}"), "configured": true,
                "selection_revision": 0, "settings_revision": 0,
                "subscriptions_revision": 0
            })
        })
        .collect();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({"version": 1, "records": records})).unwrap(),
    )
    .unwrap();
}

fn write_index(config: &Path, source: PathBuf) {
    let mut files = BTreeMap::new();
    files.insert(
        source.join("Birdy/Fire Within/01.flac"),
        IndexedTrack {
            mtime: 1,
            size: 1,
            artist: "Birdy".into(),
            album_artist: "Birdy".into(),
            album: "Fire Within".into(),
            genre: "Pop".into(),
            title: "Fire".into(),
            duration_ms: 1,
            year: Some(2013),
        },
    );
    classick::library_index::save_atomic(
        &config.with_file_name("library-index.json"),
        &LibraryIndex {
            version: INDEX_VERSION,
            source_root: source,
            scanned_at_unix_secs: Some(1),
            files,
        },
    )
    .unwrap();
}

fn unique_address(_root: &Path, _n: u32) -> String {
    #[cfg(windows)]
    return format!(r"\\.\pipe\classick-drop-{}-{_n}", std::process::id());
    #[cfg(not(windows))]
    return std::env::temp_dir()
        .join(format!("classick-drop-{}-{_n}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
}

fn completed() -> OrchestratorOutcome {
    OrchestratorOutcome::Completed {
        summary: SyncSummary::default(),
        db_restored: false,
    }
}

fn add_device(request_id: &str, serial: &str) -> Value {
    json!({
        "type": "add_selection_to_device", "request_id": request_id,
        "mutation_id": request_id, "device_id": serial,
        "rules": [{"kind": "artist", "name": "Birdy"}]
    })
}

fn append_playlist(request_id: &str) -> Value {
    json!({
        "type": "append_selection_to_playlist", "request_id": request_id,
        "slug": "favorites", "rules": [{"kind": "genre", "name": "Pop"}]
    })
}

fn ledger_has(root: &Path, request_id: &str) -> bool {
    std::fs::read(root.join("devices/library-mutation-acks.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|ledger| ledger["entries"].as_array().cloned())
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["request_id"] == request_id)
        })
}

#[tokio::test]
async fn cross_target_commands_ack_in_order_only_after_canonical_persistence() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let sandbox = Sandbox::start().await;
    let mut client = sandbox.connect().await;
    let req_a = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8701";
    let req_p = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8702";
    client.send(add_device(req_a, DEVICE_A)).await;
    client.send(append_playlist(req_p)).await;

    let added = client.next_type("device_selection_added").await;
    assert_eq!(added["request_id"], req_a);
    assert_eq!(added["selection_revision"], 1);
    assert!(ledger_has(&sandbox.root, req_a));
    let selection = classick::selection::load_or_all(
        &classick::selection::effective_device_selection_path_in(&sandbox.root, DEVICE_A).unwrap(),
    );
    assert_eq!(selection.rules.len(), 1);

    let appended = client.next_type("playlist_selection_appended").await;
    assert_eq!(appended["request_id"], req_p);
    assert_eq!(appended["revision"], 1);
    assert!(ledger_has(&sandbox.root, req_p));
    let playlist = PlaylistStore::open(sandbox.root.join("playlists"))
        .unwrap()
        .load("favorites")
        .unwrap()
        .unwrap();
    let Playlist::Manual(playlist) = playlist else {
        panic!("favorites must remain manual")
    };
    assert_eq!(playlist.tracks.len(), 1);
    sandbox.shutdown().await;
}

#[tokio::test]
async fn disconnect_after_write_then_replay_returns_the_same_ack_without_reapplying() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let sandbox = Sandbox::start().await;
    let request = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8703";
    let mut lost = sandbox.connect().await;
    lost.send(append_playlist(request)).await;
    drop(lost);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ledger_has(&sandbox.root, request) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("mutation persists despite lost acknowledgement");

    let mut replay = sandbox.connect().await;
    replay.send(append_playlist(request)).await;
    let event = replay.next_type("playlist_selection_appended").await;
    assert_eq!(event["request_id"], request);
    assert_eq!(event["revision"], 1);
    assert_eq!(event["appended_tracks"], 1);
    let playlist = PlaylistStore::open(sandbox.root.join("playlists"))
        .unwrap()
        .load("favorites")
        .unwrap()
        .unwrap();
    let Playlist::Manual(playlist) = playlist else {
        panic!("favorites must remain manual")
    };
    assert_eq!(playlist.tracks.len(), 1);
    sandbox.shutdown().await;
}
