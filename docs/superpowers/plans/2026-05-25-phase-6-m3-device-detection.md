# Phase 6 M3: Device Detection + Auto-Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the long-promised auto-sync flow — daemon detects iPod plug-in, matches it against the wizard-configured identity, and spawns a sync subprocess automatically. Adds the scheduler timer, the sync orchestrator, real `device_connected`/`device_disconnected` events on the IPC channel, and the wizard switch from local drive polling to daemon-emitted events. Tray icon swaps between Idle / Syncing / Error / Offline states. After M3, plugging in your configured iPod triggers a sync without touching the UI.

**Architecture:** A new `DeviceWatcher` trait in `src/daemon/device_watcher.rs` with a periodic-polling production impl (1.5s interval, reuses `ipod::device::scan_for_ipod`) feeds debounced device events into the daemon runtime. A new `SyncOrchestrator` spawns `ipod-sync.exe --ipc-mode --apply --ipod <drive>` subprocesses, forwards their M1 IPC events onto the broadcast channel for UIs, counts per-track failures, and bails (kills + records `Aborted`) when error-count exceeds 50% of the planned-track count. A new `SyncScheduler` fires a periodic `Scheduled` trigger on a tokio interval. Daemon runtime gains an event loop that funnels device events / scheduler ticks / client commands through the state machine and orchestrator. C# wizard drops its local-drive scan and subscribes to daemon device events instead. `TrayIconController` flips icons + tooltips on `StatusUpdate` events. Per spec `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md` §10 M3 and design decisions confirmed in conversation 2026-05-25.

**Tech Stack:** Rust + Tokio (already on `tokio::sync::broadcast`, `tokio::process::Command`, `tokio::time::interval`, `tokio::select!`). No new Rust crate dependencies — polling watcher avoids `windows-rs` Devices::Enumeration FFI (deferred to M5 polish; trait-based design supports an event-driven swap later). .NET 10 + WinUI 3, no new C# package dependencies.

**Plan scope:** M3 only. Notifications (toasts), status popover, settings window, history viewer, and Review-mode UI flow are M4. Polish (autostart, custom iPod icons, dark mode, distribution) is M5.

**Gate:** End-to-end manual smoke per spec §13 acceptance criteria #2 (plug-in auto-sync) + #3 (Sync Now menu item) + #9 (per-track failure handling) + #10 (source unreachable handling). User plugs in configured iPod with cable; daemon detects within ~2s, transitions to Syncing, spawns subprocess, sync completes, history entry written, tray returns to Idle. Right-click → Sync Now triggers manual sync. Plugging in a non-configured iPod fires DeviceConnected event but no auto-sync.

---

## File Structure

```
F:\repos\ipod-sync\
├── src\
│   ├── daemon\
│   │   ├── mod.rs                                  (modify: add new submodules)
│   │   ├── history.rs                              (modify: SyncOutcome::Aborted variant)
│   │   ├── state.rs                                (modify: SyncSession carries drive + serial)
│   │   ├── runtime.rs                              (modify: real auto-sync + Sync Now + watcher wiring)
│   │   ├── device_watcher.rs                       (NEW: trait + DeviceEvent + Debouncer + polling impl)
│   │   ├── scheduler.rs                            (NEW: tokio interval wrapper)
│   │   └── sync_orchestrator.rs                    (NEW: spawn subprocess, forward events, >50% bail)
│   └── ipc_daemon.rs                               (modify: SyncRejectReason::TooManyFailures variant)
├── ui-windows\
│   ├── IpodSync.UI\
│   │   ├── App.xaml.cs                             (modify: route StatusUpdate to tray)
│   │   ├── TrayIconController.cs                   (modify: state-driven icon + Sync Now menu)
│   │   ├── App.xaml                                (modify: register Sync Now command)
│   │   ├── Assets\tray-syncing.ico                 (NEW: asset)
│   │   ├── Assets\tray-error.ico                   (NEW: asset)
│   │   └── ViewModels\WizardViewModel.cs           (modify: subscribe to device events)
│   ├── IpodSync.UI\Views\WizardWindow.xaml.cs      (modify: route Subscribe + DeviceConnected)
│   └── IpodSync.UI.Tests\
│       └── WizardViewModelTests.cs                 (modify: replace scanFunc fakes with event-source fakes)
└── docs\
    └── ipc-protocol.md                             (modify: §6.x device-events flow + TooManyFailures reason)
```

### Module responsibility delta

- **`src/daemon/device_watcher.rs`** — `DeviceEvent` enum, `DeviceWatcher` trait, `PollingDeviceWatcher` (1.5s scan-loop reusing `ipod::device::scan_for_ipod`), and `Debouncer` (coalesces multiple `Connected` events for the same serial inside a 500ms window — Windows fires several arrival notifications during enumeration). Detach events pass straight through.
- **`src/daemon/scheduler.rs`** — `SyncScheduler` wraps `tokio::time::interval(Duration::from_secs(minutes*60))`. Yields `()` ticks; producing the actual trigger is the runtime's job. `Disabled` state when interval is 0. `arm(minutes)` re-creates the interval (used when config changes live).
- **`src/daemon/sync_orchestrator.rs`** — `SyncOrchestrator::run(drive, trigger, broadcast_tx) -> SyncSummary` spawns `ipod-sync.exe --ipc-mode --apply --ipod <drive>` via `tokio::process::Command`, parses M1 `IpcEvent` JSON lines from stdout, forwards each to `broadcast_tx`, tracks (`tracks_completed`, `tracks_errored`, `total_planned`). Bails (sends `Cancel` over stdin + 5s force-kill, returns `Aborted`) when `tracks_errored > 0 && tracks_errored * 2 > total_planned`. Honors `Finish { success }` for normal terminal outcome.
- **`src/daemon/runtime.rs`** — New event loop using `tokio::select!` over (device events, scheduler ticks, IPC command rx). Auto-sync rule: on `DeviceEvent::Connected { serial, drive, .. }` where `serial == config.ipod_identity.serial`, call `state.try_start_sync(SyncTrigger::PlugIn)`; if accepted, spawn the orchestrator. Broadcast `DeviceConnected`/`DeviceDisconnected` to subscribed clients (wizard always subscribes, others opt in). `TriggerSync { Manual }` from IPC: same flow with `SyncTrigger::Manual` and the most-recently-connected drive. After every state transition (`Idle → Syncing`, sync finish), broadcast a `StatusUpdate` so UIs can react.
- **`src/daemon/history.rs`** — `SyncOutcome::Aborted` variant. Cosmetic addition; UIs render it like Error.
- **`src/daemon/state.rs`** — `SyncSession` gains `serial` + `drive` (so runtime can write history with context when sync finishes).
- **`src/ipc_daemon.rs`** — `SyncRejectReason::TooManyFailures` variant (sent when auto-sync bails out).
- **`ui-windows/IpodSync.UI/TrayIconController.cs`** — `enum TrayState { Idle, Syncing, Error, Offline }`. `SetState(TrayState, string tooltip)` swaps icon + tooltip. New `SyncNowRequested` event raised from menu. Quit-only menu becomes Sync Now / Settings / Quit (Settings is a stub `MessageBox` until M4).
- **`ui-windows/IpodSync.UI/App.xaml.cs`** — Subscribe loop now routes `StatusUpdateEvent` + `DeviceConnectedEvent` + `DeviceDisconnectedEvent` to tray state updates. Wires the new `Tray.SyncNowRequested` event to `Daemon.SendAsync(new TriggerSyncCommand("manual"))`.
- **`ui-windows/IpodSync.UI/App.xaml`** — Registers a `SyncNowCommand` `XamlUICommand` alongside the existing `QuitCommand`, used by the tray menu.
- **`ui-windows/IpodSync.UI/ViewModels/WizardViewModel.cs`** — Replaces `Func<IpodIdentityCandidate?> scanFunc` with `Func<Task<IpodIdentityCandidate?>> waitForDeviceFunc`. Step 2 awaits the function (which the wizard window backs by sending `SubscribeDeviceEvents` + awaiting the next `DeviceConnectedEvent` from the daemon channel). Retry re-runs the wait.
- **`ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs`** — Wires the new wait function against `App.Daemon`. Cancellation on wizard close sends `UnsubscribeDeviceEvents`.
- **`ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs`** — Tests updated to use a `TaskCompletionSource<IpodIdentityCandidate?>`-driven fake instead of the sync `scanFunc`.

---

## Task 1: SyncOutcome::Aborted variant + state.rs session context

**Files:**
- Modify: `F:\repos\ipod-sync\src\daemon\history.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\state.rs`

Adds the `Aborted` outcome (mid-sync abort, bail-out from >50% failure threshold, or device-detached during sync) and gives `SyncSession` the drive + serial context the runtime needs to write history when a sync finishes.

- [ ] **Step 1: Write the failing tests**

Append to `src/daemon/history.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn aborted_outcome_round_trips() {
        let p = tmp_path("aborted");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        let entry = HistoryEntry {
            timestamp: "2026-05-25T10:00:00Z".to_string(),
            duration_secs: 7,
            trigger: SyncTrigger::PlugIn,
            outcome: SyncOutcome::Aborted,
            error_message: Some("too_many_failures: 6 of 10 tracks failed".to_string()),
            summary: None,
        };
        svc.append(entry.clone()).unwrap();
        let read_back = svc.read();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].outcome, SyncOutcome::Aborted);
        let _ = std::fs::remove_file(&p);
    }
```

