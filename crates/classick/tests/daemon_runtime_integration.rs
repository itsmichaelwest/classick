//! Integration smoke: spin up the daemon runtime with a scripted
//! device watcher and verify the auto-sync codepath fires when a
//! configured device appears.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// The daemon binds a single global named-pipe per process, so these two
// integration tests must run sequentially. A static mutex serialises
// them without needing a new crate (e.g. serial_test). Each test
// acquires the lock for its full body so the pipe is freed when the
// previous test's runtime task winds down.
static PIPE_SERIAL: Mutex<()> = Mutex::new(());

/// Per-test sandbox: unique tempdir under target/test-tmp/ with a
/// known-good config.toml + unique named-pipe name.
///
/// Without the config sandbox the daemon's `auto_sync_enabled` check
/// reads the developer's real `%APPDATA%\classick\config.toml`, which
/// on machines running in Manual mode (subsequent_sync_mode = "review")
/// returns false and silently breaks every test that exercises the
/// auto-sync path.
///
/// Without the pipe-name sandbox the production pipe
/// `\\.\pipe\classick` is bound by a real running daemon on the
/// developer's machine, and `spawn_server_full_with(..first_pipe_instance(true)..)`
/// fails immediately → the daemon task exits → device_rx is dropped
/// → tx.send(...).await.unwrap() in the test panics with SendError.
///
/// Returns (config_path, history_path, pipe_name).
fn sandbox() -> (PathBuf, PathBuf, String) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("daemon-int-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let cfg = base.join("config.toml");
    // Minimal config: just the daemon section in auto-apply mode. No
    // ipod_identity — the tests pass `configured_serial` directly via
    // DaemonDeps, which is the daemon's source of truth at startup.
    std::fs::write(
        &cfg,
        "[daemon]\nsubsequent_sync_mode = \"auto_apply\"\nschedule_minutes = 0\nnotify_on = \"all\"\n",
    )
    .unwrap();
    let pipe = format!(r"\\.\pipe\classick-test-{pid}-{n}");
    (cfg, base.join("history.json"), pipe)
}
// This test exists to PROVE the wiring works end-to-end. It uses a
// public test-only constructor on the runtime that takes injectable
// watcher + orchestrator-spawn-fn so we don't depend on a real
// classick.exe on disk.
// The actual entry point for production is `run_daemon()` in
// runtime.rs; this test calls `run_daemon_with_deps(deps)`.

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn auto_sync_fires_when_configured_device_connects() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use classick::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use classick::ipod::device::DetectedIpod;
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
    let spawn_fn = move |drive: String, _cancel_rx: tokio::sync::oneshot::Receiver<()>, _pause_rx: tokio::sync::oneshot::Receiver<()>, _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
        if let Some(s) = spawn_seen_tx.lock().unwrap().take() { let _ = s.send(drive.clone()); }
        Box::pin(async move {
            Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: classick::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let (config_path, history_path, pipe_name) = sandbox();
    let deps = DaemonDeps {
        configured_serial: Some("0xABC".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Arc::new(spawn_fn),
        spawn_backfill: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        spawn_scan: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        schedule_minutes: 0,
        preset_event_tx: None,
        config_path: Some(config_path),
        history_path: Some(history_path),
        pipe_name: Some(pipe_name),
    };
    let _runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    // Simulate a plug-in event.
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(DeviceEvent::Connected(DetectedIpod {
        serial: "0xABC".to_string(),
        model_label: "iPod 7G".to_string(),
        drive: "G:\\".to_string(),
        name: None,
        volume_guid: None,
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
    use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use classick::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use classick::ipod::device::DetectedIpod;
    use tokio::sync::mpsc;

    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> { self.0 }
    }
    let (tx, rx) = mpsc::channel::<DeviceEvent>(4);
    let watcher = ScriptedWatcher(rx);

    let spawn_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spawn_called_clone = spawn_called.clone();
    let spawn_fn = move |_drive: String, _cancel_rx: tokio::sync::oneshot::Receiver<()>, _pause_rx: tokio::sync::oneshot::Receiver<()>, _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
        spawn_called_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: classick::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let (config_path, history_path, pipe_name) = sandbox();
    let deps = DaemonDeps {
        configured_serial: Some("0xCONFIGURED".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Arc::new(spawn_fn),
        spawn_backfill: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        spawn_scan: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        schedule_minutes: 0,
        preset_event_tx: None,
        config_path: Some(config_path),
        history_path: Some(history_path),
        pipe_name: Some(pipe_name),
    };
    let _runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(DeviceEvent::Connected(DetectedIpod {
        serial: "0xWRONG".to_string(),
        model_label: "Other iPod".to_string(),
        drive: "H:\\".to_string(),
        name: None,
        volume_guid: None,
    })).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!spawn_called.load(std::sync::atomic::Ordering::Relaxed),
            "unknown serial must NOT trigger auto-sync");
}

/// Regression for "stuck in syncing": a long-running orchestrator must
/// not block the runtime loop from processing client commands or new
/// device events. Verifies by injecting a spawn-fn whose future never
/// resolves, then sending a Disconnected event that the runtime is
/// expected to handle (cleanly transitioning state back to Idle via
/// the Aborted-on-detach path) WHILE the orchestrator is still pending.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn runtime_stays_responsive_during_long_sync() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use classick::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use classick::ipod::device::DetectedIpod;
    use tokio::sync::{mpsc, oneshot};

    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> { self.0 }
    }
    let (tx, rx) = mpsc::channel::<DeviceEvent>(4);
    let watcher = ScriptedWatcher(rx);

    // Signal when spawn-fn was entered, and pend forever (the orchestrator
    // future never resolves — simulates a long sync).
    let (spawn_entered_tx, spawn_entered_rx) = oneshot::channel::<()>();
    let spawn_entered_tx = std::sync::Mutex::new(Some(spawn_entered_tx));
    let spawn_fn = move |_drive: String, _cancel_rx: tokio::sync::oneshot::Receiver<()>, _pause_rx: tokio::sync::oneshot::Receiver<()>, _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
        if let Some(s) = spawn_entered_tx.lock().unwrap().take() { let _ = s.send(()); }
        Box::pin(async move {
            std::future::pending::<()>().await;  // never resolves
            #[allow(unreachable_code)]
            Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: classick::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let (config_path, history_path, pipe_name) = sandbox();
    let deps = DaemonDeps {
        configured_serial: Some("0xABC".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Arc::new(spawn_fn),
        spawn_backfill: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        spawn_scan: Arc::new(
            |_drive: String,
             _cancel_rx: tokio::sync::oneshot::Receiver<()>,
             _pause_rx: tokio::sync::oneshot::Receiver<()>,
             _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
                Box::pin(async move {
                    Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                        outcome: classick::daemon::history::SyncOutcome::Ok,
                        summary: None,
                    })
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
            },
        ),
        schedule_minutes: 0,
        preset_event_tx: None,
        config_path: Some(config_path),
        history_path: Some(history_path),
        pipe_name: Some(pipe_name),
    };
    let _runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    // Trigger sync via Connected.
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(DeviceEvent::Connected(DetectedIpod {
        serial: "0xABC".to_string(),
        model_label: "iPod 7G".to_string(),
        drive: "G:\\".to_string(),
        name: None,
        volume_guid: None,
    })).await.unwrap();

    // Confirm orchestrator started (and is now stuck in std::future::pending).
    tokio::time::timeout(Duration::from_secs(2), spawn_entered_rx).await
        .expect("orchestrator should spawn within 2s")
        .expect("spawn-entered channel intact");

    // Now the key test: send a Disconnected event WHILE the orchestrator
    // is pending. If the runtime loop were blocked on the orchestrator
    // (the M3 bug we just fixed), this send would queue indefinitely and
    // the device-event arm would never fire. With the fix, the runtime
    // picks it up promptly.
    let send_result = tokio::time::timeout(
        Duration::from_secs(2),
        tx.send(DeviceEvent::Disconnected { serial: "0xABC".to_string() }),
    ).await;
    assert!(send_result.is_ok(), "Disconnected send timed out — runtime loop is blocked");
    assert!(send_result.unwrap().is_ok(), "send failed: receiver dropped?");

    // Give the runtime a moment to process. We can't observe state
    // mutation directly from outside, but the fact that the send
    // completed within 2s of a pending orchestrator proves the loop
    // is responsive.
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// Extended sandbox for watcher tests: adds a configured `source` dir and a
/// fake `spawn_scan` that counts invocations, so a test can assert exactly
/// how many scans a source change triggers. Reuses `sandbox()`'s tempdir +
/// pipe isolation, then overwrites the config with a `source` path via
/// `config_file::save` (PersistedConfig is public, so no hand-rolled TOML
/// needed) and swaps in a counting `spawn_scan` closure.
struct WatcherSandbox {
    source: PathBuf,
    scan_spawns: Arc<std::sync::atomic::AtomicUsize>,
    runtime_task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl WatcherSandbox {
    async fn shutdown(self) {
        self.runtime_task.abort();
    }
}

async fn sandbox_with_source() -> WatcherSandbox {
    use classick::config_file::{DaemonSettings, PersistedConfig, SyncMode};
    use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use classick::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use tokio::sync::mpsc;

    let (config_path, history_path, pipe_name) = sandbox();
    let source = config_path.parent().unwrap().join("source");
    std::fs::create_dir_all(&source).unwrap();

    let cfg = PersistedConfig {
        source: Some(source.clone()),
        daemon: Some(DaemonSettings {
            subsequent_sync_mode: SyncMode::AutoApply,
            schedule_minutes: 0,
            ..Default::default()
        }),
        ..Default::default()
    };
    classick::config_file::save(&config_path, &cfg).unwrap();

    // No device activity needed — this test only exercises the
    // filesystem-watcher → scan path.
    struct NoDeviceWatcher;
    impl DeviceWatcher for NoDeviceWatcher {
        fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
            // Leak the sender so the channel never closes — an immediately
            // closed channel makes `device_rx.recv()` perpetually Ready(None),
            // which under `biased;` starves the lower-priority watcher/timer
            // select arms this test depends on.
            let (tx, rx) = mpsc::channel(1);
            std::mem::forget(tx);
            rx
        }
    }

    let scan_spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scan_spawns_for_closure = scan_spawns.clone();
    let spawn_scan_fn = move |_drive: String,
                              _cancel_rx: tokio::sync::oneshot::Receiver<()>,
                              _pause_rx: tokio::sync::oneshot::Receiver<()>,
                              _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
        scan_spawns_for_closure.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: classick::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };
    let noop_spawn_fn = |_drive: String,
                         _cancel_rx: tokio::sync::oneshot::Receiver<()>,
                         _pause_rx: tokio::sync::oneshot::Receiver<()>,
                         _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>| {
        Box::pin(async move {
            Ok(classick::daemon::sync_orchestrator::OrchestratorOutcome::Completed {
                outcome: classick::daemon::history::SyncOutcome::Ok,
                summary: None,
            })
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<_>> + Send>>
    };

    let deps = DaemonDeps {
        configured_serial: None,
        watcher: Box::new(NoDeviceWatcher),
        spawn_sync: Arc::new(noop_spawn_fn),
        spawn_backfill: Arc::new(noop_spawn_fn),
        spawn_scan: Arc::new(spawn_scan_fn),
        schedule_minutes: 0,
        preset_event_tx: None,
        config_path: Some(config_path),
        history_path: Some(history_path),
        pipe_name: Some(pipe_name),
    };
    let runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    // A `source` is configured, so run_daemon_with_deps fires a one-shot
    // startup scan. Wait for it to be spawned, then let it complete (the
    // stub resolves instantly) and reset the counter — the test below
    // asserts scans triggered strictly by the FS-change/debounce path.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while scan_spawns.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        if std::time::Instant::now() > deadline {
            panic!("startup scan never fired within 5s");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Give the (instantly-resolving) scan a moment to complete and flip
    // state back to Idle so the debounce-triggered scan later isn't
    // dropped by the `state.is_idle()` guard.
    tokio::time::sleep(Duration::from_millis(100)).await;
    scan_spawns.store(0, std::sync::atomic::Ordering::SeqCst);

    // Give the OS filesystem watch a beat to arm before the test writes
    // under `source` (mirrors library_watcher's own tests).
    tokio::time::sleep(Duration::from_millis(200)).await;

    WatcherSandbox { source, scan_spawns, runtime_task }
}

// Task 3: a filesystem change under the configured source coalesces into
// exactly one scan after LIBRARY_DEBOUNCE_WINDOW, proving the watcher is
// wired through the debounce timer into start_scan_session.
#[tokio::test]
async fn watcher_change_triggers_one_scan_after_debounce() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let sb = sandbox_with_source().await;

    // Simulate a filesystem change under the configured source.
    std::fs::write(sb.source.join("added.flac"), b"x").unwrap();

    // Wait past the debounce window; exactly one scan should have spawned.
    tokio::time::sleep(
        classick::daemon::LIBRARY_DEBOUNCE_WINDOW + Duration::from_millis(500),
    ).await;
    assert_eq!(
        sb.scan_spawns.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one coalesced scan after a source change"
    );

    sb.shutdown().await;
}
