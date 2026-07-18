//! Cross-platform daemon integration coverage for serial-keyed device state.

use classick::config_file::{DaemonSettings, IpodIdentity, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::{SyncOutcome, SyncSummary};
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps, SpawnFn};
use classick::daemon::session_admission::EventContext;
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use classick::ipod::device::DetectedIpod;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

static SERIAL_TESTS: Mutex<()> = Mutex::new(());

struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);

impl DeviceWatcher for ScriptedWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        self.0
    }
}

struct SpawnCall {
    serial: String,
    drive: String,
    context: EventContext,
    finish: oneshot::Sender<OrchestratorOutcome>,
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
        let hello = client.next().await;
        assert_eq!(hello["type"], "hello");
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
            let value = self.next().await;
            if value["type"] == event_type {
                return value;
            }
        }
    }

    async fn next_snapshot_where(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            let value = self.next_type("device_inventory_snapshot").await;
            if predicate(&value) {
                return value;
            }
        }
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
            Err(error) if std::time::Instant::now() < deadline => {
                let _ = error;
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
            Err(error) if std::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("connect to {pipe_name}: {error}"),
        }
    }
}

struct Sandbox {
    base: PathBuf,
    pipe_name: String,
    device_tx: mpsc::Sender<DeviceEvent>,
    spawn_rx: mpsc::UnboundedReceiver<SpawnCall>,
    scan_finish_rx: mpsc::UnboundedReceiver<oneshot::Sender<OrchestratorOutcome>>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Sandbox {
    async fn start(records: &[(&str, bool)]) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("daemon-multi-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("source")).unwrap();
        let config_path = base.join("config.toml");
        let legacy = records
            .iter()
            .find(|(_, configured)| *configured)
            .map(|(serial, _)| identity(serial));
        classick::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(base.join("source")),
                daemon: Some(DaemonSettings {
                    enabled: false,
                    schedule_minutes: 0,
                    ..Default::default()
                }),
                ipod_identity: legacy.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        write_registry(&config_path, records);

        let pipe_name = unique_pipe_name(&base, n);
        let (device_tx, device_rx) = mpsc::channel(8);
        let (spawn_tx, spawn_rx) = mpsc::unbounded_channel();
        let (scan_finish_tx, scan_finish_rx) = mpsc::unbounded_channel();
        let spawn_sync: SpawnFn = Arc::new(move |serial, drive, _, _, _, context| {
            let (finish, finished) = oneshot::channel();
            spawn_tx
                .send(SpawnCall {
                    serial,
                    drive,
                    context,
                    finish,
                })
                .unwrap();
            Box::pin(async move { Ok(finished.await.unwrap()) })
        });
        let spawn_scan: SpawnFn = Arc::new(move |_, _, _, _, _, _| {
            let (finish, finished) = oneshot::channel();
            scan_finish_tx.send(finish).unwrap();
            Box::pin(async move { Ok(finished.await.unwrap()) })
        });
        let deps = DaemonDeps {
            configured_serial: legacy.map(|identity| identity.serial),
            watcher: Box::new(ScriptedWatcher(device_rx)),
            spawn_sync: spawn_sync.clone(),
            spawn_backfill: spawn_sync.clone(),
            spawn_replace_library: spawn_sync,
            spawn_scan,
            schedule_minutes: 0,
            preset_event_tx: None,
            config_path: Some(config_path),
            history_path: Some(base.join("history.json")),
            pipe_name: Some(pipe_name.clone()),
        };
        let runtime = tokio::spawn(run_daemon_with_deps(deps));
        Self {
            base,
            pipe_name,
            device_tx,
            spawn_rx,
            scan_finish_rx,
            runtime,
        }
    }

    async fn connect(&self) -> TestClient {
        TestClient::connect(&self.pipe_name).await
    }

    async fn attach(&self, serial: &str, drive: &str) {
        self.device_tx
            .send(DeviceEvent::Connected(device(serial, drive)))
            .await
            .unwrap();
    }

    async fn disconnect(&self, serial: &str) {
        self.device_tx
            .send(DeviceEvent::Disconnected {
                serial: serial.to_string(),
            })
            .await
            .unwrap();
    }

    async fn finish_startup_scan(&mut self) {
        self.scan_finish_rx
            .recv()
            .await
            .expect("startup scan")
            .send(completed(None))
            .unwrap();
    }

    async fn shutdown(self) {
        self.runtime.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.pipe_name);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn unique_pipe_name(_base: &Path, _n: u32) -> String {
    #[cfg(windows)]
    return format!(r"\\.\pipe\classick-multi-{}-{_n}", std::process::id());
    #[cfg(not(windows))]
    return _base.join("daemon.sock").to_string_lossy().into_owned();
}

fn identity(serial: &str) -> IpodIdentity {
    IpodIdentity {
        serial: serial.to_string(),
        model_label: "iPod Classic".to_string(),
        name: Some(format!("Device {serial}")),
        custom_selection: false,
    }
}

fn device(serial: &str, drive: &str) -> DetectedIpod {
    DetectedIpod {
        serial: serial.to_string(),
        model_label: "iPod Classic".to_string(),
        drive: drive.to_string(),
        name: Some(format!("Device {serial}")),
        volume_guid: None,
    }
}

fn write_registry(config_path: &Path, records: &[(&str, bool)]) {
    let path = classick::config_file::device_registry_path(config_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records: Vec<_> = records
        .iter()
        .map(|(serial, configured)| {
            json!({
                "serial": serial,
                "model_label": "iPod Classic",
                "name": format!("Device {serial}"),
                "configured": configured,
                "selection_revision": 0,
                "settings_revision": 0,
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

fn completed(summary: Option<SyncSummary>) -> OrchestratorOutcome {
    OrchestratorOutcome::Completed {
        outcome: SyncOutcome::Ok,
        summary,
        db_restored: false,
    }
}

fn devices(snapshot: &Value) -> &[Value] {
    snapshot["devices"].as_array().unwrap()
}

fn snapshot_device<'a>(snapshot: &'a Value, serial: &str) -> &'a Value {
    devices(snapshot)
        .iter()
        .find(|device| device["identity"]["serial"] == serial)
        .unwrap_or_else(|| panic!("snapshot missing {serial}: {snapshot}"))
}

#[tokio::test]
async fn a_and_unconfigured_b_coexist_disconnect_a_preserves_b_and_fresh_client_gets_all() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true)]).await;
    let mut first = sandbox.connect().await;
    let initial = first.next_type("device_inventory_snapshot").await;
    assert_eq!(devices(&initial).len(), 1);
    assert!(!snapshot_device(&initial, "RAW-A")["connected"]
        .as_bool()
        .unwrap());

    sandbox.attach("RAW-A", "/Volumes/A").await;
    sandbox.attach("RAW-B", "/Volumes/B").await;
    let both = first
        .next_snapshot_where(|snapshot| {
            devices(snapshot).len() == 2
                && snapshot_device(snapshot, "RAW-A")["connected"] == true
                && snapshot_device(snapshot, "RAW-B")["connected"] == true
        })
        .await;
    assert!(snapshot_device(&both, "RAW-A")["configured"]
        .as_bool()
        .unwrap());
    assert!(!snapshot_device(&both, "RAW-B")["configured"]
        .as_bool()
        .unwrap());
    assert_eq!(snapshot_device(&both, "RAW-B")["phase"], "unconfigured");

    sandbox.disconnect("RAW-A").await;
    let detached = first
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-A")["connected"] == false
                && snapshot_device(snapshot, "RAW-B")["connected"] == true
        })
        .await;
    assert_eq!(devices(&detached).len(), 2);