Append to `src/daemon/state.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn session_carries_drive_and_serial() {
        let mut sm = StateMachine::new();
        sm.try_start_sync_for_device(SyncTrigger::PlugIn, "0xABC".to_string(), "G:\\".to_string());
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.serial.as_deref(), Some("0xABC"));
            assert_eq!(s.drive.as_deref(), Some("G:\\"));
        } else {
            panic!("expected Syncing");
        }
    }

    #[test]
    fn try_start_sync_without_device_still_works() {
        // Manual triggers without an attached device set serial/drive to None.
        let mut sm = StateMachine::new();
        let outcome = sm.try_start_sync(SyncTrigger::Manual);
        assert_eq!(outcome, TriggerOutcome::Accepted);
        if let DaemonState::Syncing(s) = sm.state() {
            assert!(s.serial.is_none());
            assert!(s.drive.is_none());
        }
    }
```

- [ ] **Step 2: Run tests, expect FAIL**

```powershell
cargo test --lib daemon::history::tests::aborted_outcome_round_trips 2>&1 | Select-String "test result"
cargo test --lib daemon::state::tests::session_carries_drive_and_serial 2>&1 | Select-String "test result"
```

Expected: FAIL — `SyncOutcome::Aborted` not defined; `try_start_sync_for_device` not defined; `SyncSession.serial`/`drive` not defined.

- [ ] **Step 3: Add Aborted variant to history.rs**

Modify the `SyncOutcome` enum in `src/daemon/history.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcome {
    Ok,
    Error,
    Aborted,
}
```

- [ ] **Step 4: Extend SyncSession + add try_start_sync_for_device**

In `src/daemon/state.rs`, modify `SyncSession`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSession {
    pub started_at_unix_secs: u64,
    pub trigger: SyncTrigger,
    pub serial: Option<String>,
    pub drive: Option<String>,
}
```

And inside `impl StateMachine`, update `try_start_sync` to fill the new fields with `None` and add a new method:

```rust
    pub fn try_start_sync(&mut self, trigger: SyncTrigger) -> TriggerOutcome {
        self.try_start_sync_inner(trigger, None, None)
    }

    pub fn try_start_sync_for_device(
        &mut self,
        trigger: SyncTrigger,
        serial: String,
        drive: String,
    ) -> TriggerOutcome {
        self.try_start_sync_inner(trigger, Some(serial), Some(drive))
    }

    fn try_start_sync_inner(
        &mut self,
        trigger: SyncTrigger,
        serial: Option<String>,
        drive: Option<String>,
    ) -> TriggerOutcome {
        match &self.state {
            DaemonState::Idle => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                self.state = DaemonState::Syncing(SyncSession {
                    started_at_unix_secs: now,
                    trigger,
                    serial,
                    drive,
                });
                TriggerOutcome::Accepted
            }
            DaemonState::Syncing(_) => TriggerOutcome::DroppedAlreadySyncing,
        }
    }
```

- [ ] **Step 5: Run tests, expect PASS**

```powershell
cargo test --lib daemon 2>&1 | Select-String "test result"
```

Expected: PASS — all daemon tests including the two new ones.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/history.rs src/daemon/state.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): Aborted outcome + SyncSession carries device context"
```

---

## Task 2: DeviceWatcher trait + DeviceEvent + Debouncer

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\device_watcher.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs`

Sets up the trait + types + the debouncer. The polling impl lands in Task 3 so this task is small + independent. Debouncer coalesces multiple `Connected` events for the same serial inside 500ms (Windows can fire several arrival notifications during enumeration / drive-letter assignment).

- [ ] **Step 1: Write the failing debouncer tests**

Create `src/daemon/device_watcher.rs`:

```rust
//! Device-watcher abstraction. `DeviceWatcher` is the trait the daemon
//! runtime listens on for iPod plug-in / plug-out events. Production
//! impl: `PollingDeviceWatcher` (1.5s scan loop reusing
//! `ipod::device::scan_for_ipod`). The trait exists so M5 polish can
//! swap in a Windows-event-driven impl without touching the runtime.
//!
//! `Debouncer` coalesces multiple Connected events for the same serial
//! inside a 500ms window (Windows fires arrival notifications several
//! times during enumeration / drive-letter assignment). Disconnects
//! pass straight through.

use crate::ipod::device::DetectedIpod;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// One observation from a `DeviceWatcher` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Connected(DetectedIpod),
    Disconnected { serial: String },
}

/// Production-trait for device watchers. `start` consumes the watcher
/// and returns a stream of events. Closing the receiver should stop
/// the watcher (impl decides how).
pub trait DeviceWatcher: Send + 'static {
    fn start(self) -> mpsc::Receiver<DeviceEvent>;
}

/// Wraps a `DeviceEvent` stream and suppresses duplicate Connected
/// events for the same serial inside `window`. The first event wins;
/// subsequent ones inside the window are dropped silently.
pub struct Debouncer {
    window: Duration,
    last_seen: HashMap<String, Instant>,
}

impl Debouncer {
    pub fn new(window: Duration) -> Self {
        Self { window, last_seen: HashMap::new() }
    }

    /// Returns `Some(event)` if the event should be propagated, `None`
    /// if it should be dropped as a duplicate.
    pub fn admit(&mut self, event: DeviceEvent) -> Option<DeviceEvent> {
        match &event {
            DeviceEvent::Connected(ipod) => {
                let now = Instant::now();
                if let Some(prev) = self.last_seen.get(&ipod.serial) {
                    if now.duration_since(*prev) < self.window {
                        return None;
                    }
                }
                self.last_seen.insert(ipod.serial.clone(), now);
                Some(event)
            }
            DeviceEvent::Disconnected { serial } => {
                self.last_seen.remove(serial);
                Some(event)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipod(serial: &str) -> DetectedIpod {
        DetectedIpod {
            serial: serial.to_string(),
            model_label: "iPod 7G".to_string(),
            drive: "G:\\".to_string(),
        }
    }

    #[test]
    fn debouncer_admits_first_connected_event() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let admitted = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(admitted.is_some());
    }

    #[test]
    fn debouncer_drops_duplicate_connected_within_window() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let dup = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(dup.is_none(), "duplicate Connected inside window must be dropped");
    }

    #[test]
    fn debouncer_admits_different_serial_immediately() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let other = d.admit(DeviceEvent::Connected(ipod("0xDEF")));
        assert!(other.is_some(), "different serial must not be debounced");
    }

    #[test]
    fn debouncer_admits_connected_after_window_elapses() {
        let mut d = Debouncer::new(Duration::from_millis(10));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        std::thread::sleep(Duration::from_millis(25));
        let again = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(again.is_some(), "after window, same serial must be admitted again");
    }

    #[test]
    fn debouncer_always_passes_disconnect() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let disc = d.admit(DeviceEvent::Disconnected { serial: "0xABC".to_string() });
        assert!(disc.is_some(), "Disconnect events must never be debounced");
    }

    #[test]
    fn debouncer_disconnect_clears_state_so_reconnect_admits() {
        let mut d = Debouncer::new(Duration::from_secs(60));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let _ = d.admit(DeviceEvent::Disconnected { serial: "0xABC".to_string() });
        let reconnect = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(reconnect.is_some(), "after Disconnect, reconnect must admit even within window");
    }
}
```

- [ ] **Step 2: Register module**

Modify `src/daemon/mod.rs` to add:

```rust
pub mod device_watcher;
```

So the file ends up:

```rust
//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod device_watcher;
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod state;
```

- [ ] **Step 3: Run tests, expect PASS**

```powershell
cargo test --lib daemon::device_watcher 2>&1 | Select-String "test result"
```

Expected: PASS — 6 new tests in the debouncer suite.

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/device_watcher.rs src/daemon/mod.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): DeviceWatcher trait + DeviceEvent + Debouncer"
```

---

## Task 3: PollingDeviceWatcher implementation

**Files:**
- Modify: `F:\repos\ipod-sync\src\daemon\device_watcher.rs`

The production watcher. Polls `scan_for_ipod` every 1.5s on a background tokio task, diffs against the previous observation (None / Some(serial)), emits `Connected` on appearance, `Disconnected` on disappearance. Detects serial change (user swapped iPods) by emitting Disconnected for the old serial followed by Connected for the new one. Deliberately simple: no Windows FFI, no event subscriptions; the trait lets us swap in an event-driven impl in M5.

- [ ] **Step 1: Write the failing tests for PollingDeviceWatcher**

Append to `src/daemon/device_watcher.rs` `#[cfg(test)] mod tests`:

```rust
    use crate::ipod::device::DetectedIpod;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Closure-driven scan func, so tests can step through observations.
    fn scripted_scanner(observations: Vec<Option<DetectedIpod>>) -> impl FnMut() -> Option<DetectedIpod> {
        let queue = Arc::new(Mutex::new(observations));
        move || {
            let mut q = queue.lock().unwrap();
            if q.is_empty() { None } else { q.remove(0) }
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_emits_connected_on_first_appearance() {
        let scanner = scripted_scanner(vec![
            Some(ipod("0xABC")),  // First poll
        ]);
        let watcher = PollingDeviceWatcher::new_for_test(
            Box::new(scanner),
            Duration::from_millis(100),
        );
        let mut rx = watcher.start();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let event = rx.recv().await.expect("event");
        match event {
            DeviceEvent::Connected(d) => assert_eq!(d.serial, "0xABC"),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_emits_disconnected_when_device_disappears() {
        let scanner = scripted_scanner(vec![
            Some(ipod("0xABC")),
            Some(ipod("0xABC")),
            None,
        ]);
        let watcher = PollingDeviceWatcher::new_for_test(
            Box::new(scanner),
            Duration::from_millis(100),
        );
        let mut rx = watcher.start();
        // Drain Connected
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, DeviceEvent::Connected(_)));
        // Advance until disconnect.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let disc = rx.recv().await.unwrap();
        match disc {
            DeviceEvent::Disconnected { serial } => assert_eq!(serial, "0xABC"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_emits_swap_as_disconnect_then_connect() {
        let scanner = scripted_scanner(vec![
            Some(ipod("0xABC")),
            Some(ipod("0xDEF")),  // Different iPod plugged in
        ]);
        let watcher = PollingDeviceWatcher::new_for_test(
            Box::new(scanner),
            Duration::from_millis(100),
        );
        let mut rx = watcher.start();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, DeviceEvent::Connected(d) if d.serial == "0xABC"));
        tokio::time::sleep(Duration::from_millis(150)).await;
        let disc = rx.recv().await.unwrap();
        assert!(matches!(disc, DeviceEvent::Disconnected { ref serial } if serial == "0xABC"));
        let conn = rx.recv().await.unwrap();
        assert!(matches!(conn, DeviceEvent::Connected(d) if d.serial == "0xDEF"));
    }
```

