//! Integration test for the daemon's library filesystem watcher (Task 3).
//! Unlike `daemon_runtime_integration.rs` (Windows-only, its `sandbox()`
//! hardcodes a `\\.\pipe\classick-test-...` path), this file is cross-platform:
//! it derives a per-test transport address the same way production does
//! (`ipc_server::default_pipe_name` — a named pipe on Windows, a `$TMPDIR`
//! Unix socket on macOS/Unix), so the macOS-first watcher wiring actually
//! runs on a macOS dev machine (this repo has no CI).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use classick::config_file::{DaemonSettings, PersistedConfig, SyncMode};
use classick::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use classick::daemon::history::SyncSummary;
use classick::daemon::lifecycle::ShutdownReason;
use classick::daemon::runtime::{run_daemon_with_deps, DaemonDeps};
use classick::daemon::sync_orchestrator::OrchestratorOutcome;
use tokio::sync::mpsc;

// The daemon binds a single transport address per instance; serialise the
// (currently single) test in this file so a future second test can't race on
// pipe/socket setup. Mirrors the pattern in `daemon_runtime_integration.rs`.
static PIPE_SERIAL: Mutex<()> = Mutex::new(());

/// A unique-per-test transport address. On Windows a named pipe; on Unix a
/// socket path under `$TMPDIR`. Follows the platform split in
/// `ipc_server::default_pipe_name` so the daemon server actually binds on
/// macOS in-test.
fn unique_pipe_name() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\classick-watch-test-{pid}-{n}")
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir()
            .join(format!("classick-watch-test-{pid}-{n}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// A `DeviceWatcher` that never emits — this test exercises only the
/// filesystem-watcher → scan path, no device activity. The channel's `Sender`
/// is owned by `WatcherSandbox` (not created-and-dropped inside `start()`), so
/// the channel stays open for the daemon's life without leaking: closing it
/// would make `device_rx.recv()` perpetually `Ready(None)` and, under the
/// select loop's `biased;`, starve the lower-priority watcher/timer arms.
struct NoDeviceWatcher(mpsc::Receiver<DeviceEvent>);
impl DeviceWatcher for NoDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        self.0
    }
}

/// Per-test sandbox with a configured `source` dir and a fake `spawn_scan`
/// that counts invocations, so the test can assert exactly how many scans a
/// source change triggers.
struct WatcherSandbox {
    source: PathBuf,
    scan_spawns: Arc<AtomicUsize>,
    /// Kept alive so the device channel doesn't close (see `NoDeviceWatcher`).
    _device_tx: mpsc::Sender<DeviceEvent>,
    _shutdown_tx: mpsc::UnboundedSender<ShutdownReason>,
    pipe_name: String,
    base: PathBuf,
    runtime_task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl WatcherSandbox {
    async fn shutdown(self) {
        self.runtime_task.abort();
        // Best-effort cleanup of the Unix socket + temp config dir.
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.pipe_name);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn noop_spawn(
    _serial: String,
    _drive: String,
    _cancel_rx: tokio::sync::oneshot::Receiver<()>,
    _pause_rx: tokio::sync::oneshot::Receiver<()>,
    _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>,
    _event_context: classick::daemon::session_admission::EventContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<OrchestratorOutcome>> + Send>>
{
    Box::pin(async move {
        Ok(OrchestratorOutcome::Completed {
            summary: SyncSummary::default(),
            db_restored: false,
        })
    })
}

async fn sandbox_with_source() -> WatcherSandbox {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("library-watch-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let config_path = base.join("config.toml");
    let history_path = base.join("history.json");
    let source = base.join("source");
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

    // Device channel: sender held by the sandbox so it stays open.
    let (device_tx, device_rx) = mpsc::channel::<DeviceEvent>(4);
    let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();

    let scan_spawns = Arc::new(AtomicUsize::new(0));
    let scan_spawns_for_closure = scan_spawns.clone();
    let spawn_scan =
        move |_serial: String,
              _drive: String,
              _cancel_rx: tokio::sync::oneshot::Receiver<()>,
              _pause_rx: tokio::sync::oneshot::Receiver<()>,
              _prompt_rx: tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>,
              _event_context: classick::daemon::session_admission::EventContext| {
            scan_spawns_for_closure.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(OrchestratorOutcome::Completed {
                    summary: SyncSummary::default(),
                    db_restored: false,
                })
            })
                as std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = anyhow::Result<OrchestratorOutcome>>
                            + Send,
                    >,
                >
        };

    let pipe_name = unique_pipe_name();
    let deps = DaemonDeps {
        configured_serial: None,
        watcher: Box::new(NoDeviceWatcher(device_rx)),
        spawn_sync: Arc::new(noop_spawn),
        spawn_backfill: Arc::new(noop_spawn),
        spawn_replace_library: Arc::new(noop_spawn),
        spawn_scan: Arc::new(spawn_scan),
        schedule_minutes: 0,
        preset_event_tx: None,
        config_path: Some(config_path),
        history_path: Some(history_path),
        pipe_name: Some(pipe_name.clone()),
        source_availability: None,
        shutdown_rx,
    };
    let runtime_task = tokio::spawn(run_daemon_with_deps(deps));

    // A `source` is configured, so run_daemon_with_deps fires a one-shot
    // startup scan. Wait for it to spawn + complete, then reset the counter so
    // the assertion below isolates scans triggered strictly by the
    // FS-change/debounce path.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while scan_spawns.load(Ordering::SeqCst) == 0 {
        if std::time::Instant::now() > deadline {
            panic!("startup scan never fired within 5s");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Let the (instantly-resolving) scan complete and flip state back to Idle,
    // so the later debounce-triggered scan isn't dropped by `state.is_idle()`.
    tokio::time::sleep(Duration::from_millis(100)).await;
    scan_spawns.store(0, Ordering::SeqCst);

    // Give the OS filesystem watch a beat to arm before the test writes under
    // `source` (mirrors library_watcher's own unit tests).
    tokio::time::sleep(Duration::from_millis(200)).await;

    WatcherSandbox {
        source,
        scan_spawns,
        _device_tx: device_tx,
        _shutdown_tx: shutdown_tx,
        pipe_name,
        base,
        runtime_task,
    }
}

/// A filesystem change under the configured source coalesces into exactly one
/// scan after `LIBRARY_DEBOUNCE_WINDOW`, proving the watcher is wired through
/// the debounce timer into `start_scan_session`.
#[tokio::test]
async fn watcher_change_triggers_one_scan_after_debounce() {
    let _guard = PIPE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let sb = sandbox_with_source().await;

    // Simulate a filesystem change under the configured source.
    std::fs::write(sb.source.join("added.flac"), b"x").unwrap();

    // Wait past the debounce window; exactly one scan should have spawned.
    tokio::time::sleep(classick::daemon::LIBRARY_DEBOUNCE_WINDOW + Duration::from_millis(500))
        .await;
    assert_eq!(
        sb.scan_spawns.load(Ordering::SeqCst),
        1,
        "one coalesced scan after a source change"
    );

    sb.shutdown().await;
}
