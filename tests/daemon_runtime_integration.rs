//! Integration smoke: spin up the daemon runtime with a scripted
//! device watcher and verify the auto-sync codepath fires when a
//! configured device appears.

#![cfg(windows)]

use std::sync::Mutex;
use std::time::Duration;

// The daemon binds a single global named-pipe per process, so these two
// integration tests must run sequentially. A static mutex serialises
// them without needing a new crate (e.g. serial_test). Each test
// acquires the lock for its full body so the pipe is freed when the
// previous test's runtime task winds down.
static PIPE_SERIAL: Mutex<()> = Mutex::new(());
// This test exists to PROVE the wiring works end-to-end. It uses a
// public test-only constructor on the runtime that takes injectable
// watcher + orchestrator-spawn-fn so we don't depend on a real
// ipod-sync.exe on disk.
// The actual entry point for production is `run_daemon()` in
// runtime.rs; this test calls `run_daemon_with_deps(deps)`.

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn auto_sync_fires_when_configured_device_connects() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    use ipod_sync::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use ipod_sync::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use ipod_sync::ipod::device::DetectedIpod;
    use tokio::sync::{mpsc, oneshot};

    // Scripted watcher: emits Connected for the configured serial.
    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> { self.0 }
    }
    let (tx, rx) = mpsc::channel::<DeviceEvent>(4);
    let watcher = ScriptedWatcher(rx);

    // Spawn-fn: records the drive it was called with; resolves the
    // oneshot the test awaits.
    let (spawn_seen_tx, spawn_seen_rx) = oneshot::channel::<String>();
    let spawn_seen_tx = std::sync::Mutex::new(Some(spawn_seen_tx));
    let spawn_fn = move |drive: String| {
        if let Some(s) = spawn_seen_tx.lock().unwrap().take() { let _ = s.send(drive.clone()); }
        Box::pin(async move {
            Ok(ipod_sync::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: ipod_sync::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let deps = DaemonDeps {
        configured_serial: Some("0xABC".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Box::new(spawn_fn),
        schedule_minutes: 0,
    };
    let _runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    // Simulate a plug-in event.
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(DeviceEvent::Connected(DetectedIpod {
        serial: "0xABC".to_string(),
        model_label: "iPod 7G".to_string(),
        drive: "G:\\".to_string(),
    })).await.unwrap();

    // The spawn-fn should have been called with the right drive.
    let drive = tokio::time::timeout(Duration::from_secs(5), spawn_seen_rx).await
        .expect("orchestrator should be spawned within 5s of plug-in")
        .expect("spawn-channel intact");
    assert_eq!(drive, "G:\\");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn unknown_device_does_not_trigger_auto_sync() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    use ipod_sync::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use ipod_sync::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use ipod_sync::ipod::device::DetectedIpod;
    use tokio::sync::mpsc;

    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> { self.0 }
    }
    let (tx, rx) = mpsc::channel::<DeviceEvent>(4);
    let watcher = ScriptedWatcher(rx);

    let spawn_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spawn_called_clone = spawn_called.clone();
    let spawn_fn = move |_drive: String| {
        spawn_called_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            Ok(ipod_sync::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: ipod_sync::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let deps = DaemonDeps {
        configured_serial: Some("0xCONFIGURED".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Box::new(spawn_fn),
        schedule_minutes: 0,
    };
    let _runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(DeviceEvent::Connected(DetectedIpod {
        serial: "0xWRONG".to_string(),
        model_label: "Other iPod".to_string(),
        drive: "H:\\".to_string(),
    })).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!spawn_called.load(std::sync::atomic::Ordering::Relaxed),
            "unknown serial must NOT trigger auto-sync");
}