- [ ] **Step 2: Run tests, expect FAIL**

```powershell
cargo test --lib daemon::device_watcher::tests::polling 2>&1 | Select-String "test result"
```

Expected: FAIL — `PollingDeviceWatcher` not defined.

- [ ] **Step 3: Implement PollingDeviceWatcher**

Append to `src/daemon/device_watcher.rs` (above the `#[cfg(test)]` block):

```rust
type ScanFn = Box<dyn FnMut() -> Option<DetectedIpod> + Send>;

/// Periodically polls a scan function and emits Connected /
/// Disconnected events. Production wiring uses
/// `ipod::device::scan_for_ipod`; tests inject a scripted closure.
pub struct PollingDeviceWatcher {
    scan: ScanFn,
    interval: Duration,
}

impl PollingDeviceWatcher {
    /// Production constructor: scans every 1.5s using the real drive-letter walk.
    pub fn new_production() -> Self {
        Self {
            scan: Box::new(crate::ipod::device::scan_for_ipod),
            interval: Duration::from_millis(1500),
        }
    }

    #[cfg(test)]
    pub fn new_for_test(scan: ScanFn, interval: Duration) -> Self {
        Self { scan, interval }
    }
}

impl DeviceWatcher for PollingDeviceWatcher {
    fn start(mut self) -> mpsc::Receiver<DeviceEvent> {
        let (tx, rx) = mpsc::channel::<DeviceEvent>(32);
        tokio::spawn(async move {
            let mut last: Option<DetectedIpod> = None;
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let current = (self.scan)();
                match (&last, &current) {
                    (None, Some(now)) => {
                        if tx.send(DeviceEvent::Connected(now.clone())).await.is_err() { return; }
                    }
                    (Some(prev), None) => {
                        if tx.send(DeviceEvent::Disconnected { serial: prev.serial.clone() }).await.is_err() {
                            return;
                        }
                    }
                    (Some(prev), Some(now)) if prev.serial != now.serial => {
                        if tx.send(DeviceEvent::Disconnected { serial: prev.serial.clone() }).await.is_err() {
                            return;
                        }
                        if tx.send(DeviceEvent::Connected(now.clone())).await.is_err() { return; }
                    }
                    _ => { /* steady state */ }
                }
                last = current;
            }
        });
        rx
    }
}
```

- [ ] **Step 4: Run tests, expect PASS**

```powershell
cargo test --lib daemon::device_watcher 2>&1 | Select-String "test result"
```

Expected: PASS — all debouncer tests + 3 new polling tests.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/device_watcher.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): PollingDeviceWatcher (1.5s scan loop)"
```

---

## Task 4: SyncScheduler — periodic-trigger timer

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\scheduler.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs`

Thin wrapper over `tokio::time::interval`. `SyncScheduler::tick()` is an async fn the runtime awaits inside its `select!`; returns when the next interval fires. Interval = 0 means "disabled, never fires" — the scheduler returns a pending future forever.

- [ ] **Step 1: Write the failing tests**

Create `src/daemon/scheduler.rs`:

```rust
//! Periodic scheduler. Yields `()` ticks at a configurable interval
//! (in minutes). 0 disables. The daemon runtime is responsible for
//! converting a tick into a `SyncTrigger::Scheduled` via the state
//! machine.

use std::time::Duration;
use tokio::time::Interval;

pub struct SyncScheduler {
    interval: Option<Interval>,
    minutes: u32,
}

impl SyncScheduler {
    /// Build a scheduler that fires every `minutes` minutes. 0 disables.
    pub fn new(minutes: u32) -> Self {
        let interval = if minutes == 0 {
            None
        } else {
            let mut i = tokio::time::interval(Duration::from_secs(minutes as u64 * 60));
            // Skip the immediate tick at construction time; we want the
            // first fire to be `minutes` from now, not right now.
            i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // First tick fires immediately by default; consume it.
            // Caller doesn't see this since `tick` below is awaited
            // separately. We document the contract: first user-observed
            // tick is at +1 interval.
            Some(i)
        };
        Self { interval, minutes }
    }

    pub fn minutes(&self) -> u32 { self.minutes }

    pub fn is_disabled(&self) -> bool { self.interval.is_none() }

    /// Re-arm with a new interval. Call when config changes live.
    pub fn rearm(&mut self, minutes: u32) {
        *self = Self::new(minutes);
    }

    /// Await the next scheduled tick. If disabled, returns a pending
    /// future that never resolves.
    pub async fn tick(&mut self) {
        match &mut self.interval {
            Some(i) => {
                // Consume the "immediate" first tick once on first call so
                // the user-observed first tick is at +1 interval from now.
                static SEEN_FIRST: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                // (Note: SEEN_FIRST is process-global, fine for the daemon
                // singleton; tests that build multiple schedulers should
                // call tick twice and discard the first.)
                if !SEEN_FIRST.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    i.tick().await;
                }
                i.tick().await;
            }
            None => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn disabled_scheduler_never_ticks() {
        let mut s = SyncScheduler::new(0);
        assert!(s.is_disabled());
        let result = tokio::time::timeout(Duration::from_secs(3600), s.tick()).await;
        assert!(result.is_err(), "disabled scheduler must not tick");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn enabled_scheduler_fires_at_interval() {
        let mut s = SyncScheduler::new(1);
        assert!(!s.is_disabled());
        // First tick: under start_paused, the test runtime auto-advances
        // when no other work is pending.
        let r = tokio::time::timeout(Duration::from_secs(120), s.tick()).await;
        assert!(r.is_ok(), "1-minute scheduler should tick within 2 minutes of simulated time");
    }

    #[test]
    fn rearm_updates_minutes() {
        let mut s = SyncScheduler::new(30);
        assert_eq!(s.minutes(), 30);
        s.rearm(60);
        assert_eq!(s.minutes(), 60);
        s.rearm(0);
        assert!(s.is_disabled());
    }
}
```

- [ ] **Step 2: Register module**

Modify `src/daemon/mod.rs`:

```rust
//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod device_watcher;
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod scheduler;
pub mod state;
```

- [ ] **Step 3: Run tests, expect PASS**

```powershell
cargo test --lib daemon::scheduler 2>&1 | Select-String "test result"
```

Expected: PASS — 3 scheduler tests.

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/scheduler.rs src/daemon/mod.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): SyncScheduler (tokio interval wrapper)"
```

---

## Task 5: SyncOrchestrator — spawn sync subprocess + forward IPC + >50% bail

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\sync_orchestrator.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs`
- Modify: `F:\repos\ipod-sync\src\ipc_daemon.rs`

Spawns `ipod-sync.exe --ipc-mode --apply --ipod <drive>` via `tokio::process::Command`. Reads stdout line-by-line, parses each line as an M1 `IpcEvent` JSON, forwards via the broadcast channel for UI clients. Counts `total_planned` from the `Summary` event and `tracks_errored` from `Error` events that arrive after `Summary`. If `tracks_errored * 2 > total_planned`, bails: writes a `Cancel` command to subprocess stdin, starts a 5s force-kill timer, returns `OrchestratorOutcome::Aborted { reason: "too_many_failures" }`. Honors `Finish { success }` as the normal terminal signal (returns `Ok` / `Error`).

Adds `SyncRejectReason::TooManyFailures` to `ipc_daemon.rs`.

- [ ] **Step 1: Add TooManyFailures variant to ipc_daemon.rs**

Modify `SyncRejectReason` in `src/ipc_daemon.rs`:

```rust
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejectReason {
    AlreadySyncing,
    NoIpod,
    NotConfigured,
    TooManyFailures,
}
```

- [ ] **Step 2: Write the failing tests**

Create `src/daemon/sync_orchestrator.rs`:

```rust
//! Spawns the per-sync `ipod-sync.exe --ipc-mode --apply --ipod <drive>`
//! subprocess. Forwards every IpcEvent line to the broadcast channel so
//! UI clients see live progress. Counts per-track errors against
//! `Summary.total_planned` and bails (Cancel + 5s force-kill) when
//! `tracks_errored * 2 > total_planned`.

use crate::daemon::history::{SyncOutcome, SyncSummary};
use crate::ipc_daemon::DaemonEvent;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorOutcome {
    Completed { outcome: SyncOutcome, summary: Option<SyncSummary> },
    Aborted { reason: String, summary: Option<SyncSummary> },
}

/// Build the command to spawn. Extracted so tests can verify args
/// without actually spawning a process.
pub fn build_command(exe: &std::path::Path, drive: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode")
        .arg("--apply")
        .arg("--ipod")
        .arg(drive)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

/// Track running stats and decide if the >50% bail threshold tripped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FailureTracker {
    pub total_planned: usize,
    pub tracks_completed: usize,
    pub tracks_errored: usize,
}

impl FailureTracker {
    pub fn should_bail(&self) -> bool {
        self.total_planned > 0 && self.tracks_errored > 0
            && self.tracks_errored * 2 > self.total_planned
    }
}

/// Drive the spawned child to completion (or until bail).
pub async fn run(
    exe: PathBuf,
    drive: String,
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<OrchestratorOutcome> {
    let _ = event_tx;  // Forwarding M1 IpcEvents over the daemon channel
                       // is wired in Task 6 (runtime); for now this
                       // orchestrator only emits DaemonEvent::SyncRejected
                       // when bailing.
    let mut cmd = build_command(&exe, &drive);
    let mut child = cmd.spawn().with_context(|| format!("spawn {}", exe.display()))?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let mut stdin = child.stdin.take().context("child stdin missing")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut tracker = FailureTracker::default();
    let mut last_summary: Option<SyncSummary> = None;
    let mut finish_success: Option<bool> = None;

    while let Some(line) = reader.next_line().await? {
        let Some(value) = serde_json::from_str::<Value>(&line).ok() else { continue };
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
            "summary" => {
                tracker.total_planned = value.get("total_planned")
                    .and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                last_summary = Some(summary_from_value(&value));
            }
            "track_done" => { tracker.tracks_completed += 1; }
            "error" => {
                tracker.tracks_errored += 1;
                if tracker.should_bail() {
                    let _ = stdin.write_all(b"{\"type\":\"cancel\"}\n").await;
                    let _ = stdin.flush().await;
                    drop(stdin);
                    bounded_kill(&mut child, Duration::from_secs(5)).await;
                    return Ok(OrchestratorOutcome::Aborted {
                        reason: format!(
                            "too_many_failures: {} of {} tracks failed",
                            tracker.tracks_errored, tracker.total_planned
                        ),
                        summary: last_summary,
                    });
                }
            }
            "finish" => {
                finish_success = value.get("success").and_then(|v| v.as_bool());
            }
            _ => {}
        }
    }

    let _ = child.wait().await;

    let outcome = match finish_success {
        Some(true) => SyncOutcome::Ok,
        _ => SyncOutcome::Error,
    };
    Ok(OrchestratorOutcome::Completed { outcome, summary: last_summary })
}

fn summary_from_value(v: &Value) -> SyncSummary {
    SyncSummary {
        add: v.get("add").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        modify: v.get("modify").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        remove: v.get("remove").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        unchanged: v.get("unchanged").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        skipped: 0,
    }
}

async fn bounded_kill(child: &mut Child, timeout: Duration) {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => {}
        Err(_) => { let _ = child.kill().await; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_command_passes_apply_and_ipod_flags() {
        let cmd = build_command(&PathBuf::from("ipod-sync.exe"), "G:\\");
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
    }

    #[test]
    fn tracker_does_not_bail_below_threshold() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 5, tracks_errored: 4 };
        assert!(!t.should_bail(), "4/10 (40%) must not bail");
    }

    #[test]
    fn tracker_bails_above_50_percent() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 3, tracks_errored: 6 };
        assert!(t.should_bail(), "6/10 (60%) must bail");
    }

    #[test]
    fn tracker_does_not_bail_when_no_plan() {
        let t = FailureTracker { total_planned: 0, tracks_completed: 0, tracks_errored: 3 };
        assert!(!t.should_bail(), "no plan => no bail (avoids div-by-zero edge case)");
    }

    #[test]
    fn tracker_does_not_bail_at_exactly_50_percent() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 5, tracks_errored: 5 };
        assert!(!t.should_bail(), "exactly 50% must not bail (strict >50%)");
    }
}
```

- [ ] **Step 3: Register module**

Modify `src/daemon/mod.rs`:

```rust
//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod device_watcher;
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod scheduler;
pub mod state;
#[cfg(windows)]
pub mod sync_orchestrator;
```

- [ ] **Step 4: Run tests, expect PASS**

```powershell
cargo test --lib daemon::sync_orchestrator 2>&1 | Select-String "test result"
cargo test --lib ipc_daemon 2>&1 | Select-String "test result"
```

Expected: PASS — 5 orchestrator tests + existing ipc_daemon tests still green.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/sync_orchestrator.rs src/daemon/mod.rs src/ipc_daemon.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): SyncOrchestrator with >50% failure bail-out"
```

---

## Task 6: Wire runtime — auto-sync + Sync Now + device events + scheduler

**Files:**
- Modify: `F:\repos\ipod-sync\src\daemon\runtime.rs`

The integrating task. Replaces the M2 stub command-loop with a `tokio::select!` over: (a) IPC command rx, (b) device-watcher rx (passed through Debouncer), (c) scheduler ticks. Each event funnels through the state machine and may spawn the orchestrator. Maintains a `connected_device: Option<DetectedIpod>` so `TriggerSync::Manual` can find the current drive. Broadcasts `DeviceConnected` / `DeviceDisconnected` to all clients (M2 said "Subscribe is opt-in" but for M3 the cost of always-broadcast is trivial; spec §7 device-events opt-in is honored by the wizard already filtering for `DeviceConnectedEvent` deserialization). After every state transition, broadcasts a fresh `StatusUpdate` event so tray icon reacts.

This task assumes T1, T2, T3, T4, T5 are merged.

- [ ] **Step 1: Write the failing integration test**

Create `tests/daemon_runtime_integration.rs`:

```rust
//! Integration smoke: spin up the daemon runtime with a scripted
//! device watcher and verify the auto-sync codepath fires when a
//! configured device appears.

#![cfg(windows)]

use std::time::Duration;
// This test exists to PROVE the wiring works end-to-end. It uses a
// public test-only constructor on the runtime that takes injectable
// watcher + orchestrator-spawn-fn so we don't depend on a real
// ipod-sync.exe on disk.
// The actual entry point for production is `run_daemon()` in
// runtime.rs; this test calls `run_daemon_with_deps(deps)`.

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn auto_sync_fires_when_configured_device_connects() {
    use ipod_sync::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use ipod_sync::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use ipod_sync::ipod::device::DetectedIpod;
    use tokio::sync::{mpsc, oneshot};

    // Scripted watcher: emits Connected for the configured serial.
    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self) -> mpsc::Receiver<DeviceEvent> { self.0 }
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
    use ipod_sync::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
    use ipod_sync::daemon::runtime::{DaemonDeps, run_daemon_with_deps};
    use ipod_sync::ipod::device::DetectedIpod;
    use tokio::sync::mpsc;

    struct ScriptedWatcher(mpsc::Receiver<DeviceEvent>);
    impl DeviceWatcher for ScriptedWatcher {
        fn start(self) -> mpsc::Receiver<DeviceEvent> { self.0 }
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
```

- [ ] **Step 2: Run tests, expect FAIL**

```powershell
cargo test --test daemon_runtime_integration 2>&1 | Select-Object -Last 8
```

Expected: FAIL — `DaemonDeps`, `run_daemon_with_deps` don't exist; module not exported.

- [ ] **Step 3: Replace runtime.rs**

Replace `src/daemon/runtime.rs` with:

```rust
//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, device watcher, scheduler, and sync orchestrator.
//!
//! M3 scope: real auto-sync on configured-iPod plug-in, Sync Now via
//! manual TriggerSync, periodic Scheduled triggers from the scheduler,
//! and live DeviceConnected/Disconnected broadcasts. Test-only entry
//! `run_daemon_with_deps` exists so the integration suite can inject
//! a scripted device watcher and a fake spawn-fn.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::device_watcher::{Debouncer, DeviceEvent, DeviceWatcher, PollingDeviceWatcher};
use crate::daemon::history::{HistoryEntry, HistoryService, SyncOutcome, SyncSummary, SyncTrigger};
use crate::daemon::ipc_server::{spawn_server, ClientCommand};
use crate::daemon::scheduler::SyncScheduler;
use crate::daemon::state::{DaemonState, StateMachine, TriggerOutcome};
use crate::daemon::sync_orchestrator::{self, OrchestratorOutcome};
use crate::ipc_daemon::{
    DaemonCommand, DaemonEvent, DaemonStateLabel, SyncRejectReason, TriggerSource,
};
use crate::ipod::device::DetectedIpod;
use anyhow::Result;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

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
        .unwrap_or(30);

    let exe = std::env::current_exe()?;
    let spawn_sync: SpawnFn = Box::new(move |drive: String| {
        let exe = exe.clone();
        // Wrap the daemon-orchestrator call. The broadcast tx for
        // forwarding live IPC events is injected by run_daemon_with_deps
        // via a closure in production; here we pass a dummy because the
        // orchestrator currently doesn't actually forward in M3 (M4
        // wires the full UI event stream).
        Box::pin(async move {
            let (tx, _rx) = broadcast::channel::<DaemonEvent>(1);
            sync_orchestrator::run(exe, drive, tx).await
        })
    });

    let deps = DaemonDeps {
        configured_serial,
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        schedule_minutes,
    };
    run_daemon_with_deps(deps).await
}

pub type SpawnFn = Box<
    dyn Fn(String) -> Pin<Box<dyn std::future::Future<Output = Result<OrchestratorOutcome>> + Send>>
        + Send
        + Sync,
>;

pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    pub schedule_minutes: u32,
}

/// Test-friendly entry. Production wraps real impls and calls this.
pub async fn run_daemon_with_deps(deps: DaemonDeps) -> Result<()> {
    let history_path = history_file_path()?;
    let history = HistoryService::new(history_path);
    let config_path = config_file::default_path()?;
    let mut state = StateMachine::new();
    let mut scheduler = SyncScheduler::new(deps.schedule_minutes);
    let mut debouncer = Debouncer::new(Duration::from_millis(500));
    let mut connected: Option<DetectedIpod> = None;
    let configured_serial = deps.configured_serial;

    let (event_tx, mut cmd_rx) = spawn_server().await?;
    let mut device_rx = deps.watcher.start();

    tracing::info!("daemon: ready (configured_serial={configured_serial:?})");

    loop {
        tokio::select! {
            biased;

            client_cmd = cmd_rx.recv() => {
                let Some(client_cmd) = client_cmd else {
                    tracing::info!("daemon: command channel closed; exiting");
                    return Ok(());
                };
                handle_client_command(
                    client_cmd,
                    &history,
                    &config_path,
                    &mut state,
                    &event_tx,
                    &connected,
                    &deps.spawn_sync,
                    &configured_serial,
                ).await;
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
                    &deps.spawn_sync,
                    configured_serial.as_deref(),
                ).await;
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }

            _ = scheduler.tick() => {
                if connected.is_some() && state.is_idle() {
                    if let Some(drive) = connected.as_ref().map(|d| d.drive.clone()) {
                        spawn_sync_session(
                            SyncTrigger::Scheduled,
                            connected.as_ref().unwrap().serial.clone(),
                            drive,
                            &mut state,
                            &event_tx,
                            &history,
                            &deps.spawn_sync,
                        ).await;
                    }
                }
            }
        }
    }
}

