//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, device watcher, scheduler, and sync orchestrator.
//!
//! M3 scope: real auto-sync on configured-iPod plug-in, Sync Now via
//! manual TriggerSync, periodic Scheduled triggers from the scheduler,
//! and live DeviceConnected/Disconnected broadcasts. Test-only entry
//! `run_daemon_with_deps` exists so the integration suite can inject
//! a scripted device watcher and a fake spawn-fn.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::device_storage::{self, StorageInfo};
use crate::daemon::device_watcher::{Debouncer, DeviceEvent, DeviceWatcher};
#[cfg(not(target_os = "macos"))]
use crate::daemon::device_watcher::PollingDeviceWatcher;
use crate::daemon::history::{HistoryEntry, HistoryService, SyncOutcome, SyncSummary, SyncTrigger};
use crate::daemon::ipc_server::ClientCommand;
use crate::daemon::scheduler::SyncScheduler;
use crate::daemon::state::{DaemonState, SessionKind, StateMachine, TriggerOutcome};
use crate::daemon::sync_orchestrator::{self, OrchestratorOutcome};
use crate::ipc_daemon::{
    DaemonCommand, DaemonEvent, DaemonStateLabel, PlaylistKind, SyncRejectReason, TriggerSource,
};
use crate::ipod::device::DetectedIpod;
use anyhow::Result;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Production entry. Constructs the real device watcher + real
/// spawn-fn and runs the daemon.
pub async fn run_daemon() -> Result<()> {
    tracing::info!("daemon: starting");
    let config_path = config_file::default_path()?;
    let configured_serial = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.ipod_identity)
        .map(|i| i.serial);
    let schedule_minutes = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.daemon)
        .map(|d| d.schedule_minutes)
        .unwrap_or(crate::daemon::DEFAULT_SCHEDULE_MINUTES);

    // Build the broadcast event_tx FIRST so the spawn_sync closure can
    // capture a clone — that way orchestrator events flow through the
    // same channel UI clients are subscribed to.
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(crate::daemon::BROADCAST_CHANNEL_CAPACITY);
    let exe = std::env::current_exe()?;
    let event_tx_for_spawn = event_tx.clone();
    // The spawn closure re-reads the persisted (global) config AND the
    // syncing device's own settings at spawn time (same re-read-at-spawn
    // pattern as the GetConfig/SaveConfig command arms) rather than
    // capturing a snapshot at daemon startup — so a Settings change to
    // `rockbox_compat` takes effect on the very next sync without a daemon
    // restart. The device settings win over the global value once seeded;
    // see `device_config::DeviceSettings::load_or_migrate`.
    let config_path_for_spawn = config_path.clone();
    let spawn_sync: SpawnFn = Arc::new(move |serial: String, drive: String, cancel_rx, pause_rx, prompt_rx| {
        let exe = exe.clone();
        let event_tx = event_tx_for_spawn.clone();
        let global = config_file::load(&config_path_for_spawn).ok().flatten().unwrap_or_default();
        let rockbox_compat =
            crate::device_config::DeviceSettings::load_or_migrate(&serial, &global).rockbox_compat;
        Box::pin(async move {
            sync_orchestrator::run(exe, drive, rockbox_compat, cancel_rx, pause_rx, prompt_rx, event_tx).await
        })
    });

    let exe_for_backfill = std::env::current_exe()?;
    let event_tx_for_backfill = event_tx.clone();
    let spawn_backfill: SpawnFn =
        Arc::new(move |_serial: String, drive: String, cancel_rx, pause_rx, prompt_rx| {
            let exe = exe_for_backfill.clone();
            let event_tx = event_tx_for_backfill.clone();
            Box::pin(async move {
                sync_orchestrator::run_backfill(exe, drive, cancel_rx, pause_rx, prompt_rx, event_tx).await
            })
        });

    let exe_for_replace = std::env::current_exe()?;
    let event_tx_for_replace = event_tx.clone();
    let spawn_replace_library: SpawnFn =
        Arc::new(move |_serial: String, drive: String, cancel_rx, pause_rx, prompt_rx| {
            let exe = exe_for_replace.clone();
            let event_tx = event_tx_for_replace.clone();
            Box::pin(async move {
                sync_orchestrator::run_replace_library(
                    exe, drive, cancel_rx, pause_rx, prompt_rx, event_tx,
                )
                .await
            })
        });

    let exe_for_scan = std::env::current_exe()?;
    let event_tx_for_scan = event_tx.clone();
    let spawn_scan: SpawnFn =
        Arc::new(move |_serial: String, _drive: String, cancel_rx, pause_rx, prompt_rx| {
            let exe = exe_for_scan.clone();
            let event_tx = event_tx_for_scan.clone();
            Box::pin(async move {
                sync_orchestrator::run_scan(exe, cancel_rx, pause_rx, prompt_rx, event_tx).await
            })
        });

    let deps = DaemonDeps {
        configured_serial,
        #[cfg(target_os = "macos")]
        watcher: Box::new(crate::daemon::iokit_watcher::IokitDeviceWatcher::new_production()),
        #[cfg(not(target_os = "macos"))]
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        spawn_backfill,
        spawn_replace_library,
        spawn_scan,
        schedule_minutes,
        preset_event_tx: Some(event_tx),
        config_path: None,
        history_path: None,
        pipe_name: None,
    };
    run_daemon_with_deps(deps).await
}

/// Async closure that runs one sync to completion. Arc-wrapped so the
/// runtime can clone it into a tokio::spawn'd task without consuming the
/// daemon's only copy.
///
/// Args: `(serial, drive, cancel_rx, pause_rx, prompt_decisions_rx)`. `serial`
/// lets `spawn_sync` resolve per-device settings (Rockbox compat) at spawn
/// time; closures that don't need it (backfill/replace/scan) ignore it. The
/// prompt channel lets `DaemonCommand::DecidePrompt` ferry user replies
/// through to the running subprocess's stdin without blocking the runtime
/// loop. The pause channel lets `DaemonCommand::Pause` request a graceful
/// stop.
pub type SpawnFn = Arc<
    dyn Fn(
            String,
            String,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
            tokio::sync::mpsc::UnboundedReceiver<(u64, i32)>,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
        + Send
        + Sync,
>;

pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--backfill-rockbox`
    /// subprocess (`sync_orchestrator::run_backfill` in production) instead
    /// of `--apply`. Used by the `DaemonCommand::BackfillRockbox` arm,
    /// which reuses `start_sync_session` — the same state-machine guard,
    /// cancel/pause/prompt channels, and event relay as a normal sync — so
    /// a backfill and a sync can never run concurrently.
    pub spawn_backfill: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--replace-library --apply`
    /// subprocess (`sync_orchestrator::run_replace_library` in production)
    /// instead of plain `--apply`. Used by the `DaemonCommand::ReplaceLibrary`
    /// arm, which reuses `start_sync_session` — the same state-machine
    /// guard, cancel/pause/prompt channels, and event relay as a normal
    /// sync — so a replace and a sync (or backfill) can never run
    /// concurrently.
    pub spawn_replace_library: SpawnFn,
    /// Same shape as `spawn_sync`, but drives a `--scan-library` subprocess
    /// (`sync_orchestrator::run_scan` in production). The `drive` argument is
    /// ignored — a scan touches no device. Used by `DaemonCommand::ScanLibrary`
    /// via `start_scan_session`, which shares the sync guard.
    pub spawn_scan: SpawnFn,
    pub schedule_minutes: u32,
    /// If Some, the runtime uses this pre-built sender instead of
    /// constructing its own. Production passes the same one it gave
    /// to the spawn_sync closure so orchestrator events broadcast on
    /// the same channel UI clients subscribe to.
    pub preset_event_tx: Option<broadcast::Sender<DaemonEvent>>,
    /// Override the persisted-config path. Production passes `None` and
    /// `config_file::default_path()` resolves to
    /// `%APPDATA%\classick\config.toml`. Tests pass a tempdir-managed
    /// path with a known-good config so the suite is deterministic
    /// regardless of the developer's local settings (notably
    /// `subsequent_sync_mode`, which gates auto-sync).
    pub config_path: Option<PathBuf>,
    /// Override the history file path. Production: `None` → default
    /// (`%LOCALAPPDATA%\classick\history.json`). Tests: provide a
    /// temp path so history append/read doesn't pollute the
    /// developer's real history.json.
    pub history_path: Option<PathBuf>,
    /// Override the named-pipe name. Production: `None` → uses the
    /// `\\.\pipe\classick` constant which is the IPC contract with
    /// the UI. Tests pass a unique per-test pipe name like
    /// `\\.\pipe\classick-test-<pid>-<n>` so the suite runs even
    /// while a real daemon is bound to the production pipe.
    pub pipe_name: Option<String>,
}

