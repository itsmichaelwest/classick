//! Cross-platform coverage that every daemon shutdown source reuses the
//! existing bounded cancellation/finalization drain.

use classick::config_file::{DaemonSettings, IpodIdentity, PersistedConfig};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::SyncSummary;
use classick::daemon::lifecycle::ShutdownReason;
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps, SpawnFn};
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use classick::ipod::device::DetectedIpod;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

static SERIAL_TESTS: Mutex<()> = Mutex::new(());
const DEVICE_ID: &str = "000A27002138B0A8";

struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);

impl DeviceWatcher for ScriptedWatcher {
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
        client
    }

    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn next(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.reader.read_line(&mut line))
            .await
            .expect("daemon event within five seconds")
            .unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    async fn next_snapshot_where(&mut self, predicate: impl Fn(&Value) -> bool) {
        loop {
            let value = self.next().await;
            if value["type"] == "device_inventory" && predicate(&value) {
                return;
            }
        }
    }
}

struct Sandbox {
    base: PathBuf,
    pipe_name: String,
    device_tx: mpsc::Sender<DeviceEvent>,
    shutdown_tx: mpsc::UnboundedSender<ShutdownReason>,
    sync_started_rx: oneshot::Receiver<()>,
    cancel_seen_rx: oneshot::Receiver<()>,
    finish_tx: oneshot::Sender<()>,
    runtime: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Sandbox {
    async fn start() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("daemon-lifecycle-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        classick::config_file::save(
            &config_path,
            &PersistedConfig {
                source: Some(source),
                daemon: Some(DaemonSettings {
                    enabled: false,
                    schedule_minutes: 0,
                    ..Default::default()
                }),
                ipod_identity: Some(IpodIdentity {
                    serial: DEVICE_ID.into(),
                    model_label: "iPod Classic".into(),
                    name: None,
                    custom_selection: false,
                }),
                ..Default::default()
            },
        )
        .unwrap();

        let (device_tx, device_rx) = mpsc::channel(4);
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
        let (sync_started_tx, sync_started_rx) = oneshot::channel();
        let (cancel_seen_tx, cancel_seen_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let sync_started_tx = Mutex::new(Some(sync_started_tx));
        let cancel_seen_tx = Mutex::new(Some(cancel_seen_tx));
        let finish_rx = Mutex::new(Some(finish_rx));
        let spawn_sync: SpawnFn = Arc::new(move |_, _, cancel_rx, _, _, _| {
            let sync_started_tx = sync_started_tx.lock().unwrap().take().unwrap();
            let cancel_seen_tx = cancel_seen_tx.lock().unwrap().take().unwrap();
            let finish_rx = finish_rx.lock().unwrap().take().unwrap();
            Box::pin(async move {
                sync_started_tx.send(()).unwrap();
                cancel_rx.await.expect("shutdown sends cancellation");
                cancel_seen_tx.send(()).unwrap();
                finish_rx.await.expect("test releases finalization");
                Ok(OrchestratorOutcome::Cancelled {
                    summary: Some(SyncSummary::default()),
                })
            })
        });
        let spawn_scan: SpawnFn = Arc::new(move |_, _, _, _, _, _| {
            Box::pin(async move {
                Ok(OrchestratorOutcome::Completed {
                    summary: SyncSummary::default(),
                    db_restored: false,
                })
            })
        });
        let pipe_name = unique_pipe_name(&base, n);
        let deps = DaemonDeps {
            configured_serial: Some(DEVICE_ID.into()),
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
            source_availability: None,
            shutdown_rx,
        };
        let runtime = tokio::spawn(run_daemon_with_deps(deps));
        Self {
            base,
            pipe_name,
            device_tx,
            shutdown_tx,
            sync_started_rx,
            cancel_seen_rx,
            finish_tx,
            runtime,
        }
    }

    async fn begin_fake_sync(&mut self) -> TestClient {
        let mut client = TestClient::connect(&self.pipe_name).await;
        self.device_tx
            .send(DeviceEvent::Connected(DetectedIpod {
                serial: DEVICE_ID.into(),
                model_label: "iPod Classic".into(),
                drive: "/Volumes/FAKE".into(),
                name: None,
                volume_guid: None,
            }))
            .await
            .unwrap();
        client
            .next_snapshot_where(|snapshot| snapshot["devices"][0]["connected"] == true)
            .await;
        client
            .send(json!({
                "type": "trigger_sync",
                "trigger": "manual",
                "device_id": DEVICE_ID,
                "request_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8701"
            }))
            .await;
        (&mut self.sync_started_rx).await.expect("fake sync starts");
        client
    }

    async fn assert_drains_before_exit(mut self) {
        (&mut self.cancel_seen_rx)
            .await
            .expect("unified shutdown path sends cancellation");
        assert!(
            !self.runtime.is_finished(),
            "runtime must stay alive while fake finalization is pending"
        );
        self.finish_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut self.runtime)
            .await
            .expect("runtime exits after finalization")
            .expect("runtime task joins")
            .expect("runtime exits cleanly");
        let _ = std::fs::remove_dir_all(self.base);
    }
}

#[tokio::test]
async fn injected_signal_drains_the_active_sync_before_exit() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start().await;
    let _client = sandbox.begin_fake_sync().await;

    sandbox.shutdown_tx.send(ShutdownReason::Signal).unwrap();

    sandbox.assert_drains_before_exit().await;
}

#[tokio::test]
async fn injected_parent_death_drains_the_active_sync_before_exit() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start().await;
    let _client = sandbox.begin_fake_sync().await;

    sandbox
        .shutdown_tx
        .send(ShutdownReason::ParentDeath)
        .unwrap();

    sandbox.assert_drains_before_exit().await;
}

#[tokio::test]
async fn client_shutdown_still_drains_the_active_sync_before_exit() {
    let _guard = SERIAL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
    let mut sandbox = Sandbox::start().await;
    let mut client = sandbox.begin_fake_sync().await;

    client
        .send(json!({
            "type": "shutdown",
            "request_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8702"
        }))
        .await;

    sandbox.assert_drains_before_exit().await;
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

fn unique_pipe_name(base: &Path, _n: u32) -> String {
    #[cfg(windows)]
    return format!(r"\\.\pipe\classick-lifecycle-{}-{_n}", std::process::id());
    #[cfg(not(windows))]
    return base.join("daemon.sock").to_string_lossy().into_owned();
}
