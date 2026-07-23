use classick::config_file::{self, DaemonSettings, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::SyncSummary;
use classick::daemon::lifecycle::ShutdownReason;
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps, SpawnFn};
use classick::daemon::session_admission::EventContext;
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use classick::device_config::Subscriptions;
use classick::device_state::device_subscriptions_path_in;
use classick::playlist::{ManualPlaylist, Playlist, PlaylistStore};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

struct NoDeviceWatcher(mpsc::Receiver<DeviceEvent>);

impl DeviceWatcher for NoDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        self.0
    }
}

pub(super) struct TestClient {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
}

impl TestClient {
    async fn connect(pipe_name: &str) -> Self {
        let stream = connect_transport(pipe_name).await;
        let (reader, writer) = tokio::io::split(stream);
        let mut client = Self {
            reader: BufReader::new(Box::new(reader)),
            writer: Box::new(writer),
        };
        assert_eq!(client.next().await["type"], "hello");
        while client.next().await["type"] != "device_inventory" {}
        client
    }

    pub(super) async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    pub(super) async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), read_json(&mut self.reader))
            .await
            .expect("daemon event within five seconds")
    }

    pub(super) async fn next_type(&mut self, event_type: &str) -> Value {
        loop {
            let event = self.next().await;
            if event["type"] == event_type {
                return event;
            }
        }
    }

    pub(super) async fn assert_no_success_broadcast(&mut self) {
        let result = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                let event = read_json(&mut self.reader).await;
                if matches!(event["type"].as_str(), Some("playlists" | "device_config")) {
                    break event;
                }
            }
        })
        .await;
        assert!(result.is_err(), "unexpected success broadcast: {result:?}");
    }
}

async fn read_json(reader: &mut (impl AsyncBufRead + Unpin)) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).await.unwrap();
    assert_ne!(read, 0, "daemon closed the client transport");
    serde_json::from_str(line.trim()).unwrap()
}

#[cfg(unix)]
async fn connect_transport(pipe_name: &str) -> tokio::net::UnixStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::net::UnixStream::connect(pipe_name).await {
            Ok(stream) => return stream,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("connect to {pipe_name}: {error}"),
        }
    }
}

#[cfg(windows)]
async fn connect_transport(pipe_name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name) {
            Ok(client) => return client,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("connect to {pipe_name}: {error}"),
        }
    }
}