/// Internal events posted from background sync tasks back to the runtime
/// loop. The runtime owns state + history; the spawned task only does
/// the actual sync work and ships its outcome here for state-machine
/// mutation + history persistence + broadcast.
enum InternalEvent {
    SyncCompleted {
        trigger: SyncTrigger,
        serial: String,
        started_at_unix_secs: u64,
        outcome: Result<OrchestratorOutcome>,
    },
    /// Sent by the iPod-name reader task after itdb_parse completes on
    /// a freshly-plugged device. The runtime applies the name to
    /// `connected` (if the serial still matches the current device),
    /// persists it into config so the UI sees it across daemon
    /// restarts, then re-broadcasts a DeviceConnected + ConfigUpdate
    /// so the popover updates immediately.
    IpodNameResolved {
        serial: String,
        name: Option<String>,
    },
    /// Result of an off-thread source-library walk (see `spawn_library_count`).
    /// Populates the cached `library_count` (Y in "X of Y synced") so the menu
    /// can show a total on a cold start, before any sync has run.
    LibraryCountComputed {
        count: usize,
    },
    /// A --scan-library subprocess finished. No history entry — a scan is
    /// cache maintenance, not a sync.
    ScanCompleted {
        outcome: Result<OrchestratorOutcome>,
    },
}

/// Test-friendly entry. Production wraps real impls and calls this.
pub async fn run_daemon_with_deps(deps: DaemonDeps) -> Result<()> {
    let history_path = match deps.history_path {
        Some(p) => p,
        None => history_file_path()?,
    };
    let history = HistoryService::new(history_path);
    let config_path = match deps.config_path {
        Some(p) => p,
        None => config_file::default_path()?,
    };
    let mut state = StateMachine::new();
    let mut scheduler = SyncScheduler::new(deps.schedule_minutes);
    let mut debouncer = Debouncer::new(crate::daemon::DEVICE_DEBOUNCE_WINDOW);
    let mut connected: Option<DetectedIpod> = None;
    let configured_serial = deps.configured_serial;

    let pipe_name = deps
        .pipe_name
        .clone()
        .unwrap_or_else(crate::daemon::ipc_server::default_pipe_name);
    let (event_tx, mut cmd_rx, mut new_client_rx) = match deps.preset_event_tx {
        Some(tx) => crate::daemon::ipc_server::spawn_server_full_with(tx, &pipe_name).await?,
        None => {
            let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
            let (tx, rx, _new_client_rx) =
                crate::daemon::ipc_server::spawn_server_full_with(event_tx, &pipe_name).await?;
            // Test path: synthesize an empty new-client channel that
            // never fires. The integration tests don't exercise snapshot
            // semantics; production goes through spawn_server_full.
            let (_dummy_tx, dummy_rx) = mpsc::unbounded_channel::<()>();
            (tx, rx, dummy_rx)
        }
    };
    let mut device_rx = deps.watcher.start();
    // Filesystem watcher over the configured source library. Emits coalesced
    // change ticks; the select loop debounces them and triggers a scan.
    let initial_source = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source);
    let (mut library_watcher, mut library_rx) =
        crate::daemon::library_watcher::LibraryWatcher::spawn(initial_source);
    // Deadline used to debounce a burst of FS events into a single scan.
    let mut library_scan_deadline: Option<tokio::time::Instant> = None;
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();
    let spawn_sync = deps.spawn_sync;
    let spawn_backfill = deps.spawn_backfill;
    let spawn_replace_library = deps.spawn_replace_library;
    let spawn_scan = deps.spawn_scan;
    // Cancellation signal for the currently-running sync (if any). Set
    // by start_sync_session; taken + sent by handle_client_command's
    // CancelSync arm; cleared in handle_internal_event after completion.
    let mut cancel_tx_holder: Option<oneshot::Sender<()>> = None;
    // Prompt-decision relay: each DecidePrompt command sends its
    // (id, choice) into this mpsc; the orchestrator's select loop
    // reads it and writes the corresponding {"type":"prompt_decision",
    // ...}\n line to the subprocess stdin. Set alongside cancel_tx
    // at sync-start; cleared on completion.
    let mut prompt_tx_holder: Option<mpsc::UnboundedSender<(u64, i32)>> = None;
    // Pause signal for the currently-running sync (if any). Mirrors
    // cancel_tx_holder, but the orchestrator's pause arm doesn't
    // force-kill — see sync_orchestrator::run's doc comment.
    let mut pause_tx_holder: Option<oneshot::Sender<()>> = None;
    // Cached source-library track count (Y in "X of Y synced"). Walking the
    // source on every status tick would stall the daemon loop, so it's cached:
    // filled by an off-thread walk at startup + after SaveConfig (see
    // `spawn_library_count`), and also refreshed for free from each sync's
    // already-performed diff (add + modify + unchanged + metadata_only ==
    // current source count). `None` only until the first walk/sync lands.
    let mut library_count_cache: Option<usize> = None;
    let mut configured_serial = configured_serial;

    tracing::info!("daemon: ready (configured_serial={configured_serial:?})");

    // Proactively count the source library so "X of Y synced" shows a total on
    // a cold start, before any sync has run. Fills `library_count_cache`
    // asynchronously via InternalEvent::LibraryCountComputed.
    spawn_library_count(&config_path, &internal_tx);

    // Refresh the library index once at startup so the browser is current
    // without a user action. Guarded/incremental like any scan.
    if config_file::load(&config_path).ok().flatten().and_then(|c| c.source).is_some() {
        start_scan_session(
            &mut state, &event_tx, &spawn_scan, &internal_tx,
            &mut cancel_tx_holder, &mut prompt_tx_holder, &mut pause_tx_holder,
            &connected, &config_path, &history, library_count_cache,
        );
    }

    let exit_reason: ExitReason = loop {
        tokio::select! {
            biased;

            client_cmd = cmd_rx.recv() => {
                let Some(client_cmd) = client_cmd else {
                    tracing::info!("daemon: command channel closed; exiting");
                    return Ok(());
                };
                // Only SaveConfig can change the source path; avoid a disk
                // read + rewatch on every other command.
                let is_save_config =
                    matches!(&client_cmd.command, DaemonCommand::SaveConfig { .. });
                let should_exit = handle_client_command(
                    client_cmd,
                    &history,
                    &config_path,
                    &mut state,
                    &event_tx,
                    &connected,
                    &spawn_sync,
                    &spawn_backfill,
                    &spawn_replace_library,
                    &spawn_scan,
                    &internal_tx,
                    &mut cancel_tx_holder,
                    &mut prompt_tx_holder,
                    &mut pause_tx_holder,
                    &mut configured_serial,
                    &mut scheduler,
                    &mut library_count_cache,
                );
                // A SaveConfig may have changed the source path; re-point the
                // watcher. rewatch() is a no-op when the path is unchanged.
                // A transient config-read failure must NOT disarm the
                // watcher — only a legitimately-absent source should.
                if is_save_config {
                    match config_file::load(&config_path) {
                        Ok(cfg) => library_watcher.rewatch(cfg.and_then(|c| c.source)),
                        Err(e) => tracing::warn!(
                            "daemon: skipping library rewatch, config load failed: {e:#}"
                        ),
                    }
                }
                if should_exit { break ExitReason::Shutdown; }
            }

            device_event = device_rx.recv() => {
                let Some(raw) = device_event else {
                    tracing::warn!("daemon: device watcher channel closed");
                    continue;
                };
                let Some(event) = debouncer.admit(raw) else { continue };
                handle_device_event(
                    event,
                    &mut connected,
                    &event_tx,
                    &mut state,
                    &history,
                    &spawn_sync,
                    &internal_tx,
                    &mut cancel_tx_holder,
                    &mut prompt_tx_holder,
                    &mut pause_tx_holder,
                    configured_serial.as_deref(),
                    &config_path,
                    library_count_cache,
                );
                broadcast_status(&event_tx, &state, &connected, &config_path, &history, library_count_cache);
            }

            Some(internal) = internal_rx.recv() => {
                // Sync AND scan completions clear the sync-lifecycle holders
                // (both occupy the shared guard). IpodNameResolved /
                // LibraryCountComputed are unrelated to sync lifecycle.
                let is_sync_completion = matches!(internal,
                    InternalEvent::SyncCompleted { .. } | InternalEvent::ScanCompleted { .. });
                if is_sync_completion {
                    cancel_tx_holder = None;
                    prompt_tx_holder = None;
                    pause_tx_holder = None;
                }
                handle_internal_event(internal, &mut state, &event_tx, &history, &mut connected, &config_path, &mut library_count_cache);
            }

            Some(()) = new_client_rx.recv() => {
                // A fresh UI connected. Publish a snapshot StatusUpdate
                // so the new subscriber's tray + popover initialize
                // with current state, even if earlier broadcasts (e.g.
                // DeviceConnected from polling at daemon startup) went
                // out before they subscribed.
                broadcast_status(&event_tx, &state, &connected, &config_path, &history, library_count_cache);
            }

            _ = scheduler.tick() => {
                // Scheduled syncs also honour the user's auto/manual choice;
                // schedule_minutes is moot when the user opted into manual.
                // The gate reads the CONFIGURED device's settings (not
                // necessarily `connected`, though in practice a scheduled
                // tick only does anything when they match — see the
                // plug-in gate below for the equivalent check on attach).
                let scheduled_gate = configured_serial
                    .as_deref()
                    .is_some_and(|serial| auto_sync_enabled(&config_path, serial));
                if connected.is_some() && state.is_idle() && scheduled_gate {
                    if let Some(drive) = connected.as_ref().map(|d| d.drive.clone()) {
                        start_sync_session(
                            SyncTrigger::Scheduled,
                            connected.as_ref().unwrap().serial.clone(),
                            drive,
                            &mut state,
                            &event_tx,
                            &spawn_sync,
                            &internal_tx,
                            &mut cancel_tx_holder,
                            &mut prompt_tx_holder,
                            &mut pause_tx_holder,
                            library_count_cache,
                        );
                    }
                }
            }

            Some(()) = library_rx.recv() => {
                // Coalesce: (re)arm the debounce deadline. The timer arm below
                // fires the scan once the source has been quiet for the window.
                library_scan_deadline =
                    Some(tokio::time::Instant::now() + crate::daemon::LIBRARY_DEBOUNCE_WINDOW);
            }

            _ = async {
                match library_scan_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Only scan when idle + a source is configured. If busy (a
                // sync/scan is in flight), re-arm the deadline instead of
                // dropping it, so the pending rescan is retried once the
                // current operation finishes rather than being lost forever.
                let has_source = config_file::load(&config_path).ok().flatten()
                    .and_then(|c| c.source).is_some();
                if !has_source {
                    // Nothing to scan — clear the deadline.
                    library_scan_deadline = None;
                } else if state.is_idle() {
                    // Consume the deadline and run the incremental scan.
                    library_scan_deadline = None;
                    tracing::info!("daemon: library watcher fired a scan after debounce");
                    start_scan_session(
                        &mut state, &event_tx, &spawn_scan, &internal_tx,
                        &mut cancel_tx_holder, &mut prompt_tx_holder, &mut pause_tx_holder,
                        &connected, &config_path, &history, library_count_cache,
                    );
                } else {
                    // Busy (sync/scan in flight): retry after another debounce
                    // window rather than dropping the pending rescan.
                    library_scan_deadline =
                        Some(tokio::time::Instant::now() + crate::daemon::LIBRARY_DEBOUNCE_WINDOW);
                }
            }
        }
    };

    // Graceful shutdown: if a sync is in flight, give the orchestrator a
    // bounded window to drain (it writes Cancel to subprocess stdin and
    // force-kills after SYNC_KILL_GRACE). The kill_on_drop flag on the
    // child Command is the backstop — when this function returns and the
    // tokio runtime tears down, the orchestrator task is dropped and any
    // still-living subprocess gets TerminateProcess'd. Without this drain,
    // the OS would yank the subprocess mid-itdb_write and risk corrupting
    // the iPod's iTunesDB.
    match exit_reason {
        ExitReason::Shutdown => {
            if cancel_tx_holder.is_none() && state.is_idle() {
                tracing::info!("daemon: clean shutdown — no in-flight sync to drain");
            } else {
                if let Some(tx) = cancel_tx_holder.take() {
                    let _ = tx.send(());
                    tracing::info!("daemon: signalled in-flight sync to cancel before exit");
                }
                let drain = tokio::time::timeout(
                    crate::daemon::SHUTDOWN_DRAIN_BUDGET,
                    async {
                        while let Some(internal) = internal_rx.recv().await {
                            if matches!(internal, InternalEvent::SyncCompleted { .. }) {
                                return;
                            }
                        }
                    },
                ).await;
                match drain {
                    Ok(()) => tracing::info!("daemon: in-flight sync drained cleanly"),
                    Err(_) => tracing::warn!(
                        "daemon: shutdown drain timed out after {:?}; subprocess will be killed by kill_on_drop",
                        crate::daemon::SHUTDOWN_DRAIN_BUDGET,
                    ),
                }
            }
        }
    }
    Ok(())
}