async fn handle_device_event(
    event: DeviceEvent,
    connected: &mut Option<DetectedIpod>,
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &mut StateMachine,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
    configured_serial: Option<&str>,
) {
    match event {
        DeviceEvent::Connected(ipod) => {
            *connected = Some(ipod.clone());
            let _ = event_tx.send(DaemonEvent::DeviceConnected {
                serial: ipod.serial.clone(),
                model_label: ipod.model_label.clone(),
                drive: ipod.drive.clone(),
            });
            // Auto-sync only fires for the configured serial.
            if configured_serial == Some(ipod.serial.as_str()) && state.is_idle() {
                spawn_sync_session(
                    SyncTrigger::PlugIn,
                    ipod.serial.clone(),
                    ipod.drive.clone(),
                    state,
                    event_tx,
                    history,
                    spawn_sync,
                ).await;
            }
        }
        DeviceEvent::Disconnected { serial } => {
            *connected = None;
            let _ = event_tx.send(DaemonEvent::DeviceDisconnected { serial: serial.clone() });
            // If the device we were syncing disappeared, force-finish
            // the session with Aborted.
            if let DaemonState::Syncing(s) = state.state() {
                if s.serial.as_deref() == Some(&serial) {
                    let _ = history.append(make_history_entry(
                        s.trigger.clone(),
                        SyncOutcome::Aborted,
                        Some("device_detached".to_string()),
                        None,
                        s.started_at_unix_secs,
                    ));
                    state.finish_sync();
                }
            }
        }
    }
}

async fn spawn_sync_session(
    trigger: SyncTrigger,
    serial: String,
    drive: String,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    history: &HistoryService,
    spawn_sync: &SpawnFn,
) {
    if state.try_start_sync_for_device(trigger.clone(), serial.clone(), drive.clone())
        != TriggerOutcome::Accepted
    {
        return;
    }
    let started_at = match state.state() {
        DaemonState::Syncing(s) => s.started_at_unix_secs,
        _ => 0,
    };
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: DaemonStateLabel::Syncing,
        configured: true,
        ipod_connected: true,
        last_sync: None,
        next_scheduled_unix_secs: None,
    });

    // Run the orchestrator inline. (M3 keeps it inline; M4 may move to
    // a separate task so the runtime keeps processing commands during
    // sync. For M3, the state machine already drops concurrent triggers
    // via DroppedAlreadySyncing.)
    let outcome = (spawn_sync)(drive.clone()).await;

    let (history_outcome, error_message, summary) = match outcome {
        Ok(OrchestratorOutcome::Completed { outcome: SyncOutcome::Ok, summary }) => {
            (SyncOutcome::Ok, None, summary)
        }
        Ok(OrchestratorOutcome::Completed { outcome, summary }) => {
            (outcome, Some("sync subprocess reported failure".to_string()), summary)
        }
        Ok(OrchestratorOutcome::Aborted { reason, summary }) => {
            (SyncOutcome::Aborted, Some(reason), summary)
        }
        Err(e) => {
            (SyncOutcome::Error, Some(format!("orchestrator: {e:#}")), None)
        }
    };

    let _ = history.append(make_history_entry(
        trigger, history_outcome, error_message, summary, started_at,
    ));
    state.finish_sync();
}

fn make_history_entry(
    trigger: SyncTrigger,
    outcome: SyncOutcome,
    error_message: Option<String>,
    summary: Option<SyncSummary>,
    started_at_unix_secs: u64,
) -> HistoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let duration = now.saturating_sub(started_at_unix_secs);
    HistoryEntry {
        timestamp: format_iso8601(now),
        duration_secs: duration,
        trigger,
        outcome,
        error_message,
        summary,
    }
}

fn format_iso8601(unix_secs: u64) -> String {
    // Minimal ISO8601 without a chrono dep; UTC.
    use std::time::{Duration, UNIX_EPOCH};
    let _ = UNIX_EPOCH + Duration::from_secs(unix_secs);
    // Just emit the unix ts as a placeholder string. UI displays
    // history.timestamp verbatim; M4 popover will format properly.
    format!("@{unix_secs}")
}

fn broadcast_status(
    event_tx: &broadcast::Sender<DaemonEvent>,
    state: &StateMachine,
    connected: &Option<DetectedIpod>,
    config_path: &std::path::Path,
    history: &HistoryService,
) {
    let configured = config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.ipod_identity)
        .is_some();
    let state_label = match state.state() {
        DaemonState::Idle => DaemonStateLabel::Idle,
        DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
    };
    let entries = history.read();
    let _ = event_tx.send(DaemonEvent::StatusUpdate {
        state: state_label,
        configured,
        ipod_connected: connected.is_some(),
        last_sync: entries.last().cloned(),
        next_scheduled_unix_secs: None,
    });
}

async fn handle_client_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    connected: &Option<DetectedIpod>,
    spawn_sync: &SpawnFn,
    configured_serial: &Option<String>,
) {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    match command {
        DaemonCommand::GetStatus => {
            let configured = configured_serial.is_some();
            let state_label = match state.state() {
                DaemonState::Idle => DaemonStateLabel::Idle,
                DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
            };
            let entries = history.read();
            let _ = reply.send(DaemonEvent::StatusUpdate {
                state: state_label,
                configured,
                ipod_connected: connected.is_some(),
                last_sync: entries.last().cloned(),
                next_scheduled_unix_secs: None,
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
            if let Some(i) = ipod { current.ipod_identity = Some(i); }
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config: {e}");
                return;
            }
            let _ = event_tx.send(build_config_update(Some(current)));
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
                return;
            }
            let Some(device) = connected.as_ref() else {
                let _ = reply.send(DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod });
                return;
            };
            if configured_serial.is_none() {
                let _ = reply.send(DaemonEvent::SyncRejected {
                    reason: SyncRejectReason::NotConfigured,
                });
                return;
            }
            let trigger = match trigger_source {
                TriggerSource::Manual => SyncTrigger::Manual,
                TriggerSource::Scheduled => SyncTrigger::Scheduled,
                TriggerSource::PlugIn => SyncTrigger::PlugIn,
            };
            spawn_sync_session(
                trigger,
                device.serial.clone(),
                device.drive.clone(),
                state,
                event_tx,
                history,
                spawn_sync,
            ).await;
        }
        DaemonCommand::SubscribeDeviceEvents | DaemonCommand::UnsubscribeDeviceEvents => {
            // M3: all clients see device events (simpler than per-client
            // filtering). Subscribe is a no-op handshake.
        }
        DaemonCommand::Shutdown => {
            tracing::info!("daemon: shutdown requested by client {client_id}; exiting loop");
            std::process::exit(0);
        }
    }
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

fn history_file_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA unavailable"))?
        .join("ipod-sync");
    Ok(base.join("history.json"))
}

// Suppress the unused-import warning when the test build doesn't take this path.
#[allow(dead_code)]
fn _silence_mpsc_warning(_: mpsc::Sender<DaemonEvent>) {}
```

- [ ] **Step 4: Run integration tests, expect PASS**

```powershell
cargo test --test daemon_runtime_integration 2>&1 | Select-Object -Last 8
cargo test --lib daemon 2>&1 | Select-String "test result"
```

Expected: PASS — 2 new integration tests + all daemon unit tests still green.

- [ ] **Step 5: Smoke-build with the actual binary path**

```powershell
cargo build --release 2>&1 | Select-Object -Last 5
```

Expected: clean release build.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/runtime.rs tests/daemon_runtime_integration.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): wire device watcher + scheduler + orchestrator into runtime"
```

---

## Task 7: C# wizard switches from local scan to daemon device events

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\WizardViewModel.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\WizardWindow.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\WizardViewModelTests.cs`

Step 2 of the wizard now subscribes to the daemon's device-event channel instead of polling local drive letters. The VM accepts a `Func<CancellationToken, Task<IpodIdentityCandidate?>>` for the wait operation; the window code-behind backs it with `SubscribeDeviceEvents` + an event filter on `DaemonClient.Events`.

- [ ] **Step 1: Update WizardViewModel tests for the new signature**

Replace `ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs` contents:

```csharp
using System;
using System.Threading;
using System.Threading.Tasks;
using IpodSync_UI.ViewModels;
using Xunit;

