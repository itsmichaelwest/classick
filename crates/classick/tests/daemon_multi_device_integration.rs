//! Protocol-3 daemon coverage for independent, canonically identified devices.

use classick::config_file::{DaemonSettings, IpodIdentity, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::SyncSummary;
use classick::daemon::lifecycle::ShutdownReason;
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

const DEVICE_A: &str = "000A27002138B0A8";
const DEVICE_B: &str = "000A27002138B0B9";
const DEVICE_C: &str = "000A27002138B0CA";
const REQUEST_SYNC_B: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8101";
const REQUEST_SYNC_C: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8102";

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
    cancel: oneshot::Receiver<()>,
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
        assert_eq!(hello["protocol_version"], "3.0.0");
        assert_eq!(hello["role"], "daemon");
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

    async fn next_inventory_where(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            let value = self.next_type("device_inventory").await;
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
            .join("target/test-tmp")
            .join(format!("daemon-v3-multi-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        let legacy = records
            .iter()
            .find(|(_, configured)| *configured)
            .map(|(device_id, _)| identity(device_id));
        classick::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source),
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
        let (_shutdown_tx, shutdown_rx) = mpsc::unbounded_channel::<ShutdownReason>();
        let spawn_sync: SpawnFn = Arc::new(move |serial, drive, cancel, _, _, context| {
            let (finish, finished) = oneshot::channel();
            spawn_tx
                .send(SpawnCall {
                    serial,
                    drive,
                    context,
                    cancel,
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
            config_path: Some(config_path.clone()),
            history_path: Some(base.join("history.json")),
            pipe_name: Some(pipe_name.clone()),
            source_availability: None,
            shutdown_rx,
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

    async fn attach(&self, device_id: &str, drive: &str) {
        self.device_tx
            .send(DeviceEvent::Connected(device(device_id, drive)))
            .await
            .unwrap();
    }

    async fn disconnect(&self, device_id: &str) {
        self.device_tx
            .send(DeviceEvent::Disconnected {
                serial: device_id.to_string(),
            })
            .await
            .unwrap();
    }

    async fn finish_startup_scan(&mut self) {
        self.scan_finish_rx
            .recv()
            .await
            .expect("startup scan")
            .send(completed())
            .unwrap();
    }

    async fn shutdown(self) {
        self.runtime.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.pipe_name);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn unique_pipe_name(base: &Path, _n: u32) -> String {
    #[cfg(windows)]
    return format!(r"\\.\pipe\classick-v3-multi-{}-{_n}", std::process::id());
    #[cfg(not(windows))]
    return base.join("daemon.sock").to_string_lossy().into_owned();
}

fn identity(device_id: &str) -> IpodIdentity {
    IpodIdentity {
        serial: device_id.to_string(),
        model_label: "iPod Classic".to_string(),
        name: Some(format!("Device {device_id}")),
        custom_selection: false,
    }
}

fn device(device_id: &str, drive: &str) -> DetectedIpod {
    DetectedIpod {
        serial: device_id.to_string(),
        model_label: "iPod Classic".to_string(),
        drive: drive.to_string(),
        name: Some(format!("Device {device_id}")),
        volume_guid: None,
    }
}

fn write_registry(config_path: &Path, records: &[(&str, bool)]) {
    let path = classick::config_file::device_registry_path(config_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records: Vec<_> = records
        .iter()
        .map(|(device_id, configured)| {
            json!({
                "serial": device_id,
                "model_label": "iPod Classic",
                "name": format!("Device {device_id}"),
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

fn completed() -> OrchestratorOutcome {
    OrchestratorOutcome::Completed {
        summary: SyncSummary::default(),
        db_restored: false,
    }
}

fn snapshot_device<'a>(snapshot: &'a Value, device_id: &str) -> &'a Value {
    snapshot["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["device_id"] == device_id)
        .unwrap_or_else(|| panic!("snapshot missing {device_id}: {snapshot}"))
}

#[tokio::test]
async fn inventory_keeps_two_devices_independent_across_disconnect_and_reconnect() {
    let _guard = SERIAL_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut sandbox = Sandbox::start(&[(DEVICE_A, true)]).await;
    let mut first = sandbox.connect().await;
    let initial = first.next_type("device_inventory").await;
    assert_eq!(initial["devices"].as_array().unwrap().len(), 1);
    assert!(!snapshot_device(&initial, DEVICE_A)["connected"]
        .as_bool()
        .unwrap());

    sandbox.attach(DEVICE_A, "/Volumes/A").await;
    sandbox.attach(DEVICE_B, "/Volumes/B").await;
    let both = first
        .next_inventory_where(|snapshot| {
            snapshot["devices"]
                .as_array()
                .is_some_and(|devices| devices.len() == 2)
                && snapshot_device(snapshot, DEVICE_A)["connected"] == true
                && snapshot_device(snapshot, DEVICE_B)["connected"] == true
        })
        .await;
    assert_eq!(
        snapshot_device(&both, DEVICE_A)["profile_status"],
        "pending_adoption"
    );
    assert_eq!(
        snapshot_device(&both, DEVICE_B)["profile_status"],
        "not_adopted"
    );

    sandbox.disconnect(DEVICE_A).await;
    let detached = first
        .next_inventory_where(|snapshot| {
            snapshot_device(snapshot, DEVICE_A)["connected"] == false
                && snapshot_device(snapshot, DEVICE_B)["connected"] == true
        })
        .await;

    let mut fresh = sandbox.connect().await;
    let fresh_snapshot = fresh.next_type("device_inventory").await;
    assert_eq!(fresh_snapshot["devices"], detached["devices"]);
    assert!(fresh_snapshot["revision"].as_u64() > detached["revision"].as_u64());
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn sync_admission_targets_the_requested_device_and_drains_detach_before_release() {
    let _guard = SERIAL_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut sandbox = Sandbox::start(&[(DEVICE_A, true), (DEVICE_B, true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory").await;
    sandbox.attach(DEVICE_A, "/Volumes/A").await;
    sandbox.attach(DEVICE_B, "/Volumes/B").await;
    let _ = client
        .next_inventory_where(|snapshot| {
            snapshot_device(snapshot, DEVICE_A)["connected"] == true
                && snapshot_device(snapshot, DEVICE_B)["connected"] == true
        })
        .await;

    client
        .send(json!({
            "type": "trigger_sync",
            "device_id": DEVICE_C,
            "request_id": REQUEST_SYNC_C,
            "trigger": "manual"
        }))
        .await;
    let rejected = client.next_type("sync_rejected").await;
    assert_eq!(rejected["device_id"], DEVICE_C);
    assert_eq!(rejected["request_id"], REQUEST_SYNC_C);
    assert_eq!(rejected["reason"], "not_adopted");

    sandbox.finish_startup_scan().await;
    client
        .send(json!({
            "type": "trigger_sync",
            "device_id": DEVICE_B,
            "request_id": REQUEST_SYNC_B,
            "trigger": "manual"
        }))
        .await;
    let call = sandbox.spawn_rx.recv().await.unwrap();
    assert_eq!(call.serial, DEVICE_B);
    assert_eq!(call.drive, "/Volumes/B");
    assert_eq!(call.context.serial.as_deref(), Some(DEVICE_B));

    sandbox.disconnect(DEVICE_B).await;
    tokio::time::timeout(Duration::from_secs(5), call.cancel)
        .await
        .expect("detach signals cancellation")
        .expect("cancel channel remains connected");
    call.finish.send(completed()).unwrap();
    sandbox.shutdown().await;
}