/// Reason we exited the main select loop. Currently only Shutdown; the
/// enum exists so a future channel-closed / panic-recovery branch can
/// take a different drain path.
enum ExitReason {
    Shutdown,
}

/// Apply a sync's outcome to state + history, then broadcast a fresh
/// StatusUpdate so UIs flip back to Idle. Called from the runtime
/// loop when a SyncCompleted internal event arrives — i.e., AFTER
/// the spawned orchestrator task has finished.
fn handle_internal_event(
    event: InternalEvent,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    history: &HistoryService,
    connected: &mut Option<DetectedIpod>,
    config_path: &std::path::Path,
    library_count_cache: &mut Option<usize>,
) {
    match event {
        InternalEvent::LibraryCountComputed { count } => {
            *library_count_cache = Some(count);
            broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
        }
        InternalEvent::IpodNameResolved { serial, name } => {
            // Apply only if the resolved-name's serial still matches
            // the currently-connected device. (Device could've been
            // detached during the iTunesDB parse.)
            let Some(c) = connected.as_mut() else { return };
            if c.serial != serial { return }
            if c.name == name { return }
            c.name = name.clone();

            // Persist the name into the user's IpodIdentity so it
            // survives daemon restarts (read_ipod_name is a 100-500ms
            // op we don't want to repeat unnecessarily).
            if let Ok(Some(mut cfg)) = config_file::load(config_path) {
                if let Some(id) = cfg.ipod_identity.as_mut() {
                    if id.serial == serial && id.name != name {
                        id.name = name.clone();
                        if let Err(e) = config_file::save(config_path, &cfg) {
                            tracing::warn!("daemon: failed to persist iPod name to config: {e}");
                        }
                    }
                }
            }

            // Re-broadcast DeviceConnected with the now-populated
            // name, and a ConfigUpdate so the popover/title bar
            // refreshes from either path.
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: c.serial.clone(),
                model_label: c.model_label.clone(),
                drive: c.drive.clone(),
                name: c.name.clone(),
            });
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = event_tx.send(build_config_update(cfg));
            return;
        }
        InternalEvent::SyncCompleted { trigger, serial, started_at_unix_secs, outcome } => {
            // If the device was detached mid-sync, the Disconnected handler
            // already wrote an Aborted history entry and called finish_sync.
            // In that case state is Idle and serial != this sync's serial —
            // silently drop this completion to avoid a duplicate history entry.
            if state.is_idle() {
                tracing::debug!("daemon: sync completion arrived but state is already Idle (likely device-detached mid-sync); ignoring");
                return;
            }

            let (history_outcome, error_message, summary, db_restored) = match outcome {
                Ok(OrchestratorOutcome::Completed { outcome: SyncOutcome::Ok, summary, db_restored }) => {
                    (SyncOutcome::Ok, None, summary, db_restored)
                }
                Ok(OrchestratorOutcome::Completed { outcome, summary, db_restored }) => {
                    (outcome, Some("sync subprocess reported failure".to_string()), summary, db_restored)
                }
                Ok(OrchestratorOutcome::Aborted { reason, summary }) => {
                    (SyncOutcome::Aborted, Some(reason), summary, false)
                }
                // A graceful pause isn't a failure or a user-driven abort of
                // the *library* — it's recorded as Aborted (reason "paused")
                // so history still reflects "didn't fully complete", while
                // the live "paused" signal itself rode the raw SyncEvent
                // stream the UI's Phase.paused reducer watches directly.
                Ok(OrchestratorOutcome::Paused { summary }) => {
                    (SyncOutcome::Aborted, Some("paused".to_string()), summary, false)
                }
                Err(e) => {
                    (SyncOutcome::Error, Some(format!("orchestrator: {e:#}")), None, false)
                }
            };

            // Refresh the library-count cache from this sync's diff — free,
            // since the apply loop already walked the source to compute it.
            // add + modify + unchanged + metadata_only are the tracks
            // currently present in the source (remove is present in the
            // manifest but gone from source, so it's excluded from "current
            // library size"). metadata_only tracks are already on the iPod
            // (only their tags/art were rewritten) — omitting them here
            // undercounts the total, so the UI's "X of Y synced" could show
            // X > Y after a tag-only sync.
            if let Some(s) = summary.as_ref() {
                *library_count_cache = Some(s.add + s.modify + s.unchanged + s.metadata_only);
            }

            let entry = make_history_entry(
                trigger, history_outcome, error_message, summary, started_at_unix_secs, db_restored,
            );
            let last_sync = Some(entry.clone());
            let _ = history.append(entry);
            state.finish_sync();

            let _ = serial;  // recorded in history via trigger context above
            let _ = event_tx.send(DaemonEvent::StatusUpdate {
                state: DaemonStateLabel::Idle,
                configured: true,
                ipod_connected: connected.is_some(),
                last_sync,
                next_scheduled_unix_secs: None,
                storage: current_storage(connected),
                synced_count: synced_track_count(),
                library_count: *library_count_cache,
            });
        }
        InternalEvent::ScanCompleted { outcome } => {
            if let Err(e) = &outcome {
                tracing::warn!("daemon: library scan failed: {e:#}");
            }
            state.finish_sync();
            // Fresh index on disk: rebroadcast the library and a status
            // update (selection-aware count may have changed).
            let _ = event_tx.send(crate::daemon::library::build_library_update(config_path));
            broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
        }
    }
}