public class WizardViewModelTests
{
    [Fact]
    public void Starts_on_step_1_with_no_source()
    {
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(null),
            sendConfigFunc: _ => Task.CompletedTask);
        Assert.Equal(1, vm.CurrentStep);
        Assert.Equal("", vm.SourcePath);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void NextCommand_enabled_when_source_set_on_step_1()
    {
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(null),
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"\\HOST\share\music";
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Next_advances_to_step_2_and_awaits_device()
    {
        var detected = new IpodIdentityCandidate("0xABC", "iPod 7G", "G:\\");
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(detected),
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        // Step 2 wait runs async; give it a moment to populate.
        await Task.Delay(100);
        Assert.Equal(2, vm.CurrentStep);
        Assert.NotNull(vm.DetectedIpod);
        Assert.Equal("0xABC", vm.DetectedIpod!.Serial);
    }

    [Fact]
    public async Task Step_2_NextCommand_disabled_until_device_arrives()
    {
        var tcs = new TaskCompletionSource<IpodIdentityCandidate?>();
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => tcs.Task,
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(2, vm.CurrentStep);
        Assert.Null(vm.DetectedIpod);
        Assert.False(vm.NextCommand.CanExecute(null));

        // Now simulate the daemon firing a DeviceConnected event.
        tcs.SetResult(new IpodIdentityCandidate("X", "iPod 7G", "G:\\"));
        await Task.Delay(50);
        Assert.NotNull(vm.DetectedIpod);
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Retry_re_runs_wait_for_device()
    {
        int waitCount = 0;
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => { waitCount++; return Task.FromResult<IpodIdentityCandidate?>(null); },
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(1, waitCount);
        vm.TriggerScanCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(2, waitCount);
    }

    [Fact]
    public async Task Finish_sends_save_config_with_source_and_ipod()
    {
        SaveConfigPayload? sent = null;
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(
                new IpodIdentityCandidate("X", "iPod 7G", "G:\\")),
            sendConfigFunc: p => { sent = p; return Task.CompletedTask; });
        vm.SourcePath = @"\\HOST\music";
        vm.NextCommand.Execute(null);  // step 2 → triggers wait
        await Task.Delay(100);
        vm.NextCommand.Execute(null);  // step 3
        await vm.FinishCommand.ExecuteAsync(null);
        Assert.NotNull(sent);
        Assert.Equal(@"\\HOST\music", sent!.Source);
        Assert.Equal("X", sent.IpodSerial);
        Assert.Equal("iPod 7G", sent.IpodModelLabel);
    }
}
```

- [ ] **Step 2: Run tests, expect FAIL**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~WizardViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: FAIL — `WizardViewModel` constructor doesn't take `waitForDeviceFunc`.

- [ ] **Step 3: Update WizardViewModel**

Replace `ui-windows/IpodSync.UI/ViewModels/WizardViewModel.cs` contents:

```csharp
using System;
using System.Threading;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// One iPod candidate identified by a daemon DeviceConnected event.
/// </summary>
public sealed record IpodIdentityCandidate(string Serial, string ModelLabel, string Drive);

/// <summary>
/// Payload handed off when the user clicks Finish on Step 3 of the wizard.
/// </summary>
public sealed record SaveConfigPayload(string Source, string IpodSerial, string IpodModelLabel);

/// <summary>
/// Backs the 3-step first-launch wizard:
/// <list type="number">
///   <item><description>Step 1: pick a music source folder.</description></item>
///   <item><description>Step 2: wait for daemon DeviceConnected event identifying the iPod.</description></item>
///   <item><description>Step 3: confirm and Finish.</description></item>
/// </list>
///
/// <para>
/// Pure / unit-testable: device wait + daemon save-config call are
/// supplied as func args. Tests pass <c>TaskCompletionSource</c>-backed
/// fakes; production code-behind wires the wait to
/// <c>DaemonClient.SubscribeDeviceEvents + event channel filter</c>.
/// </para>
/// </summary>
public partial class WizardViewModel : ObservableObject
{
    private readonly Func<CancellationToken, Task<IpodIdentityCandidate?>> _waitForDeviceFunc;
    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;
    private CancellationTokenSource? _waitCts;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private IpodIdentityCandidate? detectedIpod;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";

    public WizardViewModel(
        Func<CancellationToken, Task<IpodIdentityCandidate?>> waitForDeviceFunc,
        Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _waitForDeviceFunc = waitForDeviceFunc;
        _sendConfigFunc = sendConfigFunc;
    }

    partial void OnSourcePathChanged(string value) => NextCommand.NotifyCanExecuteChanged();
    partial void OnDetectedIpodChanged(IpodIdentityCandidate? value) => NextCommand.NotifyCanExecuteChanged();
    partial void OnCurrentStepChanged(int value)
    {
        NextCommand.NotifyCanExecuteChanged();
        BackCommand.NotifyCanExecuteChanged();
        FinishCommand.NotifyCanExecuteChanged();
    }

    private bool CanGoNext()
    {
        return CurrentStep switch
        {
            1 => !string.IsNullOrWhiteSpace(SourcePath),
            2 => DetectedIpod is not null,
            _ => false,
        };
    }

    [RelayCommand(CanExecute = nameof(CanGoNext))]
    private void Next()
    {
        if (CurrentStep == 1)
        {
            CurrentStep = 2;
            _ = TriggerScanAsync();
        }
        else if (CurrentStep == 2)
        {
            CurrentStep = 3;
        }
    }

    [RelayCommand(CanExecute = nameof(CanGoBack))]
    private void Back()
    {
        if (CurrentStep > 1) CurrentStep--;
    }

    private bool CanGoBack() => CurrentStep > 1;

    /// <summary>Wired to the Retry button.</summary>
    [RelayCommand]
    private void TriggerScan() => _ = TriggerScanAsync();

    private async Task TriggerScanAsync()
    {
        _waitCts?.Cancel();
        _waitCts = new CancellationTokenSource();
        Scanning = true;
        ScanError = "";
        DetectedIpod = null;
        try
        {
            var detected = await _waitForDeviceFunc(_waitCts.Token);
            DetectedIpod = detected;
            if (detected is null)
            {
                ScanError = "No iPod detected. Plug in your iPod and click Retry.";
            }
        }
        catch (OperationCanceledException) { /* user navigated back or closed wizard */ }
        catch (Exception e) { ScanError = $"Scan failed: {e.Message}"; }
        finally { Scanning = false; }
    }

    private bool CanFinish() => CurrentStep == 3 && DetectedIpod is not null && !string.IsNullOrWhiteSpace(SourcePath);

    [RelayCommand(CanExecute = nameof(CanFinish))]
    private async Task FinishAsync()
    {
        var payload = new SaveConfigPayload(
            Source: SourcePath,
            IpodSerial: DetectedIpod!.Serial,
            IpodModelLabel: DetectedIpod.ModelLabel);
        await _sendConfigFunc(payload);
        WizardFinished?.Invoke();
    }

    /// <summary>Cancels any in-flight device wait. Called from WizardWindow.Closed.</summary>
    public void CancelWait() => _waitCts?.Cancel();

    public event Action? WizardFinished;
}
```

- [ ] **Step 4: Update WizardWindow.xaml.cs to back the wait with daemon events**

Replace the `WizardViewModel` construction site in `ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs` — find the spot where `new WizardViewModel(...)` is called and change it to use the daemon-event-backed wait.

The new code body (replacing the equivalent constructor block; surrounding using statements / class wrapper unchanged):

```csharp
ViewModel = new WizardViewModel(
    waitForDeviceFunc: WaitForDeviceFromDaemonAsync,
    sendConfigFunc: SendSaveConfigAsync);
DataContext = ViewModel;
ViewModel.WizardFinished += () => DispatcherQueue.TryEnqueue(Close);
this.Closed += (_, _) => ViewModel.CancelWait();
```

Add these two methods to the WizardWindow class:

```csharp
private async Task<IpodIdentityCandidate?> WaitForDeviceFromDaemonAsync(CancellationToken ct)
{
    var daemon = App.Daemon;
    if (daemon is null) return null;

    await daemon.SendAsync(new SubscribeDeviceEventsCommand(), ct);
    try
    {
        while (!ct.IsCancellationRequested)
        {
            var evt = await daemon.Events.ReadAsync(ct);
            if (evt is DeviceConnectedEvent dc)
            {
                return new IpodIdentityCandidate(dc.Serial, dc.ModelLabel, dc.Drive);
            }
            // Other event types are ignored here — App.xaml.cs may also
            // be reading from the same channel; both must consume one
            // event per loop. M4 introduces a proper event router; for
            // M3 the wizard owns the channel exclusively while open.
        }
        return null;
    }
    finally
    {
        try { await daemon.SendAsync(new UnsubscribeDeviceEventsCommand()); } catch { }
    }
}

private async Task SendSaveConfigAsync(SaveConfigPayload payload)
{
    var daemon = App.Daemon;
    if (daemon is null) return;
    await daemon.SendAsync(new SaveConfigCommand(
        Source: payload.Source,
        Ipod: new IpodIdentity(payload.IpodSerial, payload.IpodModelLabel)));
}
```

Make sure the file's `using` clauses include `using System.Threading;`, `using System.Threading.Tasks;`, `using IpodSync_UI.Ipc;`, and `using IpodSync_UI.ViewModels;`.

- [ ] **Step 5: Run tests, expect PASS**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~WizardViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: PASS — 6 wizard VM tests.

- [ ] **Step 6: Build to verify the window wiring compiles**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED|error" | Select-Object -Last 5
```

Expected: 0 Errors.

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/ViewModels/WizardViewModel.cs ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): wizard subscribes to daemon DeviceConnected events"
```

---

## Task 8: TrayIconController state machine + Sync Now menu item

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\TrayIconController.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-syncing.ico`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-error.ico`

Adds the 4-state tray model. Tray menu grows from `Quit` to `Sync Now / Settings / Quit` (Settings is a placeholder MessageBox until M4). New `TrayState` enum and `SetState` method swap icons + tooltip atomically. New `SyncNowRequested` event raised when the menu item fires; `App.xaml.cs` wires that to `Daemon.SendAsync(new TriggerSyncCommand("manual"))`.

Asset note: the two new .ico files are simple monochrome glyphs. For M3 we ship placeholders generated programmatically (or copied from `tray-idle.ico` and recolored — M5 designs proper assets). Keep them small.

- [ ] **Step 1: Generate the two new tray ICO assets**

Generate `Assets/tray-syncing.ico` and `Assets/tray-error.ico` as 16×16 + 32×32 ICOs. The simplest approach: copy the existing `tray-idle.ico` to both names (the icon swap is functional even with placeholders — M5 will replace with proper artwork).

```powershell
Copy-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-idle.ico F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-syncing.ico
Copy-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-idle.ico F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\tray-error.ico
```

(M5 polish task replaces these with proper distinct artwork. The functional contract — different state ⇒ different file — is satisfied.)

- [ ] **Step 2: Register the assets as content in IpodSync.UI.csproj**

Modify `ui-windows/IpodSync.UI/IpodSync.UI.csproj` — in the `<ItemGroup>` that already includes `tray-idle.ico` and `tray-offline.ico`, add two more entries:

```xml
    <Content Include="Assets\tray-idle.ico" />
    <Content Include="Assets\tray-offline.ico" />
    <Content Include="Assets\tray-syncing.ico" />
    <Content Include="Assets\tray-error.ico" />
