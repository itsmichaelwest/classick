//! Transactional host-side playlist deletion across every remembered device.

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

struct TestClient {
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
        assert_eq!(client.next().await["type"], "status_update");
        assert_eq!(client.next().await["type"], "device_inventory_snapshot");
        client
    }

    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), read_json(&mut self.reader))
            .await
            .expect("daemon event within five seconds")
    }

    async fn next_type(&mut self, event_type: &str) -> Value {
        loop {
            let event = self.next().await;
            if event["type"] == event_type {
                return event;
            }
        }
    }

    async fn assert_no_success_broadcast(&mut self) {
        let result = tokio::time::timeout(Duration::from_millis(300), async {
            loop {
                let event = read_json(&mut self.reader).await;
                if matches!(
                    event["type"].as_str(),
                    Some("playlists_update" | "device_config_update")
                ) {
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

struct Sandbox {
    root: PathBuf,
    registry_path: PathBuf,
    pipe_name: String,
    _device_tx: mpsc::Sender<DeviceEvent>,
    _shutdown_tx: mpsc::UnboundedSender<ShutdownReason>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Sandbox {
    async fn start(records: &[(&str, u64)]) -> Self {
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
        let pipe_name = unique_pipe_name(&root);
        let (device_tx, device_rx) = mpsc::channel(1);
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let spawn = noop_spawn();
        let deps = DaemonDeps {
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
        };
        let runtime = tokio::spawn(run_daemon_with_deps(deps));
        Self {
            root,
            registry_path,
            pipe_name,
            _device_tx: device_tx,
            _shutdown_tx: shutdown_tx,
            runtime,
        }
    }

    async fn connect(&self) -> TestClient {
        TestClient::connect(&self.pipe_name).await
    }

    async fn shutdown(self) {
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

fn test_root(label: &str) -> PathBuf {
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
        return std::env::temp_dir()
            .join(format!("classick-pdel-{}-{n}.sock", std::process::id()))
            .to_string_lossy()
            .into_owned();
    }
}

fn write_registry(config_path: &Path, records: &[(&str, u64)]) -> PathBuf {
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

fn save_playlist(root: &Path, slug: &str) {
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

fn save_subscriptions(root: &Path, serial: &str, playlists: &[&str]) -> PathBuf {
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

fn load_subscriptions(root: &Path, serial: &str) -> Subscriptions {
    Subscriptions::load_or_default(&device_subscriptions_path_in(root, serial).unwrap())
}

fn subscriptions_revision(registry_path: &Path, serial: &str) -> u64 {
    let registry: Value = serde_json::from_slice(&std::fs::read(registry_path).unwrap()).unwrap();
    registry["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["serial"] == serial)
        .unwrap()["subscriptions_revision"]
        .as_u64()
        .unwrap()
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[tokio::test]
async fn deletion_scrubs_a_and_b_preserves_unrelated_order_and_leaves_c_unchanged() {
    let sandbox = Sandbox::start(&[("RAW-A", 4), ("RAW-B", 8), ("RAW-C", 12)]).await;
    save_playlist(&sandbox.root, "gym");
    save_subscriptions(&sandbox.root, "RAW-A", &["before", "gym", "after"]);
    save_subscriptions(&sandbox.root, "RAW-B", &["gym", "other", "gym"]);
    let c_path = save_subscriptions(&sandbox.root, "RAW-C", &["other", "before"]);
    let c_before = std::fs::read(&c_path).unwrap();
    let mut client = sandbox.connect().await;

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":"delete-gym"}))
        .await;

    let mut changed_serials = Vec::new();
    loop {
        let event = client.next().await;
        match event["type"].as_str() {
            Some("device_config_update") => changed_serials.push(event["serial"].clone()),
            Some("playlists_update") if event["acknowledged_request_id"] == "delete-gym" => {
                assert_eq!(event["playlists"], json!([]));
                break;
            }
            _ => {}
        }
    }
    changed_serials.sort_by_key(|serial| serial.as_str().unwrap().to_string());

    assert_eq!(changed_serials, vec![json!("RAW-A"), json!("RAW-B")]);
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-A").playlists,
        ["before", "after"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-B").playlists,
        ["other"]
    );
    assert_eq!(std::fs::read(c_path).unwrap(), c_before);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 5);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-B"), 9);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-C"), 12);
    assert!(!sandbox.root.join("playlists/gym.m3u8").exists());
    assert!(!sandbox.root.join("devices/playlist-mutations").exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn missing_playlist_is_an_acknowledged_no_op() {
    let sandbox = Sandbox::start(&[("RAW-A", 2)]).await;
    let subscriptions_path = save_subscriptions(&sandbox.root, "RAW-A", &["ghost", "other"]);
    let before = std::fs::read(&subscriptions_path).unwrap();
    let mut client = sandbox.connect().await;

    client
        .send(json!({"type":"delete_playlist","slug":"ghost","request_id":"delete-missing"}))
        .await;

    let update = client.next_type("playlists_update").await;
    assert_eq!(update["acknowledged_request_id"], "delete-missing");
    assert_eq!(std::fs::read(subscriptions_path).unwrap(), before);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 2);
    client.assert_no_success_broadcast().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn registry_publish_failure_rolls_back_and_emits_no_success_broadcast() {
    let sandbox = Sandbox::start(&[("RAW-A", 3)]).await;
    save_playlist(&sandbox.root, "gym");
    let subscriptions_path = save_subscriptions(&sandbox.root, "RAW-A", &["gym", "other"]);
    let playlist_path = sandbox.root.join("playlists/gym.m3u8");
    let playlist_before = std::fs::read(&playlist_path).unwrap();
    let subscriptions_before = std::fs::read(&subscriptions_path).unwrap();
    let mut client = sandbox.connect().await;
    std::fs::remove_file(&sandbox.registry_path).unwrap();
    std::fs::create_dir(&sandbox.registry_path).unwrap();

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":"delete-fails"}))
        .await;

    client.assert_no_success_broadcast().await;
    assert_eq!(std::fs::read(playlist_path).unwrap(), playlist_before);
    assert_eq!(
        std::fs::read(subscriptions_path).unwrap(),
        subscriptions_before
    );
    assert!(!sandbox.root.join("devices/playlist-mutations").exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_rolls_forward_publishing_journal_and_next_preview_is_clean() {
    let root = test_root("recover-publishing");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("RAW-A", 6), ("RAW-B", 9)]);
    save_playlist(&root, "gym");
    save_playlist(&root, "keep-a");
    save_playlist(&root, "keep-b");
    let a_path = save_subscriptions(&root, "RAW-A", &["gym", "keep-a"]);
    let b_path = save_subscriptions(&root, "RAW-B", &["keep-b", "gym"]);
    let a_original = std::fs::read(&a_path).unwrap();
    let b_original = std::fs::read(&b_path).unwrap();
    let a_target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep-a".into()],
    })
    .unwrap();
    let b_target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep-b".into()],
    })
    .unwrap();
    let request_id = "recover-delete";
    let mutation_root = root.join("devices/playlist-mutations");
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    std::fs::create_dir_all(&stage_root).unwrap();
    let playlist_path = root.join("playlists/gym.m3u8");
    let playlist_original = std::fs::read(&playlist_path).unwrap();
    let playlist_stage = stage_root.join("playlist.original");
    std::fs::rename(&playlist_path, &playlist_stage).unwrap();
    let a_original_stage = stage_root.join("subscription-0.original");
    let a_target_stage = stage_root.join("subscription-0.target");
    let b_original_stage = stage_root.join("subscription-1.original");
    let b_target_stage = stage_root.join("subscription-1.target");
    std::fs::write(&a_original_stage, &a_original).unwrap();
    std::fs::write(&a_target_stage, &a_target).unwrap();
    std::fs::write(&b_original_stage, &b_original).unwrap();
    std::fs::write(&b_target_stage, &b_target).unwrap();
    std::fs::rename(&a_target_stage, &a_path).unwrap();
    write_mutation_journal(
        &root,
        request_id,
        "publishing",
        &playlist_path,
        &playlist_stage,
        &playlist_original,
        &[
            JournalSubscription {
                serial: "RAW-A",
                live: &a_path,
                original_stage: &a_original_stage,
                target_stage: &a_target_stage,
                original: &a_original,
                target: &a_target,
                original_revision: 6,
            },
            JournalSubscription {
                serial: "RAW-B",
                live: &b_path,
                original_stage: &b_original_stage,
                target_stage: &b_target_stage,
                original: &b_original,
                target: &b_target,
                original_revision: 9,
            },
        ],
    );

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let mut client = sandbox.connect().await;
    client
        .send(
            json!({"type":"preview_device","serial":"RAW-A","request_id":"preview-after-recovery"}),
        )
        .await;
    let preview = client.next_type("device_preview").await;

    assert_eq!(preview["acknowledged_request_id"], "preview-after-recovery");
    assert!(preview.get("unresolved_subscriptions").is_none());
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-A").playlists,
        ["keep-a"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-B").playlists,
        ["keep-b"]
    );
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 7);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-B"), 10);
    assert!(!playlist_path.exists());
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_restores_prepared_journal_without_mutating_live_state() {
    let root = test_root("recover-prepared");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("RAW-A", 11)]);
    save_playlist(&root, "gym");
    let subscriptions_path = save_subscriptions(&root, "RAW-A", &["gym", "keep"]);
    let original = std::fs::read(&subscriptions_path).unwrap();
    let target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep".into()],
    })
    .unwrap();
    let request_id = "prepared-delete";
    let mutation_root = root.join("devices/playlist-mutations");
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    std::fs::create_dir_all(&stage_root).unwrap();
    let playlist_path = root.join("playlists/gym.m3u8");
    let playlist_original = std::fs::read(&playlist_path).unwrap();
    let playlist_stage = stage_root.join("playlist.original");
    let original_stage = stage_root.join("subscription-0.original");
    let target_stage = stage_root.join("subscription-0.target");
    std::fs::write(&original_stage, &original).unwrap();
    std::fs::write(&target_stage, &target).unwrap();
    write_mutation_journal(
        &root,
        request_id,
        "prepared",
        &playlist_path,
        &playlist_stage,
        &playlist_original,
        &[JournalSubscription {
            serial: "RAW-A",
            live: &subscriptions_path,
            original_stage: &original_stage,
            target_stage: &target_stage,
            original: &original,
            target: &target,
            original_revision: 11,
        }],
    );

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let _client = sandbox.connect().await;

    assert_eq!(std::fs::read(&subscriptions_path).unwrap(), original);
    assert_eq!(std::fs::read(&playlist_path).unwrap(), playlist_original);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 11);
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}

struct JournalSubscription<'a> {
    serial: &'a str,
    live: &'a Path,
    original_stage: &'a Path,
    target_stage: &'a Path,
    original: &'a [u8],
    target: &'a [u8],
    original_revision: u64,
}

fn write_mutation_journal(
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

impl Sandbox {
    async fn start_from_existing(
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
}