pub(super) struct Sandbox {
    pub(super) root: PathBuf,
    pub(super) registry_path: PathBuf,
    pipe_name: String,
    _device_tx: mpsc::Sender<DeviceEvent>,
    _shutdown_tx: mpsc::UnboundedSender<ShutdownReason>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Sandbox {
    pub(super) async fn start(records: &[(&str, u64)]) -> Self {
        let root = test_root("runtime");
        let config_path = root.join("config.toml");
        config_file::save(
            &config_path,
            &PersistedConfig {
                daemon: Some(DaemonSettings {
                    enabled: false,
                    schedule_minutes: 0,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let registry_path = write_registry(&config_path, records);
        Self::start_from_existing(root, config_path, registry_path).await
    }

    pub(super) async fn start_from_existing(
        root: PathBuf,
        config_path: PathBuf,
        registry_path: PathBuf,
    ) -> Self {
        let pipe_name = unique_pipe_name(&root);
        let (device_tx, device_rx) = mpsc::channel(1);
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let spawn = noop_spawn();
        let runtime = tokio::spawn(run_daemon_with_deps(DaemonDeps {
            configured_serial: None,
            watcher: Box::new(NoDeviceWatcher(device_rx)),
            spawn_sync: spawn.clone(),
            spawn_backfill: spawn.clone(),
            spawn_replace_library: spawn.clone(),
            spawn_scan: spawn,
            schedule_minutes: 0,
            preset_event_tx: None,
            config_path: Some(config_path.clone()),
            history_path: Some(root.join("history.json")),
            pipe_name: Some(pipe_name.clone()),
            source_availability: None,
            shutdown_rx,
        }));
        Self {
            root,
            registry_path,
            pipe_name,
            _device_tx: device_tx,
            _shutdown_tx: shutdown_tx,
            runtime,
        }
    }

    pub(super) async fn connect(&self) -> TestClient {
        TestClient::connect(&self.pipe_name).await
    }

    pub(super) async fn shutdown(self) {
        self.runtime.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.pipe_name);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn noop_spawn() -> SpawnFn {
    Arc::new(|_: String, _: String, _, _, _, _: EventContext| {
        Box::pin(async {
            Ok(OrchestratorOutcome::Completed {
                summary: SyncSummary::default(),
                db_restored: false,
            })
        })
    })
}

pub(super) fn test_root(label: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "playlist-deletion-{label}-{}-{n}",
            std::process::id()
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn unique_pipe_name(_root: &Path) -> String {
    #[cfg(windows)]
    return format!(
        r"\\.\pipe\classick-playlist-delete-{}-{}",
        std::process::id(),
        _root.file_name().unwrap().to_string_lossy()
    );
    #[cfg(not(windows))]
    {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("classick-pdel-{}-{n}.sock", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }
}

pub(super) fn write_registry(config_path: &Path, records: &[(&str, u64)]) -> PathBuf {
    let path = config_file::device_registry_path(config_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records: Vec<_> = records
        .iter()
        .map(|(serial, subscriptions_revision)| {
            json!({
                "serial": serial,
                "model_label": "iPod Classic",
                "configured": true,
                "selection_revision": 0,
                "settings_revision": 0,
                "subscriptions_revision": subscriptions_revision
            })
        })
        .collect();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({"version": 1, "records": records})).unwrap(),
    )
    .unwrap();
    path
}

pub(super) fn save_playlist(root: &Path, slug: &str) {
    PlaylistStore::open(root.join("playlists"))
        .unwrap()
        .save(&Playlist::Manual(ManualPlaylist {
            slug: slug.into(),
            name: "Gym".into(),
            tracks: vec!["Artist/Album/01.flac".into()],
            skipped_unsafe: 0,
        }))
        .unwrap();
}

pub(super) fn save_subscriptions(root: &Path, serial: &str, playlists: &[&str]) -> PathBuf {
    let path = device_subscriptions_path_in(root, serial).unwrap();
    Subscriptions::save_atomic(
        &path,
        &Subscriptions {
            version: 1,
            playlists: playlists.iter().map(|slug| (*slug).to_string()).collect(),
        },
    )
    .unwrap();
    path
}

pub(super) fn load_subscriptions(root: &Path, serial: &str) -> Subscriptions {
    Subscriptions::load_or_default(&device_subscriptions_path_in(root, serial).unwrap())
}

pub(super) fn subscriptions_revision(registry_path: &Path, serial: &str) -> u64 {
    let registry: Value = serde_json::from_slice(&std::fs::read(registry_path).unwrap()).unwrap();
    if registry["schema_version"] == 2 {
        registry["devices"][serial]["migration_status"]["subscriptions_revision"]
            .as_u64()
            .unwrap()
    } else {
        registry["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["serial"] == serial)
            .unwrap()["subscriptions_revision"]
            .as_u64()
            .unwrap()
    }
}

pub(super) struct JournalSubscription<'a> {
    pub(super) serial: &'a str,
    pub(super) live: &'a Path,
    pub(super) original_stage: &'a Path,
    pub(super) target_stage: &'a Path,
    pub(super) original: &'a [u8],
    pub(super) target: &'a [u8],
    pub(super) original_revision: u64,
}

pub(super) fn write_mutation_journal(
    root: &Path,
    request_id: &str,
    phase: &str,
    playlist_live: &Path,
    playlist_stage: &Path,
    playlist_original: &[u8],
    subscriptions: &[JournalSubscription<'_>],
) {
    let mutation_root = root.join("devices/playlist-mutations");
    std::fs::create_dir_all(&mutation_root).unwrap();
    let relative = |path: &Path| path.strip_prefix(root).unwrap().to_path_buf();
    let subscriptions: Vec<_> = subscriptions
        .iter()
        .map(|subscription| {
            json!({
                "serial": subscription.serial,
                "live_path": relative(subscription.live),
                "staged_original_path": relative(subscription.original_stage),
                "staged_target_path": relative(subscription.target_stage),
                "original_hash": hash(subscription.original),
                "target_hash": hash(subscription.target),
                "original_revision": subscription.original_revision,
                "target_revision": subscription.original_revision + 1
            })
        })
        .collect();
    std::fs::write(
        mutation_root.join(format!("{request_id}.json")),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "request_id": request_id,
            "slug": "gym",
            "phase": phase,
            "playlist": {
                "live_path": relative(playlist_live),
                "staged_original_path": relative(playlist_stage),
                "original_hash": hash(playlist_original)
            },
            "subscriptions": subscriptions
        }))
        .unwrap(),
    )
    .unwrap();
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