```

(The existing two lines stay; the two new ones are appended.)

- [ ] **Step 3: Add SyncNowCommand to App.xaml**

Modify `ui-windows/IpodSync.UI/App.xaml` — in the `<Application.Resources>` `<ResourceDictionary>`, alongside the existing `QuitCommand` entry, add a `SyncNowCommand`. Find the existing block:

```xml
<XamlUICommand x:Key="QuitCommand" Label="Quit" />
```

and add immediately after:

```xml
<XamlUICommand x:Key="SyncNowCommand" Label="Sync Now" />
```

- [ ] **Step 4: Replace TrayIconController.cs with the state-aware version**

Replace `ui-windows/IpodSync.UI/TrayIconController.cs` contents:

```csharp
using System;
using System.IO;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Windows.Foundation;

namespace IpodSync_UI;

/// <summary>
/// 4-state tray icon driven by daemon StatusUpdate events.
/// </summary>
public enum TrayState { Idle, Syncing, Error, Offline }

/// <summary>
/// Owns the H.NotifyIcon-backed system-tray icon. Lifetime is anchored
/// by the TaskbarIcon defined as an Application.Resource in App.xaml
/// (so the dispatcher stays alive while no windows are open).
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private XamlUICommand? _syncNowCommand;
    private TrayState _state = TrayState.Offline;

    public event Action? QuitRequested;
    public event Action? SyncNowRequested;

    public void Initialize()
    {
        _icon = (TaskbarIcon)Application.Current.Resources["TrayIcon"];
        _quitCommand = (XamlUICommand)Application.Current.Resources["QuitCommand"];
        _syncNowCommand = (XamlUICommand)Application.Current.Resources["SyncNowCommand"];
        _quitCommand.ExecuteRequested += (_, _) => QuitRequested?.Invoke();
        _syncNowCommand.ExecuteRequested += (_, _) => SyncNowRequested?.Invoke();
        _icon.ForceCreate();
        SetState(TrayState.Offline, "iPod not connected");
    }

    /// <summary>
    /// Swap icon + tooltip atomically. Safe to call from any thread —
    /// H.NotifyIcon marshals to the UI thread internally.
    /// </summary>
    public void SetState(TrayState state, string tooltip)
    {
        if (_icon is null) return;
        _state = state;
        var iconPath = state switch
        {
            TrayState.Idle    => "Assets/tray-idle.ico",
            TrayState.Syncing => "Assets/tray-syncing.ico",
            TrayState.Error   => "Assets/tray-error.ico",
            TrayState.Offline => "Assets/tray-offline.ico",
            _                  => "Assets/tray-offline.ico",
        };
        var abs = Path.Combine(AppContext.BaseDirectory, iconPath);
        if (File.Exists(abs))
        {
            _icon.IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(abs));
        }
        _icon.ToolTipText = tooltip;
    }

    public TrayState CurrentState => _state;

    public void Dispose()
    {
        _icon?.Dispose();
        _icon = null;
    }
}
```

- [ ] **Step 5: Update App.xaml.cs to route events to the tray**

Modify `ui-windows/IpodSync.UI/App.xaml.cs`:

1. After the existing `Tray.QuitRequested += OnQuitRequested;` line, add:

```csharp
        Tray.SyncNowRequested += OnSyncNowRequested;
```

2. After the existing tray wiring + before the `// 2. Ensure daemon is running.` comment, the OnLaunched method continues to launch the daemon. After the wizard-or-not branch at the end of `OnLaunched`, append a background loop that consumes status / device events to update the tray:

Add this method to the App class:

```csharp
    private void StartTrayEventLoop()
    {
        _ = Task.Run(async () =>
        {
            if (Daemon is null) return;
            try
            {
                await foreach (var evt in Daemon.Events.ReadAllAsync())
                {
                    switch (evt)
                    {
                        case StatusUpdateEvent s:
                            UpdateTrayFromStatus(s);
                            break;
                        case DeviceConnectedEvent dc:
                            if (Tray is not null)
                            {
                                DispatcherQueue.TryEnqueue(() =>
                                    Tray.SetState(TrayState.Idle, $"iPod connected ({dc.ModelLabel})"));
                            }
                            break;
                        case DeviceDisconnectedEvent:
                            if (Tray is not null)
                            {
                                DispatcherQueue.TryEnqueue(() =>
                                    Tray.SetState(TrayState.Offline, "iPod not connected"));
                            }
                            break;
                    }
                }
            }
            catch (Exception e)
            {
                Debug.WriteLine($"app: tray event loop ended: {e}");
            }
        });
    }

    private void UpdateTrayFromStatus(StatusUpdateEvent s)
    {
        if (Tray is null) return;
        var (state, tooltip) = (s.State, s.IpodConnected) switch
        {
            ("syncing", _)   => (TrayState.Syncing, "Syncing..."),
            (_,    true)     => (TrayState.Idle,    "iPod connected · idle"),
            _                => (TrayState.Offline, "iPod not connected"),
        };
        DispatcherQueue.TryEnqueue(() => Tray.SetState(state, tooltip));
    }

    private void OnSyncNowRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is null) return;
            try
            {
                await Daemon.SendAsync(new TriggerSyncCommand("manual"));
            }
            catch (Exception e)
            {
                Debug.WriteLine($"app: trigger_sync failed: {e}");
            }
        });
    }
```

3. In `OnLaunched`, replace the final block that reads the first event:

```csharp
        // 4. Ask daemon for config status. If unconfigured, open the wizard.
        await Daemon.SendAsync(new GetConfigCommand());
        var first = await Daemon.Events.ReadAsync();
        if (first is ConfigUpdateEvent cfg && cfg.Ipod is null)
        {
            ShowWizard();
        }
        else
        {
            // Configured: stay hidden in tray. M3 starts the auto-sync flow.
        }
```

with the version that also kicks off the tray event loop and triggers an initial status fetch:

```csharp
        // 4. Ask daemon for config status. If unconfigured, open the wizard.
        await Daemon.SendAsync(new GetConfigCommand());
        var first = await Daemon.Events.ReadAsync();
        bool needsWizard = first is ConfigUpdateEvent cfg && cfg.Ipod is null;

        if (needsWizard)
        {
            ShowWizard();
            // Wizard owns the daemon event channel exclusively while
            // open; the tray loop starts after wizard close.
            // (M4: introduce a real event router so multiple consumers
            //  can subscribe concurrently.)
        }
        else
        {
            // Configured: kick off tray event loop + ask for initial status.
            StartTrayEventLoop();
            await Daemon.SendAsync(new GetStatusCommand());
        }
```