/// Whether the daemon should auto-trigger syncs (plug-in + scheduled) for
/// `serial`. Gated on that device's own `auto_sync` toggle ("Sync
/// automatically on plug-in") — per-device since the trust-package settings
/// migration, no longer the global daemon setting. When it's off — or
/// neither the global config nor the device settings can be read — plug-in
/// and scheduled triggers no-op and the only way to start a sync is the
/// tray's Sync Now action. `subsequent_sync_mode` is NOT the gate: it
/// selects apply-vs-review *once a sync runs* (review is v1.1; today every
/// sync applies).
///
/// TODO(windows): the WinUI app still encodes auto-sync on/off in
/// `subsequent_sync_mode` (`WizardViewModel`: `IsAutomatic ? "auto_apply" :
/// "review"`) and always sends `enabled: true`. Until it maps the on/off
/// intent to `enabled` instead, a Windows user who picks Manual mode will
/// still be auto-synced. macOS already writes `enabled` correctly. This
/// still holds under per-device settings: `enabled` is only the seed value
/// the first time a device is seen (see `DeviceSettings::load_or_migrate`).
fn auto_sync_enabled(config_path: &std::path::Path, serial: &str) -> bool {
    let global = config_file::load(config_path).ok().flatten().unwrap_or_default();
    let device = crate::device_config::DeviceSettings::load_or_migrate(serial, &global);
    should_auto_sync(&device)
}

/// Pure decision core of [`auto_sync_enabled`], split out so it can be
/// tested without touching the filesystem.
pub(crate) fn should_auto_sync(settings: &crate::device_config::DeviceSettings) -> bool {
    settings.auto_sync
}

fn handle_device_event(
    event: DeviceEvent,
    connected: &mut Option<DetectedIpod>,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut StateMachine,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    prompt_tx_holder: &mut Option<mpsc::UnboundedSender<(u64, i32)>>,
    pause_tx_holder: &mut Option<oneshot::Sender<()>>,
    configured_serial: Option<&str>,
    config_path: &std::path::Path,
    library_count_cache: Option<usize>,
) {
    match event {
        DeviceEvent::Connected(mut ipod) => {
            // Seed name from persisted config so the UI doesn't flash
            // "iPod (Classic 7G)" then snap to "Michael's iPod" — if
            // we read the name on a previous plug-in we already have it.
            if ipod.name.is_none() {
                if let Ok(Some(cfg)) = config_file::load(config_path) {
                    if let Some(id) = cfg.ipod_identity.as_ref() {
                        if id.serial == ipod.serial { ipod.name = id.name.clone(); }
                    }
                }
            }

            *connected = Some(ipod.clone());
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: ipod.serial.clone(),
                model_label: ipod.model_label.clone(),
                drive: ipod.drive.clone(),
                name: ipod.name.clone(),
            });
            // Pair every attach with a ConfigUpdate. Otherwise a cached-name
            // plug-in (where IpodNameResolved dedups on unchanged name and
            // never re-broadcasts) leaves a subscriber that connected before
            // the attach without the persisted iPod identity — so it can't
            // match the connected serial and stays stuck on "Set Up".
            let _ = event_tx.send(build_config_update(config_file::load(config_path).ok().flatten()));

            // Off-thread iTunesDB read so the daemon loop stays
            // responsive. Result arrives via IpodNameResolved.
            let drive_for_read = ipod.drive.clone();
            let serial_for_read = ipod.serial.clone();
            let tx_for_read = internal_tx.clone();
            tokio::task::spawn_blocking(move || {
                let drive_path = std::path::PathBuf::from(&drive_for_read);
                let started = std::time::Instant::now();
                let name = crate::ipod::db::read_ipod_name(&drive_path);
                tracing::info!(
                    "daemon: read iPod name for {serial_for_read} in {}ms → {:?}",
                    started.elapsed().as_millis(),
                    name,
                );
                let _ = tx_for_read.send(InternalEvent::IpodNameResolved {
                    serial: serial_for_read,
                    name,
                });
            });

            // Auto-sync only fires for the configured serial AND when the
            // user has opted into automatic mode. Manual mode means they
            // want to drive sync explicitly via the tray's Sync Now action.
            if configured_serial == Some(ipod.serial.as_str())
                && state.is_idle()
                && auto_sync_enabled(config_path, &ipod.serial)
            {
                start_sync_session(
                    SyncTrigger::PlugIn,
                    ipod.serial.clone(),
                    ipod.drive.clone(),
                    state,
                    event_tx,
                    spawn_sync,
                    internal_tx,
                    cancel_tx_holder,
                    prompt_tx_holder,
                    pause_tx_holder,
                    library_count_cache,
                );
            }
        }
        DeviceEvent::Disconnected { serial } => {
            *connected = None;
            let _ = event_tx.send(DaemonEvent::DeviceDisconnected { serial: serial.clone() });
            // If the device we were syncing disappeared, force-finish
            // the session with Aborted. The spawned orchestrator task
            // is still running — its SyncCompleted will arrive later and
            // be silently dropped (handle_internal_event checks for the
            // already-Idle case).
            if let DaemonState::Syncing(s) = state.state() {
                if s.serial.as_deref() == Some(&serial) {
                    let _ = history.append(make_history_entry(
                        s.trigger.clone(),
                        SyncOutcome::Aborted,
                        Some("device_detached".to_string()),
                        None,
                        s.started_at_unix_secs,
                        false,
                    ));
                    state.finish_sync();
                }
            }
        }
    }
}

/// Kick off a sync as a background task. Updates state to Syncing and
/// emits the Syncing StatusUpdate broadcast immediately, then returns
/// so the runtime loop stays responsive to client commands + device
/// events. The spawned task ships its outcome back via internal_tx,
/// where handle_internal_event picks it up to finalize state + history.
fn start_sync_session(
    trigger: SyncTrigger,
    serial: String,
    drive: String,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_sync: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    prompt_tx_holder: &mut Option<mpsc::UnboundedSender<(u64, i32)>>,
    pause_tx_holder: &mut Option<oneshot::Sender<()>>,
    library_count_cache: Option<usize>,
) {
    if state.try_start_sync_for_device(trigger.clone(), serial.clone(), drive.clone())
        != TriggerOutcome::Accepted
    {
        return;
    }
    let started_at_unix_secs = match state.state() {
        DaemonState::Syncing(s) => s.started_at_unix_secs,
        _ => 0,
    };
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: DaemonStateLabel::Syncing,
        configured: true,
        ipod_connected: true,
        last_sync: None,
        next_scheduled_unix_secs: None,
        storage: device_storage::query_storage(&drive),
        synced_count: synced_track_count(),
        library_count: library_count_cache,
    });

    // Per-sync cancel channel. Sender held by the runtime so the
    // CancelSync IPC command can wake the orchestrator; Receiver
    // passed into the spawn closure.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    *cancel_tx_holder = Some(cancel_tx);

    // Per-sync prompt-decision channel. Sender held by the runtime
    // so DaemonCommand::DecidePrompt can ferry user replies through
    // to the subprocess. Receiver passed into the spawn closure for
    // the orchestrator's select loop to read.
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<(u64, i32)>();
    *prompt_tx_holder = Some(prompt_tx);

    // Per-sync pause channel. Sender held by the runtime so the Pause
    // IPC command can wake the orchestrator; Receiver passed into the
    // spawn closure.
    let (pause_tx, pause_rx) = oneshot::channel::<()>();
    *pause_tx_holder = Some(pause_tx);

    let spawn_sync = spawn_sync.clone();
    let internal_tx = internal_tx.clone();
    let drive_for_task = drive.clone();
    let trigger_for_task = trigger.clone();
    let serial_for_task = serial.clone();
    let serial_for_spawn = serial;
    tokio::spawn(async move {
        let outcome = (spawn_sync)(serial_for_spawn, drive_for_task, cancel_rx, pause_rx, prompt_rx).await;
        let _ = internal_tx.send(InternalEvent::SyncCompleted {
            trigger: trigger_for_task,
            serial: serial_for_task,
            started_at_unix_secs,
            outcome,
        });
    });
}