    let mut fresh = sandbox.connect().await;
    let fresh_snapshot = fresh.next_type("device_inventory_snapshot").await;
    assert_eq!(fresh_snapshot["devices"], detached["devices"]);
    assert!(fresh_snapshot["revision"].as_u64() > detached["revision"].as_u64());
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn exact_targeting_uses_b_drive_and_rejects_unknown_occupied_and_stale_targets() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true), ("RAW-B", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    sandbox.attach("RAW-A", "/Volumes/A").await;
    sandbox.attach("RAW-B", "/Volumes/B").await;
    let _ = client
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-A")["connected"] == true
                && snapshot_device(snapshot, "RAW-B")["connected"] == true
        })
        .await;

    client
        .send(json!({"type":"trigger_sync","source":"manual","serial":" raw-x ","request_id":"unknown-x"}))
        .await;
    let unknown = client.next_type("sync_rejected").await;
    assert_eq!(unknown["reason"], "not_configured");
    assert_eq!(unknown["serial"], " raw-x ");
    assert_eq!(unknown["acknowledged_request_id"], "unknown-x");
    assert!(
        sandbox.spawn_rx.try_recv().is_err(),
        "unknown X must not target A"
    );

    client
        .send(json!({"type":"trigger_sync","source":"manual","serial":"RAW-B","request_id":"occupied-b"}))
        .await;
    let occupied = client.next_type("sync_rejected").await;
    assert_eq!(occupied["reason"], "already_syncing");
    assert_eq!(occupied["serial"], "RAW-B");
    assert_eq!(occupied["acknowledged_request_id"], "occupied-b");

    sandbox.finish_startup_scan().await;
    let _ = client
        .next_snapshot_where(|snapshot| snapshot["devices"].is_array())
        .await;
    client
        .send(
            json!({"type":"trigger_sync","source":"manual","serial":"RAW-B","request_id":"sync-b"}),
        )
        .await;
    let call = tokio::time::timeout(Duration::from_secs(5), sandbox.spawn_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.serial, "RAW-B");
    assert_eq!(call.drive, "/Volumes/B");
    assert_eq!(call.context.serial.as_deref(), Some("RAW-B"));

    client
        .send(json!({"type":"pause","serial":"RAW-A","request_id":"stale-a"}))
        .await;
    let stale = client.next_type("sync_rejected").await;
    assert_eq!(stale["reason"], "already_syncing");
    assert_eq!(stale["serial"], "RAW-A");
    assert_eq!(stale["acknowledged_request_id"], "stale-a");

    call.finish
        .send(completed(Some(SyncSummary {
            add: 2,
            modify: 0,
            remove: 0,
            unchanged: 3,
            skipped: 0,
            metadata_only: 0,
            skipped_for_space_tracks: 0,
            skipped_for_space_bytes: 0,
            artwork_failed_sources: 0,
        })))
        .unwrap();
    let completed_snapshot = client
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-B")["latest_attempt"].is_object()
        })
        .await;
    assert!(snapshot_device(&completed_snapshot, "RAW-A")["latest_attempt"].is_null());
    assert_eq!(
        snapshot_device(&completed_snapshot, "RAW-B")["latest_attempt"]["serial"],
        "RAW-B"
    );
    assert_eq!(
        snapshot_device(&completed_snapshot, "RAW-B")["phase"],
        "idle"
    );
    sandbox.shutdown().await;
}