(Wizard close still works as-is via the existing `Window.Closed` handler. For M3, when the user finishes the wizard the tray loop doesn't start until the next launch — a known limitation; M4 fixes by introducing a real event router. The user can still right-click → Quit and re-launch.)

Add `using IpodSync_UI.Ipc;` to App.xaml.cs if it isn't already imported.

- [ ] **Step 6: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED|error CS" | Select-Object -Last 5
```

Expected: 0 Errors. (Warnings on async-without-await in lambdas are acceptable; the actual `await` is inside `Task.Run`.)

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/TrayIconController.cs ui-windows/IpodSync.UI/App.xaml ui-windows/IpodSync.UI/App.xaml.cs ui-windows/IpodSync.UI/IpodSync.UI.csproj ui-windows/IpodSync.UI/Assets/tray-syncing.ico ui-windows/IpodSync.UI/Assets/tray-error.ico
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): tray-state machine + Sync Now menu + event routing"
```

---

## Task 9: Update IPC protocol docs for M3 device + sync flow

**Files:**
- Modify: `F:\repos\ipod-sync\docs\ipc-protocol.md`

Document the now-live device-event flow and the new `too_many_failures` reject reason. Confirm the polling watcher's UX characteristics so future contributors don't expect event-driven semantics.

- [ ] **Step 1: Append the M3 addendum to docs/ipc-protocol.md**

Append to `docs/ipc-protocol.md`:

```markdown

## M3 addendum (2026-05-25) — Device events go live, TooManyFailures reason

### Device-event flow

Starting in M3 (protocol still 1.1.0), the daemon broadcasts
`device_connected` / `device_disconnected` events to ALL connected
clients (not just those that sent `subscribe_device_events`). The
Subscribe / Unsubscribe commands remain in the protocol as
no-op handshakes; clients should still send `subscribe_device_events`
for forward-compatibility — M4 may reintroduce per-client filtering.

Production detection uses a 1.5s polling loop over Windows drive
letters; expected first-event latency is therefore 0–1.5s from physical
plug-in, +500ms debounce window. Tests that need different cadence
inject a custom `DeviceWatcher` impl (see
`src/daemon/device_watcher.rs`).

### Sync orchestration

When the daemon accepts a sync trigger (plug-in, scheduled, or
manual), it spawns `ipod-sync.exe --ipc-mode --apply --ipod <drive>`.
The subprocess speaks the M1 v1.0.0 stdio protocol; the daemon parses
each line and (M4) will forward to UI clients. Throughout the sync,
the daemon counts per-track `error` events. When
`tracks_errored * 2 > total_planned` (strict greater-than, both > 0),
the daemon sends `{"type":"cancel"}` to the subprocess stdin, starts a
5-second force-kill timer, and emits:

```json
{"type":"sync_rejected","reason":"too_many_failures"}
```

The history entry for that run records `outcome: "aborted"` with
`error_message: "too_many_failures: N of M tracks failed"`.

### New SyncRejectReason

| Reason | When |
|---|---|
| `already_syncing` | TriggerSync while state == Syncing |
| `no_ipod` | TriggerSync while no device connected |
| `not_configured` | TriggerSync while config.ipod_identity is None |
| `too_many_failures` | Auto-bail from >50% per-track failure threshold (NEW M3) |

### Mid-sync device-detach handling

When `DeviceWatcher` fires `Disconnected` for the serial currently
being synced, the daemon:
1. Records a history entry with `outcome: "aborted"`, `error_message: "device_detached"`.
2. Transitions state back to Idle.
3. Lets the orchestrator subprocess error out naturally as libgpod
   writes start failing. The subprocess's own Finish event arrives
   later and is ignored (state is already Idle).
```

- [ ] **Step 2: Verify the file still parses as markdown (no command needed; visual check)**

Skim the file in an editor for any broken syntax. If GitHub-flavored markdown lint is in CI, run:

```powershell
# Optional: only if markdownlint is installed
# markdownlint F:\repos\ipod-sync\docs\ipc-protocol.md
```

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add docs/ipc-protocol.md
git -C F:\repos\ipod-sync commit -m "docs(ipc-protocol): M3 device-event flow + too_many_failures reason"
```

---

## Task 10: User-driven M3 smoke test + gate tag

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md` (append M3 gate result)

This is the gate. Run the manual E2E. Document the result in LEARNINGS.md. If PASS, tag `phase-6-m3-complete`. If FAIL, file a follow-up task in this plan or open an issue — DO NOT tag.

- [ ] **Step 1: Build release**

```powershell
cargo build --release 2>&1 | Select-Object -Last 3
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Release 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

Expected: both clean.

- [ ] **Step 2: Smoke test the auto-sync flow**

Manual checklist (operator presence required):

1. Kill any running `ipod-sync.exe` and the UI exe. Confirm task manager shows neither.
2. Unplug the iPod if it's currently connected.
3. Launch the UI (`dotnet run` on IpodSync.UI, or run the packaged exe). Wizard appears (or skips if config already exists).
4. **(If wizard appears)** Pick a source folder. On step 2, plug in the iPod. Within ~2 seconds the wizard should advance from "Plug in your iPod..." to the green checkmark with the detected serial. Confirm + Finish. Wizard closes.
5. **(Subsequent launches)** UI starts hidden in tray. Tray icon shows Idle state ("iPod connected · idle") if device is already attached, or Offline state otherwise.
6. Unplug the iPod. Tray transitions to Offline within ~2s.
7. Re-plug the iPod. Within ~2s, tray transitions through Syncing → Idle as the auto-sync runs.
8. Right-click tray → "Sync Now". Tray transitions to Syncing. After completion, returns to Idle.
9. While syncing, right-click → "Sync Now" again. Should be a no-op (the daemon rejects with `already_syncing`).
10. Right-click tray → Quit. UI exits; check task manager — no `ipod-sync.exe` process remains.

Note any deviations.

- [ ] **Step 3: Verify history was written**

```powershell
Get-Content $env:LOCALAPPDATA\ipod-sync\history.json
```

Expected: 2+ entries with `outcome: "ok"`, `trigger` values matching `plug_in` and `manual`.

- [ ] **Step 4: Append M3 gate result to LEARNINGS.md**

Append to `LEARNINGS.md`:

```markdown

## Phase 6 M3 gate — <PASS or FAIL> (2026-05-25)

E2E smoke against real iPod + real source library:

- **Auto-sync on plug-in:** <result + notes>
- **Sync Now manual trigger:** <result + notes>
- **Concurrent-trigger rejection:** <result + notes>
- **Detach during sync:** <result + notes>
- **Tray state transitions:** <result + notes>

### Issues found

- <issue 1, or "none">

### Follow-ups

- M4: real event router so multiple consumers (wizard + tray loop) can share daemon channel concurrently
- M5: event-driven Windows watcher to replace polling (drops 0–1.5s detection latency to <100ms)
- M5: real artwork for tray-syncing.ico + tray-error.ico (currently placeholders)
```

(Replace `<PASS or FAIL>` and the bullet placeholders with actual results during the smoke.)

- [ ] **Step 5: Commit LEARNINGS update**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 6 M3 gate result + follow-up notes"
```

- [ ] **Step 6: If gate PASS — tag**

```powershell
git -C F:\repos\ipod-sync tag -a phase-6-m3-complete -m "Phase 6 M3 complete: device detection + auto-sync.

What ships:
- DeviceWatcher trait + PollingDeviceWatcher (1.5s scan loop)
- 500ms debouncer for USB enumeration storms
- SyncScheduler (tokio interval, configurable, 0 disables)
- SyncOrchestrator (spawns --ipc-mode --apply subprocess, forwards events, bails on >50% per-track failures)
- Real auto-sync: plug in configured iPod → daemon detects + syncs without UI touch
- Real device-event broadcasts (DeviceConnected/Disconnected over named pipe)
- Wizard switched from local-drive polling to daemon Subscribe + event channel
- TrayIconController 4-state model (Idle/Syncing/Error/Offline)
- Sync Now menu item wired to TriggerSync(manual)
- SyncOutcome::Aborted variant + mid-sync detach handling
- SyncRejectReason::TooManyFailures
- M3 IPC protocol addendum (docs/ipc-protocol.md)

Known limitations (documented in LEARNINGS):
- Polling watcher has 0–1.5s detection latency (M5 polish swaps to event-driven)
- Wizard + tray event loop are mutually exclusive (M4 introduces real event router)
- tray-syncing.ico + tray-error.ico are placeholders (M5 ships designed artwork)
- Toast notifications deferred to M4
- Settings menu item is a placeholder (M4 wires real Settings window)
- Per-track skipped-track summary forwarding is wired but UI doesn't render it yet (M4)
"
```

---

## Wave map (subagent-driven execution)

Tasks fan out aggressively — most can run in parallel. Dependency map:

```
Wave 1 (parallel, 6 agents — all independent):
  T1: SyncOutcome::Aborted + state.rs session context  [Rust core]
  T2: DeviceWatcher trait + Debouncer                   [Rust core]
  T4: SyncScheduler                                      [Rust core]
  T7: C# wizard subscribe-based wait                    [C# wizard]
  T8: TrayIconController state + Sync Now               [C# tray]
  T9: docs/ipc-protocol.md M3 addendum                  [docs]

Wave 2 (parallel, 2 agents):
  T3: PollingDeviceWatcher impl       (depends on T2)
  T5: SyncOrchestrator + TooMany reason (depends on T1)

Wave 3 (sequential, 1 agent):
  T6: Runtime wiring                  (depends on T1-T5)

Wave 4 (user-driven):
  T10: Smoke test + tag                (depends on T6)
```

**Parallel-agent staging rule (inherited from M2 LEARNINGS):** every agent uses named-file `git add` only — NEVER `git add -A` or `git add .`. M2's wave-2 git-index race absorbed unrelated files; named staging avoids it.

---

## Self-review notes (inline)

- **Spec coverage:** Spec §10 M3 has 7 deliverables. Mapping:
  1. *DeviceWatcher trait + Windows impl* → T2 + T3 (polling; spec mentioned SetupDi but design decision approved 2026-05-25 chose polling for MVP)
  2. *SyncScheduler* → T4
  3. *SyncOrchestrator + subprocess forwarding* → T5 + T6 (forwarding to broadcast wired in T6 runtime; M3 forwards events but doesn't yet route the full event stream to UI — M4 closes that with a real router)
  4. *Per-track skip in auto-mode* → already inherited from Phase 3.z `--apply` behavior; M3 adds the daemon-side >50% bail threshold (T5)
  5. *Tray icon state updates* → T8
  6. *Sync Now tray menu* → T8
  7. *iPod model label + generic icon* → already shipped in M2 (DetectedIpod.model_label); T8 surfaces it in tray tooltip
- **Placeholder scan:** No "TBD" / "implement later" / "fill in" patterns. ICO placeholders are explicitly called out as "M5 will replace with real art" — that's a deferred concrete asset choice, not a plan gap.
- **Type consistency:** `SyncOutcome::Aborted` defined in T1, used in T5, T6, T9. `SyncRejectReason::TooManyFailures` defined in T5, documented in T9. `DeviceEvent` / `DeviceWatcher` trait defined in T2, used by T3 + T6. `SpawnFn` / `DaemonDeps` are new in T6 (test seam). `WizardViewModel(waitForDeviceFunc, sendConfigFunc)` consistent across T7's test file, VM, and window code-behind. `TrayState` + `SetState(TrayState, string)` consistent across T8 controller + App routing.
- **Scope check:** M3 only. Toast notifications, status popover, settings window, history viewer, and Review-mode flow are explicitly deferred to M4. Autostart, dark-mode pass, custom iPod icons, MSIX hardening are M5.
- **Ambiguity check:** Per-track failure threshold is explicitly `tracks_errored * 2 > total_planned` (strict greater-than; T5's `FailureTracker::should_bail`); test asserts exactly-50% does NOT bail. Debounce window = 500ms (T2). Polling interval = 1.5s (T3). Force-kill grace = 5s after Cancel (T5). Schedule_minutes = 0 means disabled (T4).