/// Start a library-scan subprocess. Mirrors `start_sync_session` minus the
/// device/serial and history bookkeeping — a scan touches no iPod and writes
/// no history. Shares the guard, cancel/pause/prompt channels, and event
/// relay, so a scan and a sync can never run concurrently.
#[allow(clippy::too_many_arguments)]
fn start_scan_session(
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    prompt_tx_holder: &mut Option<mpsc::UnboundedSender<(u64, i32)>>,
    pause_tx_holder: &mut Option<oneshot::Sender<()>>,
    connected: &Option<DetectedIpod>,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count_cache: Option<usize>,
) {
    if state.try_start_scan() != TriggerOutcome::Accepted {
        return;
    }
    broadcast_status(event_tx, state, connected, config_path, history, library_count_cache);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    *cancel_tx_holder = Some(cancel_tx);
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<(u64, i32)>();
    *prompt_tx_holder = Some(prompt_tx);
    let (pause_tx, pause_rx) = oneshot::channel::<()>();
    *pause_tx_holder = Some(pause_tx);

    let spawn_scan = spawn_scan.clone();
    let internal_tx = internal_tx.clone();
    tokio::spawn(async move {
        let outcome = (spawn_scan)(String::new(), String::new(), cancel_rx, pause_rx, prompt_rx).await;
        let _ = internal_tx.send(InternalEvent::ScanCompleted { outcome });
    });
}

fn make_history_entry(
    trigger: SyncTrigger,
    outcome: SyncOutcome,
    error_message: Option<String>,
    summary: Option<SyncSummary>,
    started_at_unix_secs: u64,
    db_restored: bool,
) -> HistoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let duration = now.saturating_sub(started_at_unix_secs);
    HistoryEntry {
        timestamp: crate::daemon::format::rfc3339(now),
        duration_secs: duration,
        trigger,
        outcome,
        error_message,
        summary,
        db_restored,
    }
}

/// Map the state machine to the wire's `status_update.state`. A `Scan`
/// session reports `Scanning`; a real sync reports `Syncing`.
fn state_label(state: &StateMachine) -> DaemonStateLabel {
    match state.state() {
        DaemonState::Idle => DaemonStateLabel::Idle,
        DaemonState::Syncing(s) if s.kind == SessionKind::Scan => DaemonStateLabel::Scanning,
        DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
    }
}

fn broadcast_status(
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &StateMachine,
    connected: &Option<DetectedIpod>,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count: Option<usize>,
) {
    let configured = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.ipod_identity)
        .is_some();
    // Selection-aware Y: under a non-All selection this is the *selected*
    // track count; under mode=All it returns None and we keep the walk cache.
    let library_count = crate::daemon::library::selected_library_count(config_path)
        .or(library_count);
    let entries = history.read();
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: state_label(state),
        configured,
        ipod_connected: connected.is_some(),
        last_sync: entries.last().cloned(),
        next_scheduled_unix_secs: None,
        storage: current_storage(connected),
        synced_count: synced_track_count(),
        library_count,
    });
}

/// Tracks currently on the iPod per the manifest (X in "X of Y synced").
/// Cheap and always fresh — just a JSON read + `Vec::len()`, no source
/// walk. Falls back to 0 if the manifest path can't be resolved or the
/// manifest doesn't exist yet (nothing synced yet is a legitimate 0, not
/// an error worth surfacing on a status tick).
fn synced_track_count() -> usize {
    let Ok(manifest_path) = crate::config::default_manifest_path() else { return 0 };
    crate::manifest::load_or_default(&manifest_path)
        .map(|m| m.tracks.len())
        .unwrap_or(0)
}

/// Kick off an off-thread walk of the configured source library to fill the
/// cached `library_count` (Y in "X of Y synced"), delivered back via
/// `InternalEvent::LibraryCountComputed`. No-op when no source is configured.
///
/// Runs on `spawn_blocking` because a large library on a slow/spinning volume
/// can take a while to walk — doing it inline would stall the daemon loop.
/// This is what lets "X of Y" appear on a cold start (fresh daemon, before any
/// sync): the walk fills Y proactively instead of waiting for a sync's diff.
/// Called at startup and after `SaveConfig` (the source path may have changed).
fn spawn_library_count(
    config_path: &std::path::Path,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
) {
    let config_path = config_path.to_path_buf();
    let tx = internal_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(count) = count_source_library(&config_path) {
            let _ = tx.send(InternalEvent::LibraryCountComputed { count });
        }
    });
}

/// Resolve the configured source path and count its library tracks (Y in
/// "X of Y synced"), applying the same `*.flac` + skip rules as a real sync
/// (`source::walk`). `None` when no source is configured yet, or the walk
/// failed (e.g. the source volume is unreachable — better to keep the last
/// known Y than to flap). Extracted from `spawn_library_count` so the
/// config-resolve + count logic is unit-testable without a tokio runtime.
fn count_source_library(config_path: &std::path::Path) -> Option<usize> {
    let source = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source)?;
    let started = std::time::Instant::now();
    match crate::source::walk(&source) {
        Ok(entries) => {
            tracing::info!(
                "daemon: counted source library ({} tracks) in {}ms",
                entries.len(),
                started.elapsed().as_millis()
            );
            Some(entries.len())
        }
        Err(e) => {
            tracing::warn!("daemon: source-library count walk failed: {e:#}");
            None
        }
    }
}

/// Query free + total bytes for the connected iPod's drive. `None` when
/// no device is connected OR when the volume query failed (drive may
/// have been unplugged mid-tick). UI treats absence as "no info yet".
fn current_storage(connected: &Option<DetectedIpod>) -> Option<StorageInfo> {
    connected
        .as_ref()
        .and_then(|d| device_storage::query_storage(&d.drive))
}

