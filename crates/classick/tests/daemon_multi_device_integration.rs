//! Cross-platform daemon integration coverage for serial-keyed device state.

use classick::config_file::{DaemonSettings, IpodIdentity, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::{HistoryEntry, HistoryFile, SyncOutcome, SyncSummary, SyncTrigger};
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps, SpawnFn};
use classick::daemon::session_admission::EventContext;
use classick::daemon::source_availability::{
    BoxFuture, MountInteraction, SourceAvailabilityService, SourceMountBackend, SourceUnavailable,
};
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use classick::ipod::device::DetectedIpod;
use classick::portable_path::PortablePath;
use classick::source_location::{SourceIdentity, SourceLocation};
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

struct AuthRequiredBackend;

impl SourceMountBackend for AuthRequiredBackend {
    fn mount<'a>(
        &'a self,
        _location: &'a SourceLocation,
        _interaction: MountInteraction,
    ) -> BoxFuture<'a, Result<PathBuf, SourceUnavailable>> {
        Box::pin(async { Err(SourceUnavailable::AuthRequired) })
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

    async fn next_type_within(&mut self, event_type: &str, timeout: Duration) -> Option<Value> {
        tokio::time::timeout(timeout, async {
            loop {
                let value = read_json(&mut self.reader).await;
                if value["type"] == event_type {
                    break value;
                }
            }
        })
        .await
        .ok()
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
    config_path: PathBuf,
    history_path: PathBuf,
    pipe_name: String,
    device_tx: mpsc::Sender<DeviceEvent>,
    spawn_rx: mpsc::UnboundedReceiver<SpawnCall>,
    scan_finish_rx: mpsc::UnboundedReceiver<oneshot::Sender<OrchestratorOutcome>>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Sandbox {
    async fn start(records: &[(&str, bool)]) -> Self {
        Self::start_with_auth_required_source(records, false).await
    }

    async fn start_with_auth_required_source(
        records: &[(&str, bool)],
        auth_required: bool,
    ) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("daemon-multi-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = if auth_required {
            base.join("missing/media/music")
        } else {
            let source = base.join("source");
            std::fs::create_dir_all(&source).unwrap();
            source
        };
        let source_location = auth_required.then(|| SourceLocation {
            resolved_path: source.clone(),
            identity: SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(PortablePath::parse("media/music").unwrap()),
            },
        });
        let config_path = base.join("config.toml");
        let legacy = records
            .iter()
            .find(|(_, configured)| *configured)
            .map(|(serial, _)| identity(serial));
        classick::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source),
                source_location,
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
        let history_path = base.join("history.json");
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
            history_path: Some(history_path.clone()),
            pipe_name: Some(pipe_name.clone()),
            source_availability: auth_required
                .then(|| SourceAvailabilityService::new(Arc::new(AuthRequiredBackend))),
        };
        let runtime = tokio::spawn(run_daemon_with_deps(deps));
        Self {
            base,
            config_path,
            history_path,
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

    async fn stop_keep_files(self) -> PathBuf {
        self.runtime.abort();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.pipe_name);
        self.base
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
async fn fresh_client_replays_failed_startup_source_state_after_snapshot() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let sandbox = Sandbox::start_with_auth_required_source(&[], true).await;
    let mut first = sandbox.connect().await;
    let failed = first.next_type("source_availability").await;
    assert_eq!(failed["state"], "auth_required");
    assert!(failed["acknowledged_request_id"].is_null());

    let mut fresh = sandbox.connect().await;
    sandbox.attach("RAW-LIVE", "/Volumes/LIVE").await;
    let status = fresh.next().await;
    let inventory = fresh.next().await;
    let replay = fresh.next().await;
    assert_eq!(status["type"], "status_update");
    assert_eq!(inventory["type"], "device_inventory_snapshot");
    assert_eq!(replay["type"], "source_availability");
    assert_eq!(replay["state"], "auth_required");
    assert!(replay["acknowledged_request_id"].is_null());
    assert!(
        first
            .next_type_within("source_availability", Duration::from_millis(150))
            .await
            .is_none(),
        "client B's initial source replay must not be broadcast to client A"
    );
    sandbox.shutdown().await;
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

#[tokio::test]
async fn detached_sync_drains_before_releasing_admission_and_records_one_aborted_attempt() {
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
    sandbox.finish_startup_scan().await;
    let _ = client.next_type("device_inventory_snapshot").await;

    client
        .send(json!({
            "type": "trigger_sync",
            "source": "manual",
            "serial": "raw-a",
            "request_id": "sync-a"
        }))
        .await;
    let SpawnCall {
        context,
        mut cancel,
        finish,
        ..
    } = sandbox.spawn_rx.recv().await.unwrap();
    let history_tmp = sandbox.history_path.with_extension("json.tmp");
    std::fs::create_dir(&history_tmp).unwrap();

    sandbox.disconnect(" raw-a ").await;
    tokio::time::timeout(Duration::from_secs(5), &mut cancel)
        .await
        .expect("detach signals cancellation")
        .expect("cancel channel remains connected");
    let detached = client
        .next_snapshot_where(|snapshot| {
            let a = snapshot_device(snapshot, "RAW-A");
            a["connected"] == false && a["latest_attempt"].is_object()
        })
        .await;
    let a = snapshot_device(&detached, "RAW-A");
    assert_eq!(a["session_id"], context.session_id);
    assert_eq!(a["latest_attempt"]["serial"], "RAW-A");
    assert_eq!(a["latest_attempt"]["outcome"], "aborted");
    assert_eq!(a["latest_attempt"]["error_message"], "device_detached");
    assert!(a["last_terminal_error"]
        .as_str()
        .is_some_and(|message| message.contains("persist sync history")));

    sandbox.disconnect("RAW-A").await;

    client
        .send(json!({
            "type": "trigger_sync",
            "source": "manual",
            "serial": "RAW-B",
            "request_id": "b-before-a-drains"
        }))
        .await;
    let rejection = client.next_type("sync_rejected").await;
    assert_eq!(rejection["reason"], "already_syncing");
    assert!(sandbox.spawn_rx.try_recv().is_err());

    std::fs::remove_dir(history_tmp).unwrap();
    finish.send(completed(None)).unwrap();
    let drained = client
        .next_snapshot_where(|snapshot| {
            let a = snapshot_device(snapshot, "RAW-A");
            a["session_id"].is_null()
                && a["latest_attempt"]["error_message"] == "device_detached"
                && a["last_terminal_error"] == "device_detached"
        })
        .await;
    assert_eq!(snapshot_device(&drained, "RAW-A")["phase"], "disconnected");

    client
        .send(json!({"type": "get_history", "limit": 50, "request_id": "history"}))
        .await;
    let history = client.next_type("history_update").await;
    let entries = history["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "detach must create one terminal history entry"
    );
    assert_eq!(entries[0]["serial"], "RAW-A");
    assert_eq!(entries[0]["outcome"], "aborted");

    client
        .send(json!({
            "type": "trigger_sync",
            "source": "manual",
            "serial": "RAW-B",
            "request_id": "b-after-a-drains"
        }))
        .await;
    let b = sandbox
        .spawn_rx
        .recv()
        .await
        .expect("B admitted after A drained");
    assert_eq!(b.serial, "RAW-B");
    b.finish.send(completed(None)).unwrap();
    sandbox.shutdown().await;
}

#[tokio::test]
async fn v2_history_replies_filter_legacy_entries_and_preserve_scoped_entries() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    let entry = |serial: &str, timestamp: &str| HistoryEntry {
        serial: serial.to_string(),
        session_id: None,
        timestamp: timestamp.to_string(),
        duration_secs: 1,
        trigger: SyncTrigger::Manual,
        outcome: SyncOutcome::Ok,
        error_message: None,
        summary: None,
        db_restored: false,
    };
    std::fs::write(
        &sandbox.history_path,
        serde_json::to_vec_pretty(&HistoryFile {
            version: 1,
            entries: vec![
                entry("", "2026-07-18T10:00:00Z"),
                entry("RAW-A", "2026-07-18T10:01:00Z"),
                entry("", "2026-07-18T10:02:00Z"),
            ],
        })
        .unwrap(),
    )
    .unwrap();

    client
        .send(json!({"type": "get_history", "limit": 50, "request_id": "history"}))
        .await;
    let history = client.next_type("history_update").await;
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);
    assert_eq!(history["entries"][0]["serial"], "RAW-A");

    client
        .send(json!({"type": "get_status", "request_id": "status"}))
        .await;
    let status = client.next_type("status_update").await;
    assert_eq!(status["last_sync"]["serial"], "RAW-A");

    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn canonical_alias_uses_registry_raw_serial_for_device_config_paths() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("0xraw-b", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;

    client
        .send(json!({
            "type": "save_device_config",
            "serial": " RAW-B ",
            "settings": {"auto_sync": false, "rockbox_compat": true},
            "request_id": "alias-settings"
        }))
        .await;
    let update = client.next_type("device_config_update").await;
    assert_eq!(update["serial"], "0xraw-b");
    assert_eq!(update["acknowledged_request_id"], "alias-settings");

    let raw_path =
        classick::device_state::device_settings_path_in(&sandbox.base, "0xraw-b").unwrap();
    let requester_path =
        classick::device_state::device_settings_path_in(&sandbox.base, " RAW-B ").unwrap();
    assert!(raw_path.exists(), "registry raw serial must own the path");
    assert_ne!(raw_path, requester_path);
    assert!(
        !requester_path.exists(),
        "request spelling must never own state"
    );
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn failed_global_config_write_does_not_configure_device() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true), ("RAW-B", false)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    let config_tmp = sandbox.config_path.with_extension("toml.tmp");
    std::fs::create_dir(&config_tmp).unwrap();

    client
        .send(json!({
            "type": "save_config",
            "source": sandbox.base.join("replacement-source").to_string_lossy(),
            "ipod": {"serial": "RAW-B", "model_label": "iPod Classic"},
            "request_id": "config-failure"
        }))
        .await;
    let snapshot = client.next_type("device_inventory_snapshot").await;
    assert!(!snapshot_device(&snapshot, "RAW-B")["configured"]
        .as_bool()
        .unwrap());

    std::fs::remove_dir(config_tmp).unwrap();
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn failed_registry_forget_keeps_legacy_input_and_registry_consistent() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    let registry_path = classick::config_file::device_registry_path(&sandbox.config_path);
    let registry_bytes = std::fs::read(&registry_path).unwrap();
    std::fs::remove_file(&registry_path).unwrap();
    std::fs::create_dir(&registry_path).unwrap();

    client
        .send(json!({
            "type": "forget_ipod",
            "serial": "raw-a",
            "request_id": "forget-failure"
        }))
        .await;
    let snapshot = client.next_type("device_inventory_snapshot").await;
    assert!(snapshot_device(&snapshot, "RAW-A")["configured"]
        .as_bool()
        .unwrap());
    let config = classick::config_file::load(&sandbox.config_path)
        .unwrap()
        .unwrap();
    assert_eq!(config.ipod_identity.unwrap().serial, "RAW-A");

    std::fs::remove_dir(&registry_path).unwrap();
    std::fs::write(&registry_path, registry_bytes).unwrap();
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn failed_registry_configure_acknowledges_only_actual_global_and_device_authorities() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true), ("RAW-B", false)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    let registry_path = classick::config_file::device_registry_path(&sandbox.config_path);
    let registry_bytes = std::fs::read(&registry_path).unwrap();
    std::fs::remove_file(&registry_path).unwrap();
    std::fs::create_dir(&registry_path).unwrap();
    let replacement = sandbox.base.join("replacement-source");
    std::fs::create_dir_all(&replacement).unwrap();

    client
        .send(json!({
            "type": "save_config",
            "source": replacement.to_string_lossy(),
            "ipod": {"serial": "RAW-B", "model_label": "iPod Classic"},
            "request_id": "registry-configure-failure"
        }))
        .await;
    let update = client.next_type("config_update").await;
    assert_eq!(
        update["acknowledged_request_id"],
        "registry-configure-failure"
    );
    assert_eq!(update["source"], replacement.to_string_lossy().as_ref());
    assert_eq!(update["ipod"]["serial"], "RAW-A");
    let snapshot = client.next_type("device_inventory_snapshot").await;
    assert!(!snapshot_device(&snapshot, "RAW-B")["configured"]
        .as_bool()
        .unwrap());

    std::fs::remove_dir(&registry_path).unwrap();
    std::fs::write(&registry_path, registry_bytes).unwrap();
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn forget_keeps_connected_device_unconfigured_and_restart_does_not_remigrate_legacy() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    sandbox.attach("RAW-A", "/Volumes/A").await;
    let _ = client
        .next_snapshot_where(|snapshot| snapshot_device(snapshot, "RAW-A")["connected"] == true)
        .await;

    client
        .send(json!({
            "type": "forget_ipod",
            "serial": " raw-a ",
            "request_id": "forget-a"
        }))
        .await;
    let forgotten = client
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-A")["connected"] == true
                && snapshot_device(snapshot, "RAW-A")["configured"] == false
        })
        .await;
    assert_eq!(
        snapshot_device(&forgotten, "RAW-A")["phase"],
        "unconfigured"
    );
    assert_eq!(
        classick::config_file::load(&sandbox.config_path)
            .unwrap()
            .unwrap()
            .ipod_identity
            .unwrap()
            .serial,
        "RAW-A",
        "legacy identity remains migration input, not live authority"
    );
    client
        .send(json!({"type": "get_config", "request_id": "after-forget"}))
        .await;
    let update = client.next_type("config_update").await;
    assert!(update["ipod"].is_null());
    assert_eq!(update["acknowledged_request_id"], "after-forget");

    sandbox.finish_startup_scan().await;
    let base = sandbox.stop_keep_files().await;
    let config_path = base.join("config.toml");
    let registry =
        std::fs::read_to_string(classick::config_file::device_registry_path(&config_path)).unwrap();
    let registry: Value = serde_json::from_str(&registry).unwrap();
    assert_eq!(registry["records"][0]["configured"], false);
    let _ = std::fs::remove_dir_all(base);
}