/// Handle one client command. Returns `true` iff the daemon should exit
/// its main loop (currently only the Shutdown command sets this — the
/// outer loop then runs the graceful-drain sequence so the in-flight
/// sync subprocess doesn't get yanked mid-write).
fn handle_client_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    connected: &Option<DetectedIpod>,
    spawn_sync: &SpawnFn,
    spawn_backfill: &SpawnFn,
    spawn_replace_library: &SpawnFn,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    prompt_tx_holder: &mut Option<mpsc::UnboundedSender<(u64, i32)>>,
    pause_tx_holder: &mut Option<oneshot::Sender<()>>,
    configured_serial: &mut Option<String>,
    scheduler: &mut SyncScheduler,
    library_count_cache: &mut Option<usize>,
) -> bool {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    match command {
        DaemonCommand::GetStatus => {
            let configured = configured_serial.is_some();
            let library_count = crate::daemon::library::selected_library_count(config_path)
                .or(*library_count_cache);
            let entries = history.read();
            let _ = reply.send(DaemonEvent::StatusUpdate {
                state: state_label(state),
                configured,
                ipod_connected: connected.is_some(),
                last_sync: entries.last().cloned(),
                next_scheduled_unix_secs: None,
                storage: current_storage(connected),
                synced_count: synced_track_count(),
                library_count,
            });
        }
        DaemonCommand::GetConfig => {
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = reply.send(build_config_update(cfg));
        }
        DaemonCommand::SaveConfig { source, daemon, ipod } => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            if let Some(s) = source { current.source = Some(PathBuf::from(s)); }
            if let Some(d) = daemon { current.daemon = Some(d); }
            if let Some(mut i) = ipod {
                // Wizard / settings clients don't know the iPod's
                // firmware name — preserve it across saves so the user
                // doesn't lose "Michael's iPod" the moment they re-run
                // the wizard for the same device. Only carry it over
                // when serials match (different iPod = clean slate).
                if i.name.is_none() {
                    if let Some(prev) = current.ipod_identity.as_ref() {
                        if prev.serial == i.serial { i.name = prev.name.clone(); }
                    }
                }
                // Seed the per-device selection.json from the shared one the
                // first time this device flips shared -> custom, so the
                // user's existing choices carry over instead of silently
                // resetting to mode=All on the very first per-device load.
                // Only on the false->true transition (or no prior identity
                // for this serial) — flipping back to false intentionally
                // leaves the per-device file dormant, and
                // seed_custom_selection() is itself a no-op once the
                // per-device file exists, so re-saving with the flag
                // already true is harmless. Seeded BEFORE the config save
                // below so a crash in between never leaves the flag on
                // with no per-device file backing it.
                let was_custom = current.ipod_identity.as_ref()
                    .filter(|prev| prev.serial == i.serial)
                    .map(|prev| prev.custom_selection)
                    .unwrap_or(false);
                if i.custom_selection && !was_custom {
                    match (
                        crate::selection::default_selection_path(),
                        crate::device_state::device_selection_path(&i.serial),
                    ) {
                        (Ok(shared), Ok(custom)) => {
                            if let Err(e) = crate::selection::seed_custom_selection(&shared, &custom) {
                                tracing::warn!(
                                    "daemon: failed to seed per-device selection for {}: {e:#}",
                                    i.serial,
                                );
                            }
                        }
                        _ => tracing::warn!(
                            "daemon: cannot resolve selection paths to seed per-device selection for {}",
                            i.serial,
                        ),
                    }
                }
                current.ipod_identity = Some(i);
            }
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config: {e}");
                return false;
            }
            // Invalidate the cached library count — the source path may
            // have changed, which would make the cached Y stale — then kick
            // off a fresh walk so "X of Y" refreshes without waiting for the
            // next sync. (A sync diff also refreshes it; whichever lands first.)
            *library_count_cache = None;
            spawn_library_count(config_path, internal_tx);
            // Mirror the persisted ipod identity into the in-memory
            // `configured_serial` so subsequent plug-in / TriggerSync
            // checks see the post-wizard state without needing a daemon
            // restart. Without this, the wizard's first SaveConfig is
            // invisible to the daemon's auto-sync gate.
            *configured_serial = current.ipod_identity.as_ref().map(|id| id.serial.clone());
            // Live-reload the scheduled-sync interval. The scheduler is built
            // once at startup, so without this a schedule change in Settings
            // wouldn't take effect until the daemon restarted. Only re-arm on
            // an actual change — rearm() resets the countdown, so re-arming on
            // every save would let frequent edits perpetually postpone a tick.
            let new_minutes = current.daemon.as_ref().map(|d| d.schedule_minutes).unwrap_or(0);
            if new_minutes != scheduler.minutes() {
                tracing::info!(
                    "daemon: schedule interval {} → {} min; re-arming scheduler",
                    scheduler.minutes(),
                    new_minutes,
                );
                scheduler.rearm(new_minutes);
            }
            let _ = event_tx.send(build_config_update(Some(current)));
        }
        DaemonCommand::ForgetIpod => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            current.ipod_identity = None;
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config after forget_ipod: {e}");
                return false;
            }
            *configured_serial = None;
            tracing::info!("daemon: client {client_id} cleared the persisted iPod identity");
            let _ = event_tx.send(build_config_update(Some(current)));
            // Re-announce the currently-attached device (if any) so a
            // freshly-opened wizard sees it. Without this re-emit, the
            // device-watcher's polling loop is in steady-state — the
            // device is still physically connected so no transition
            // event fires, and the wizard's DeviceConnected subscriber
            // waits forever.
            if let Some(device) = connected.as_ref() {
                let _ = event_tx.send(DaemonEvent::DeviceConnected {
                    serial: device.serial.clone(),
                    model_label: device.model_label.clone(),
                    drive: device.drive.clone(),
                    name: device.name.clone(),
                });
            }
        }
        DaemonCommand::GetHistory { limit } => {
            let mut entries = history.read();
            let start = entries.len().saturating_sub(limit);
            entries.drain(..start);
            let _ = reply.send(DaemonEvent::HistoryUpdate { entries });
        }
        DaemonCommand::TriggerSync { source: trigger_source } => {
            if !state.is_idle() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::AlreadySyncing,
                });
                return false;
            }
            let Some(device) = connected.as_ref() else {
                let _ = reply.send(DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod });
                return false;
            };
            if configured_serial.is_none() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::NotConfigured,
                });
                return false;
            }
            let trigger = match trigger_source {
                TriggerSource::Manual => SyncTrigger::Manual,
                TriggerSource::Scheduled => SyncTrigger::Scheduled,
                TriggerSource::PlugIn => SyncTrigger::PlugIn,
            };
            let _ = history;  // history mutations now happen in handle_internal_event
            start_sync_session(
                trigger,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_sync,
                internal_tx,
                cancel_tx_holder,
                prompt_tx_holder,
                pause_tx_holder,
                *library_count_cache,
            );
        }
        DaemonCommand::BackfillRockbox => {
            // Mirrors TriggerSync's guard + spawn + relay path exactly,
            // just pointed at `spawn_backfill` (a `--backfill-rockbox`
            // subprocess) instead of `spawn_sync` (`--apply`).
            // `start_sync_session`'s state-machine check is what makes a
            // backfill and a sync mutually exclusive — whichever gets
            // there first flips state to Syncing and the other is
            // dropped/no-op.
            if !state.is_idle() {
                tracing::debug!(
                    "daemon: client {client_id} sent backfill_rockbox but a sync is already in progress"
                );
                return false;
            }
            let Some(device) = connected.as_ref() else {
                tracing::debug!(
                    "daemon: client {client_id} sent backfill_rockbox but no iPod is connected"
                );
                return false;
            };
            tracing::info!(
                "daemon: client {client_id} triggered a Rockbox-compat backfill for {}",
                device.serial
            );
            start_sync_session(
                SyncTrigger::Manual,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_backfill,
                internal_tx,
                cancel_tx_holder,
                prompt_tx_holder,
                pause_tx_holder,
                *library_count_cache,
            );
        }
        DaemonCommand::ReplaceLibrary => {
            // Mirrors BackfillRockbox's arm exactly, just pointed at
            // `spawn_replace_library` (a `--replace-library --apply`
            // subprocess) instead of `spawn_backfill`. `start_sync_session`'s
            // state-machine check is what makes a replace mutually exclusive
            // with a sync/backfill — whichever gets there first flips state
            // to Syncing and the other is dropped/no-op. The UI does its own
            // typed confirmation before ever sending this command, so there's
            // no confirmation prompt to relay here (`--apply` already skips
            // the core's interactive one). Unlike BackfillRockbox/ScanLibrary,
            // this command is destructive (wipes the on-device library), so
            // a busy/no-device guard replies with `SyncRejected` (mirroring
            // TriggerSync's reply mechanism) instead of silently dropping —
            // the UI needs a definitive answer before it can retry or warn.
            if !state.is_idle() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::AlreadySyncing,
                });
                tracing::debug!(
                    "daemon: client {client_id} sent replace_library but a sync is already in progress"
                );
                return false;
            }
            let Some(device) = connected.as_ref() else {
                let _ = reply.send(DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod });
                tracing::debug!(
                    "daemon: client {client_id} sent replace_library but no iPod is connected"
                );
                return false;
            };
            tracing::info!(
                "daemon: client {client_id} triggered a library replace for {}",
                device.serial
            );
            start_sync_session(
                SyncTrigger::Manual,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                spawn_replace_library,
                internal_tx,
                cancel_tx_holder,
                prompt_tx_holder,
                pause_tx_holder,
                *library_count_cache,
            );
        }
        DaemonCommand::CancelSync => {
            // Wake the orchestrator's cancel arm. The orchestrator
            // writes a Cancel command to subprocess stdin and force-kills
            // after 5s; the SyncCompleted internal event arrives shortly
            // with outcome = Aborted{reason="user_cancelled"}.
            if let Some(tx) = cancel_tx_holder.take() {
                let _ = tx.send(());
                tracing::info!("daemon: client {client_id} cancelled the running sync");
            } else {
                tracing::debug!("daemon: client {client_id} sent cancel_sync but no sync is in progress");
            }
        }
        DaemonCommand::Pause => {
            // Wake the orchestrator's pause arm. Unlike CancelSync, this
            // is graceful — no force-kill; the SyncCompleted internal
            // event arrives once the subprocess has drained, checkpointed,
            // emitted "paused", and exited on its own.
            if let Some(tx) = pause_tx_holder.take() {
                let _ = tx.send(());
                tracing::info!("daemon: client {client_id} requested pause of the running sync");
            } else {
                tracing::debug!("daemon: client {client_id} sent pause but no sync is in progress");
            }
        }
        DaemonCommand::DecidePrompt { id, choice } => {
            // Forward the user's reply to the running sync subprocess.
            // The orchestrator writes the prompt_decision line to
            // stdin; the apply loop's await_prompt then returns the
            // chosen PromptOutcome and the sync proceeds.
            if let Some(tx) = prompt_tx_holder.as_ref() {
                if tx.send((id, choice)).is_err() {
                    tracing::warn!(
                        "daemon: client {client_id} sent decide_prompt id={id} choice={choice} \
                         but the orchestrator channel was already closed (sync probably ended)"
                    );
                } else {
                    tracing::info!(
                        "daemon: client {client_id} answered prompt id={id} → choice={choice}"
                    );
                }
            } else {
                tracing::debug!(
                    "daemon: client {client_id} sent decide_prompt id={id} but no sync is in progress"
                );
            }
        }
        DaemonCommand::SubscribeDeviceEvents => {
            // All clients see all device events on the shared
            // broadcast channel — subscribe is a handshake, not a
            // routing op. BUT a late subscriber misses any
            // DeviceConnected emitted before the subscribe (e.g. the
            // first-poll event on daemon startup, before the wizard
            // opens). Re-broadcast for any currently-attached device
            // so the late subscriber sees the steady state.
            if let Some(device) = connected.as_ref() {
                let _ = event_tx.send(DaemonEvent::DeviceConnected {
                    serial: device.serial.clone(),
                    model_label: device.model_label.clone(),
                    drive: device.drive.clone(),
                    name: device.name.clone(),
                });
            }
        }
        DaemonCommand::UnsubscribeDeviceEvents => {
            // Symmetric no-op — subscription is implicit, so there's
            // nothing to tear down.
        }
        DaemonCommand::GetLibrary => {
            let _ = reply.send(crate::daemon::library::build_library_update(config_path));
        }
        DaemonCommand::ScanLibrary => {
            if !state.is_idle() {
                tracing::debug!("daemon: client {client_id} sent scan_library while busy; dropped");
                return false;
            }
            let has_source = config_file::load(config_path).ok().flatten()
                .and_then(|c| c.source).is_some();
            if !has_source {
                tracing::debug!("daemon: client {client_id} sent scan_library but no source configured; dropped");
                return false;
            }
            tracing::info!("daemon: client {client_id} triggered a library scan");
            start_scan_session(
                state, event_tx, spawn_scan, internal_tx,
                cancel_tx_holder, prompt_tx_holder, pause_tx_holder,
                connected, config_path, history, *library_count_cache,
            );
        }
        DaemonCommand::GetSelection => {
            // Deprecated as of v1.6.0 (see ipc_daemon.rs doc comment): reads
            // the CONFIGURED device's own per-device selection via
            // `effective_device_selection_path`, not the old
            // `custom_selection`-gated shared/per-device split. No
            // configured device -> mode All (nothing to resolve a
            // per-device path for).
            let identity = config_file::load(config_path).ok().flatten().and_then(|c| c.ipod_identity);
            let sel = match identity {
                Some(id) => crate::selection::effective_device_selection_path(&id.serial)
                    .map(|p| crate::selection::load_or_all(&p))
                    .unwrap_or_else(|_| crate::selection::Selection::all()),
                None => crate::selection::Selection::all(),
            };
            let _ = reply.send(DaemonEvent::SelectionUpdate { mode: sel.mode, rules: sel.rules });
        }
        DaemonCommand::SaveSelection { mode, rules } => {
            // Deprecated as of v1.6.0: same per-device target as GetSelection.
            let sel = crate::selection::Selection {
                version: crate::selection::SELECTION_VERSION,
                mode,
                rules,
            };
            let identity = config_file::load(config_path).ok().flatten().and_then(|c| c.ipod_identity);
            let Some(identity) = identity else {
                tracing::warn!("daemon: client {client_id} save_selection with no configured device; dropped");
                return false;
            };
            match crate::selection::effective_device_selection_path(&identity.serial) {
                Ok(path) => {
                    if let Err(e) = crate::selection::save_atomic(&path, &sel) {
                        tracing::error!("daemon: failed to save selection: {e:#}");
                        return false;
                    }
                }
                Err(e) => {
                    tracing::error!("daemon: cannot resolve selection path: {e:#}");
                    return false;
                }
            }
            let _ = reply.send(DaemonEvent::SelectionUpdate { mode: sel.mode, rules: sel.rules });
            // Y in "X of Y" likely changed; push a fresh status to everyone.
            broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
        }
        DaemonCommand::PreviewSelection { mode, rules } => {
            let source = config_file::load(config_path).ok().flatten().and_then(|c| c.source);
            let index = match (source, crate::library_index::default_index_path()) {
                (Some(root), Ok(p)) => crate::library_index::load_or_empty(&p, &root),
                _ => crate::library_index::LibraryIndex::empty(std::path::PathBuf::new()),
            };
            let manifest = crate::config::default_manifest_path()
                .and_then(|p| crate::manifest::load_or_default(&p))
                .unwrap_or_else(|_| crate::manifest::Manifest::empty());
            let (selected_tracks, selected_bytes, adds, removes) =
                crate::daemon::library::preview(&index, &manifest, mode, &rules);
            let _ = reply.send(DaemonEvent::SelectionPreview {
                selected_tracks, selected_bytes, adds, removes,
            });
        }
        DaemonCommand::ListPlaylists => {
            let _ = reply.send(build_playlists_update(config_path));
        }
        DaemonCommand::GetPlaylist { slug } => {
            let _ = reply.send(build_playlist_detail(&slug));
        }
        DaemonCommand::SavePlaylist { playlist } => {
            let Ok(store) = open_playlist_store() else {
                tracing::warn!("daemon: client {client_id} save_playlist: could not open playlist store");
                return false;
            };
            let built = match playlist {
                crate::ipc_daemon::PlaylistPayload::Manual { slug, name, tracks } => {
                    let slug = match slug {
                        Some(s) => s,
                        None => match store.unique_slug(&name) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "daemon: client {client_id} save_playlist: could not allocate a slug for {name:?}: {e:#}"
                                );
                                return false;
                            }
                        },
                    };
                    crate::playlist::Playlist::Manual(crate::playlist::ManualPlaylist {
                        slug,
                        name,
                        tracks: tracks.into_iter().map(PathBuf::from).collect(),
                        skipped_unsafe: 0,
                    })
                }
                crate::ipc_daemon::PlaylistPayload::Smart { slug, name, rules } => {
                    let slug = match slug {
                        Some(s) => s,
                        None => match store.unique_slug(&name) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "daemon: client {client_id} save_playlist: could not allocate a slug for {name:?}: {e:#}"
                                );
                                return false;
                            }
                        },
                    };
                    crate::playlist::Playlist::Smart(crate::playlist::SmartPlaylist { slug, name, rules })
                }
            };
            if let Err(e) = store.save(&built) {
                tracing::error!("daemon: client {client_id} save_playlist failed: {e:#}");
                return false;
            }
            let _ = event_tx.send(build_playlists_update(config_path));
        }
        DaemonCommand::DeletePlaylist { slug } => {
            match open_playlist_store() {
                Ok(store) => {
                    if let Err(e) = store.delete(&slug) {
                        tracing::error!("daemon: client {client_id} delete_playlist({slug}) failed: {e:#}");
                    }
                }
                Err(e) => {
                    tracing::warn!("daemon: client {client_id} delete_playlist: could not open playlist store ({e:#})");
                }
            }
            let _ = event_tx.send(build_playlists_update(config_path));
        }
        DaemonCommand::GetDeviceConfig { serial } => {
            let _ = reply.send(build_device_config_update(config_path, &serial));
        }
        DaemonCommand::SaveDeviceConfig { serial, selection, subscriptions, settings } => {
            if let Some(sel) = selection {
                match crate::selection::effective_device_selection_path(&serial) {
                    Ok(path) => {
                        let full = crate::selection::Selection {
                            version: crate::selection::SELECTION_VERSION,
                            mode: sel.mode,
                            rules: sel.rules,
                        };
                        if let Err(e) = crate::selection::save_atomic(&path, &full) {
                            tracing::error!("daemon: failed to save device selection for {serial}: {e:#}");
                        }
                    }
                    Err(e) => tracing::error!("daemon: cannot resolve device selection path for {serial}: {e:#}"),
                }
            }
            if let Some(subs) = subscriptions {
                match crate::device_state::device_subscriptions_path(&serial) {
                    Ok(path) => {
                        let full = crate::device_config::Subscriptions {
                            version: crate::device_config::SUBSCRIPTIONS_VERSION,
                            playlists: subs.playlists,
                        };
                        if let Err(e) = crate::device_config::Subscriptions::save_atomic(&path, &full) {
                            tracing::error!("daemon: failed to save subscriptions for {serial}: {e:#}");
                        }
                    }
                    Err(e) => tracing::error!("daemon: cannot resolve subscriptions path for {serial}: {e:#}"),
                }
            }
            if let Some(set) = settings {
                match crate::device_state::device_settings_path(&serial) {
                    Ok(path) => {
                        let full = crate::device_config::DeviceSettings {
                            version: crate::device_config::DEVICE_SETTINGS_VERSION,
                            auto_sync: set.auto_sync,
                            rockbox_compat: set.rockbox_compat,
                        };
                        if let Err(e) = crate::device_config::DeviceSettings::save_atomic(&path, &full) {
                            tracing::error!("daemon: failed to save settings for {serial}: {e:#}");
                        }
                    }
                    Err(e) => tracing::error!("daemon: cannot resolve settings path for {serial}: {e:#}"),
                }
            }
            let _ = event_tx.send(build_device_config_update(config_path, &serial));
            // Selection may have changed; refresh "X of Y" for everyone, but
            // only when this save targets the CONFIGURED device — status
            // reflects that one device, not every device this daemon has
            // ever seen a config file for.
            if configured_serial.as_deref() == Some(serial.as_str()) {
                broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
            }
        }
        DaemonCommand::PreviewDevice { serial } => {
            let _ = reply.send(build_device_preview(config_path, connected, &serial));
        }
        DaemonCommand::Shutdown => {
            tracing::info!("daemon: shutdown requested by client {client_id}; exiting loop");
            return true;
        }
    }
    false
}

fn build_config_update(cfg: Option<PersistedConfig>) -> DaemonEvent {
    match cfg {
        Some(c) => DaemonEvent::ConfigUpdate {
            source: c.source.map(|p| p.display().to_string()),
            daemon: c.daemon,
            ipod: c.ipod_identity,
        },
        None => DaemonEvent::ConfigUpdate { source: None, daemon: None, ipod: None },
    }
}

/// The cached library index for the configured source, same pattern as the
/// inline load in `PreviewSelection`'s arm — an empty index (root `""`)
/// when there's no source configured yet or the cache file is missing.
fn load_cached_index(config_path: &std::path::Path) -> crate::library_index::LibraryIndex {
    let source = config_file::load(config_path).ok().flatten().and_then(|c| c.source);
    match (source, crate::library_index::default_index_path()) {
        (Some(root), Ok(p)) => crate::library_index::load_or_empty(&p, &root),
        _ => crate::library_index::LibraryIndex::empty(PathBuf::new()),
    }
}

/// Open (creating on demand) the one playlist store at its default root.
fn open_playlist_store() -> Result<crate::playlist::PlaylistStore> {
    crate::playlist::PlaylistStore::default_root().and_then(crate::playlist::PlaylistStore::open)
}

/// `list_playlists` reply / `save_playlist`+`delete_playlist` broadcast
/// payload: every playlist in the store, summarized against the cached
/// index. A store-open failure (e.g. an unwritable config dir) degrades to
/// an empty list with a logged warning rather than failing the arm.
fn build_playlists_update(config_path: &std::path::Path) -> DaemonEvent {
    let index = load_cached_index(config_path);
    match open_playlist_store() {
        Ok(store) => {
            DaemonEvent::PlaylistsUpdate { playlists: crate::daemon::library::build_playlist_summaries(&store, &index) }
        }
        Err(e) => {
            tracing::warn!("daemon: playlists: failed to open store ({e:#}); replying with an empty list");
            DaemonEvent::PlaylistsUpdate { playlists: Vec::new() }
        }
    }
}