#[tokio::test]
async fn save_device_config_advances_only_successfully_persisted_components() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    let subscriptions_path =
        classick::device_state::device_subscriptions_path_in(&sandbox.base, "RAW-A").unwrap();
    let subscriptions_tmp = subscriptions_path.with_extension("json.tmp");
    std::fs::create_dir(&subscriptions_tmp).unwrap();

    client
        .send(json!({
            "type": "save_device_config",
            "serial": "raw-a",
            "selection": {"mode": "include", "rules": []},
            "subscriptions": {"playlists": ["must-not-persist"]},
            "settings": {"auto_sync": false, "rockbox_compat": true},
            "request_id": "mixed-config"
        }))
        .await;
    let update = client.next_type("device_config_update").await;
    assert_eq!(update["serial"], "RAW-A");
    assert_eq!(update["selection"]["mode"], "include");
    assert_eq!(update["subscriptions"]["playlists"], json!([]));
    assert_eq!(update["settings"]["auto_sync"], false);
    assert_eq!(update["settings"]["rockbox_compat"], true);
    let snapshot = client
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-A")["selection_revision"] == 1
        })
        .await;
    let a = snapshot_device(&snapshot, "RAW-A");
    assert_eq!(a["selection_revision"], 1);
    assert_eq!(a["subscriptions_revision"], 0);
    assert_eq!(a["settings_revision"], 1);

    std::fs::remove_dir(subscriptions_tmp).unwrap();
    sandbox.finish_startup_scan().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn history_persist_failure_retains_truthful_terminal_attempt_in_snapshot() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start(&[("RAW-A", true)]).await;
    let mut client = sandbox.connect().await;
    let _ = client.next_type("device_inventory_snapshot").await;
    sandbox.attach("RAW-A", "/Volumes/A").await;
    let _ = client
        .next_snapshot_where(|snapshot| snapshot_device(snapshot, "RAW-A")["connected"] == true)
        .await;
    sandbox.finish_startup_scan().await;
    let _ = client.next_type("device_inventory_snapshot").await;

    client
        .send(json!({
            "type": "trigger_sync",
            "source": "manual",
            "serial": "raw-a",
            "request_id": "sync-a"
        }))
        .await;
    let call = sandbox.spawn_rx.recv().await.unwrap();
    let history_tmp = sandbox.history_path.with_extension("json.tmp");
    std::fs::create_dir(&history_tmp).unwrap();
    call.finish.send(completed(None)).unwrap();

    let snapshot = client
        .next_snapshot_where(|snapshot| {
            snapshot_device(snapshot, "RAW-A")["last_terminal_error"]
                .as_str()
                .is_some_and(|message| message.contains("persist sync history"))
        })
        .await;
    let a = snapshot_device(&snapshot, "RAW-A");
    assert_eq!(a["phase"], "error");
    assert_eq!(a["latest_attempt"]["outcome"], "ok");
    assert_eq!(a["latest_attempt"]["serial"], "RAW-A");
    assert!(a["latest_successful_sync"].is_object());

    std::fs::remove_dir(history_tmp).unwrap();
    sandbox.shutdown().await;
}