/// `get_playlist` reply: this playlist's full content — the manual track
/// list or the smart rule set — for the editor, unlike
/// `build_playlists_update`'s per-list summaries (track count only). An
/// unopenable store, a missing slug, or an on-disk file that fails to
/// parse all reply with `error` set (and every content field `None`)
/// rather than degrading to an empty result — see the `PlaylistDetail`
/// and `GetPlaylist` doc comments in `ipc_daemon.rs`.
fn build_playlist_detail(slug: &str) -> DaemonEvent {
    let store = match open_playlist_store() {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!("daemon: get_playlist({slug}): could not open playlist store ({e:#})");
            return DaemonEvent::PlaylistDetail {
                slug: slug.to_string(), name: None, kind: None, tracks: None, rules: None,
                error: Some(format!("could not open playlist store: {e:#}")),
            };
        }
    };
    match store.load(slug) {
        Ok(Some(crate::playlist::Playlist::Manual(m))) => DaemonEvent::PlaylistDetail {
            slug: m.slug,
            name: Some(m.name),
            kind: Some(PlaylistKind::Manual),
            tracks: Some(
                m.tracks.into_iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect(),
            ),
            rules: None,
            error: None,
        },
        Ok(Some(crate::playlist::Playlist::Smart(s))) => DaemonEvent::PlaylistDetail {
            slug: s.slug,
            name: Some(s.name),
            kind: Some(PlaylistKind::Smart),
            tracks: None,
            rules: Some(s.rules),
            error: None,
        },
        Ok(None) => {
            tracing::warn!("daemon: get_playlist({slug}): no such playlist");
            DaemonEvent::PlaylistDetail {
                slug: slug.to_string(), name: None, kind: None, tracks: None, rules: None,
                error: Some("no such playlist".to_string()),
            }
        }
        Err(e) => {
            tracing::warn!("daemon: get_playlist({slug}): failed to read ({e:#})");
            DaemonEvent::PlaylistDetail {
                slug: slug.to_string(), name: None, kind: None, tracks: None, rules: None,
                error: Some(format!("{e:#}")),
            }
        }
    }
}

/// `get_device_config` reply / `save_device_config` broadcast payload: one
/// device's resolved selection + subscriptions + settings. Every part
/// fails open to its type's default (see `selection::load_or_all`,
/// `Subscriptions::load_or_default`, `DeviceSettings::load_or_migrate`) —
/// this never fails the arm, even for a `serial` the daemon has never seen.
fn build_device_config_update(config_path: &std::path::Path, serial: &str) -> DaemonEvent {
    let selection = crate::selection::effective_device_selection_path(serial)
        .map(|p| crate::selection::load_or_all(&p))
        .unwrap_or_else(|_| crate::selection::Selection::all());
    let subscriptions = crate::device_state::device_subscriptions_path(serial)
        .map(|p| crate::device_config::Subscriptions::load_or_default(&p))
        .unwrap_or_default();
    let global = config_file::load(config_path).ok().flatten().unwrap_or_default();
    let settings = crate::device_config::DeviceSettings::load_or_migrate(serial, &global);
    DaemonEvent::DeviceConfigUpdate {
        serial: serial.to_string(),
        selection: crate::ipc_daemon::SelectionPayload { mode: selection.mode, rules: selection.rules },
        subscriptions: crate::ipc_daemon::SubscriptionsPayload { playlists: subscriptions.playlists },
        settings: crate::ipc_daemon::DeviceSettingsPayload {
            auto_sync: settings.auto_sync,
            rockbox_compat: settings.rockbox_compat,
        },
    }
}

/// `preview_device` reply: gathers this device's cached index + selection +
/// subscriptions + playlist store, plus a live free-bytes baseline (only
/// when `serial` is the device currently connected), and hands off to the
/// pure `daemon::library::compute_device_preview`.
fn build_device_preview(
    config_path: &std::path::Path,
    connected: &Option<DetectedIpod>,
    serial: &str,
) -> DaemonEvent {
    let index = load_cached_index(config_path);
    let selection = crate::selection::effective_device_selection_path(serial)
        .map(|p| crate::selection::load_or_all(&p))
        .unwrap_or_else(|_| crate::selection::Selection::all());
    let subs = crate::device_state::device_subscriptions_path(serial)
        .map(|p| crate::device_config::Subscriptions::load_or_default(&p))
        .unwrap_or_default();
    let store = open_playlist_store();
    if let Err(e) = &store {
        tracing::warn!("daemon: preview_device({serial}): failed to open playlist store ({e:#}); playlist subscriptions ignored");
    }
    let current_free_bytes = connected
        .as_ref()
        .filter(|d| d.serial == serial)
        .and_then(|d| device_storage::query_storage(&d.drive))
        .map(|s| s.free_bytes);
    crate::daemon::library::compute_device_preview(&index, &selection, &subs, store.as_ref().ok(), current_free_bytes)
}

fn history_file_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA unavailable"))?
        .join(crate::PROJECT_DIR);
    Ok(base.join("history.json"))
}

// Suppress the unused-import warning when the test build doesn't take this path.
#[allow(dead_code)]
fn _silence_mpsc_warning(_: mpsc::Sender<DaemonEvent>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::PersistedConfig;

    #[test]
    fn state_label_maps_scan_sessions_to_scanning() {
        let mut sm = StateMachine::new();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Idle));
        sm.try_start_scan();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Scanning));
        sm.finish_sync();
        sm.try_start_sync(SyncTrigger::Manual);
        assert!(matches!(state_label(&sm), DaemonStateLabel::Syncing));
    }

    // should_auto_sync is the trivial pure seam over per-device settings —
    // migration/seeding from the global `enabled` flag is covered by
    // device_config.rs's `load_or_migrate` tests, not re-tested here.
    #[test]
    fn should_auto_sync_follows_device_setting() {
        use crate::device_config::DeviceSettings;
        assert!(should_auto_sync(&DeviceSettings { auto_sync: true, ..DeviceSettings::default() }));
        assert!(
            !should_auto_sync(&DeviceSettings { auto_sync: false, ..DeviceSettings::default() }),
            "auto-sync must be off when the device setting is off"
        );
    }

    // Cold-start "X of Y": count_source_library resolves the config's source
    // and counts flac tracks with the same skip rules as a real sync, so Y is
    // known before any sync runs. Returns None when no source is configured.
    #[test]
    fn count_source_library_counts_flac_respecting_skip_rules() {
        use std::fs;
        let base = std::env::temp_dir().join(format!("classick-libcount-{}", std::process::id()));
        let src = base.join("music");
        fs::create_dir_all(&src).unwrap();
        for n in ["a.flac", "b.flac", "c.flac"] {
            fs::write(src.join(n), b"x").unwrap();
        }
        fs::write(src.join("notes.txt"), b"x").unwrap(); // not flac → ignored
        fs::create_dir_all(src.join("_excluded")).unwrap();
        fs::write(src.join("_excluded/skip.flac"), b"x").unwrap(); // skipped dir → ignored

        let cfg_path = base.join("config.toml");
        let cfg = PersistedConfig { source: Some(src.clone()), ..Default::default() };
        crate::config_file::save(&cfg_path, &cfg).unwrap();
        assert_eq!(count_source_library(&cfg_path), Some(3));

        // No source configured → None (Y stays unknown, menu shows "X synced").
        let empty_path = base.join("empty.toml");
        crate::config_file::save(&empty_path, &PersistedConfig::default()).unwrap();
        assert_eq!(count_source_library(&empty_path), None);

        let _ = fs::remove_dir_all(&base);
    }
}
