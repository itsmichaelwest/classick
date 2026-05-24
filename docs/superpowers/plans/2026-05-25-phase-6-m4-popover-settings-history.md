# Phase 6 M4: Status Popover + Settings + Toasts + History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the visible UI surface that turns M3's headless daemon into a usable app — left-click the tray icon for a Windows 11 file-provider-style status popover with current activity + recent history; toast notifications honour `notify_on`; full Settings window (General / Schedule / History / About). Plus close the M3 architectural gap: introduce a real `DaemonEventRouter` in C# so the wizard, tray, popover, and notification service can all subscribe concurrently to daemon events (kills the wizard-vs-tray exclusivity hack from M3). On the Rust side: snapshot `StatusUpdate` on every new client connection, forward sync subprocess events to UI clients, and emit RFC3339 timestamps.

**Architecture:** C# work dominates this milestone. A new `DaemonEventRouter` owns the single `DaemonClient.Events` channel and dispatches typed events to .NET event subscribers (`StatusUpdated`, `DeviceConnected`, `SyncRejected`, `IpcEvent`, etc.). Three new windows: `PopoverWindow` (frameless 360×dynamic, Mica backdrop, anchored above tray icon, light-dismiss), `SettingsWindow` (700×500 NavigationView shell with four `Page` tabs), and the existing `WizardWindow` is retrofitted to subscribe through the router. `NotificationService` wraps `AppNotificationManager` for toasts driven by `StatusUpdate` events filtered by `notify_on`. Daemon-side: `SyncOrchestrator` finally uses its `event_tx` parameter to forward IPC events from the sync subprocess, and a new `new-client` internal channel lets `ipc_server` ask the runtime to broadcast a snapshot `StatusUpdate` whenever a fresh UI connects. Per spec `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md` §10 M4 and the M3 carry-forwards documented in `LEARNINGS.md`.

**Tech Stack:** .NET 10 + WinUI 3 (`Microsoft.UI.Xaml.Window`, NavigationView, `Microsoft.UI.Composition.SystemBackdrops.MicaController` via existing `Microsoft.WindowsAppSDK` 2.1.3). `Microsoft.Windows.AppNotifications.AppNotificationManager` for toasts (already available via SDK; no new NuGet). H.NotifyIcon 2.4.1 (already in csproj). Rust stable + Tokio (already wired). No new crate or package dependencies.

**Plan scope:** M4 only. M5 (autostart, dark-mode pass, accessibility audit, per-generation iPod artwork, MSIX hardening, code signing) gets its own plan after M4 ships.

**Gate:** End-to-end manual smoke per spec §13 acceptance criteria #2, #4, #5, #6, #8, #9, #10. User plugs iPod → toast appears → tray flips to Syncing → left-clicks tray → popover opens with live progress + recent history → sync completes → "Sync complete: +N -M tracks" toast → right-click → Settings → all four tabs render + Save persists. ReviewPage flow is verified if T15 ships; otherwise documented as M5 follow-up with the `subsequent_sync_mode = auto-apply` default making post-first-sync auto.

---

## File Structure

```
F:\repos\ipod-sync\
├── src\
│   ├── daemon\
│   │   ├── format.rs                                       (NEW: RFC3339 helper, ~20 LOC)
│   │   ├── mod.rs                                          (modify: add format)
│   │   ├── runtime.rs                                      (modify: use format::rfc3339_now, wire new-client snapshot, pass event_tx to orchestrator)
│   │   ├── ipc_server.rs                                   (modify: new-client signal channel)
│   │   └── sync_orchestrator.rs                            (modify: forward subprocess IpcEvents via broadcast)
│   └── ipc_daemon.rs                                       (modify: SyncEvent wrapper variant)
├── ui-windows\
│   ├── IpodSync.UI.Core\
│   │   └── Ipc\
│   │       ├── DaemonEvent.cs                              (modify: SyncEvent variant)
│   │       └── DaemonEventRouter.cs                        (NEW: typed event fan-out)
│   ├── IpodSync.UI\
│   │   ├── App.xaml                                        (modify: SettingsCommand resource)
│   │   ├── App.xaml.cs                                     (modify: use router; wire Settings; popover)
│   │   ├── TrayIconController.cs                           (modify: left-click + Settings menu)
│   │   ├── Notifications\NotificationService.cs            (NEW)
│   │   ├── Views\
│   │   │   ├── PopoverWindow.xaml + .xaml.cs               (NEW)
│   │   │   ├── SettingsWindow.xaml + .xaml.cs              (NEW)
│   │   │   ├── SettingsGeneralPage.xaml + .xaml.cs         (NEW)
│   │   │   ├── SettingsSchedulePage.xaml + .xaml.cs        (NEW)
│   │   │   ├── SettingsHistoryPage.xaml + .xaml.cs         (NEW)
│   │   │   ├── SettingsAboutPage.xaml + .xaml.cs           (NEW)
│   │   │   └── WizardWindow.xaml.cs                        (modify: use router)
│   │   └── ViewModels\
│   │       ├── PopoverViewModel.cs                         (NEW)
│   │       ├── SettingsViewModel.cs                        (NEW: shell VM + 4 tab VMs)
│   │       └── WizardViewModel.cs                          (modify: drop CancelWait, takes router instead)
│   └── IpodSync.UI.Tests\
│       ├── DaemonEventRouterTests.cs                       (NEW)
│       ├── PopoverViewModelTests.cs                        (NEW)
│       ├── SettingsViewModelTests.cs                       (NEW)
│       └── NotificationServiceTests.cs                     (NEW, light)
└── docs\
    └── ipc-protocol.md                                     (modify: §M4 addendum — sync_event forwarding + new-client snapshot semantics)
```

### Module responsibility delta

- **`src/daemon/format.rs`** — `pub fn rfc3339_now() -> String` and `pub fn rfc3339(unix_secs: u64) -> String`. Hand-rolled to avoid a chrono dep; format `YYYY-MM-DDTHH:MM:SSZ` from `std::time::SystemTime`. Used in `make_history_entry` (currently emits the placeholder `@{unix_secs}` per M3's deferred-formatter note).
- **`src/daemon/ipc_server.rs`** — new mpsc `new_client_tx` is passed in from the runtime. When `handle_client` finishes its Hello write, it sends `()` over `new_client_tx`. The runtime's `select!` picks this up and broadcasts a fresh `StatusUpdate` snapshot.
- **`src/daemon/sync_orchestrator.rs`** — `event_tx: broadcast::Sender<DaemonEvent>` is no longer unused. For every parsed line from the subprocess stdout, build a `DaemonEvent::SyncEvent { line: raw_json }` and broadcast. UI clients receive these alongside daemon events and route them through the new `IpcEvent` deserializer.
- **`src/ipc_daemon.rs`** — `DaemonEvent::SyncEvent { line: String }` variant. Wraps a raw JSON line from the sync subprocess so the wire format stays self-describing without re-modeling every M1 event type at the daemon level.
- **`src/daemon/runtime.rs`** — calls `format::rfc3339_now` for history timestamps. Constructs a real `event_tx`-bearing closure for `spawn_sync` (no more dummy `(tx, _rx) = broadcast::channel(1)` — passes the runtime's actual broadcast Sender clone). Selects on `new_client_rx` and emits a snapshot `StatusUpdate` per signal.
- **`ui-windows/IpodSync.UI.Core/Ipc/DaemonEventRouter.cs`** — owns the only `DaemonClient.Events` reader. Exposes typed events: `StatusUpdated`, `ConfigUpdated`, `HistoryUpdated`, `DeviceConnected`, `DeviceDisconnected`, `SyncRejected`, `SyncEvent` (forwarded M1 IpcEvent, deserialized again from the wrapped line). `Start(DispatcherQueue)` spawns the reader task; `Stop()` cancels. All subscribers attach via `+=`.
- **`ui-windows/IpodSync.UI/Notifications/NotificationService.cs`** — `Initialize(DaemonEventRouter, getConfig)` subscribes to `StatusUpdated` and emits toasts via `AppNotificationManager.Default`. Filters per current `notify_on` value: `all` → start + complete + error; `errors_only` → error only; `none` → silent. Last-broadcast state stored locally so we don't double-fire (only transitions trigger).
- **`ui-windows/IpodSync.UI/Views/PopoverWindow.xaml + .xaml.cs`** — frameless WinUI 3 `Window`, 360×dynamic (max 480), Mica backdrop via `MicaController`, anchored above the tray icon (computed at open via `H.NotifyIcon`'s screen-rect API). Light-dismiss handled by tracking deactivation. Bound to `PopoverViewModel`.
- **`ui-windows/IpodSync.UI/ViewModels/PopoverViewModel.cs`** — observable status text (derived from latest `StatusUpdate`), connected-device label, activity feed `ObservableCollection<HistoryEntryViewModel>` (latest 5 from `get_history`), commands: `SyncNowCommand`, `OpenSettingsCommand`, `OpenSourceFolderCommand`, `ShowAllHistoryCommand`.
- **`ui-windows/IpodSync.UI/Views/SettingsWindow.xaml + .xaml.cs`** — 700×500 `Window` containing a `NavigationView` (left sidebar, 4 items). Hosts a `Frame` that loads one of the 4 `Page` instances per nav selection. Save / Cancel buttons in window footer; Save broadcasts a `SaveConfigCommand` aggregating dirty fields across tabs.
- **`ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs`** — shell VM holding the current `PersistedConfig` snapshot + per-tab sub-VMs (`SettingsGeneralViewModel`, `SettingsScheduleViewModel`, `SettingsHistoryViewModel`, `SettingsAboutViewModel`). Tracks dirty flags. Save aggregates into one `SaveConfigCommand`.
- **`ui-windows/IpodSync.UI/Views/SettingsGeneralPage.xaml + .xaml.cs`** — source path (label + Change button → folder picker), iPod identity (model + serial labels, Re-identify button → opens wizard step 2 only — deferred to M5), sync_mode dropdown (Review / AutoApply for `first_sync_mode` + `subsequent_sync_mode`), notify_on dropdown (All / ErrorsOnly / None).
- **`ui-windows/IpodSync.UI/Views/SettingsSchedulePage.xaml + .xaml.cs`** — schedule slider (0 = disabled, 5–1440 min), autostart-with-Windows toggle (disabled with explanatory tooltip; M5 wires StartupTask).
- **`ui-windows/IpodSync.UI/Views/SettingsHistoryPage.xaml + .xaml.cs`** — scrollable `ListView` of `HistoryEntryViewModel`s. Click row → expander shows full detail (error_message, summary breakdown). "Clear history" button → confirm dialog → send `SaveConfigCommand` with cleared history flag (actually new command `ClearHistoryCommand`; daemon-side add).
- **`ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml + .xaml.cs`** — version (UI assembly version + core_version from latest Hello), license (MIT/Apache-2.0), GitHub link (`https://github.com/itsmichaelwest/ipod-sync` — placeholder, may not exist yet; just link), "Show log folder" button → opens `%LOCALAPPDATA%\ipod-sync\logs` in Explorer.
- **`ui-windows/IpodSync.UI/TrayIconController.cs`** — adds `PopoverRequested` event (raised on left-click) and `SettingsRequested` event (raised on context-menu Settings click). Wires `SettingsCommand` from XAML resources.
- **`ui-windows/IpodSync.UI/App.xaml.cs`** — major refactor. Creates and starts the `DaemonEventRouter`. Replaces the M3 `StartTrayEventLoop` task with router subscriptions. Wires popover open on `Tray.PopoverRequested` and settings open on `Tray.SettingsRequested`. Removes the wizard-channel-exclusivity hack (wizard now uses the router).
- **`ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs`** + **`ViewModels/WizardViewModel.cs`** — drops `CancelWait()` + the embedded channel-read loop. The wait function now subscribes to `router.DeviceConnected += handler`, raises a `TaskCompletionSource<IpodIdentityCandidate>`, unsubscribes in `finally`. No more channel exclusivity.

### Wave plan (recommended max-parallel)

```
Wave 1 — foundation + Rust (5 parallel, all independent files):
  T1 Rust RFC3339            src/daemon/format.rs + runtime.rs
  T2 Rust sync_event fwd     src/ipc_daemon.rs + sync_orchestrator.rs + runtime.rs
  T3 Rust new-client snapshot src/daemon/ipc_server.rs + runtime.rs
  T4 C# DaemonEventRouter    ui-windows/IpodSync.UI.Core/...
  T6 SettingsWindow shell    ui-windows/IpodSync.UI/Views/Settings*.xaml (shell only)

  ⚠ T1, T2, T3 all touch runtime.rs. To avoid the M3-style stale-edit
  race, run T1 → T2 → T3 sequentially inside Wave 1's Rust slot
  (single agent owns runtime.rs end-to-end), while T4 and T6 run in
  parallel on independent C# files. So Wave 1 effectively dispatches
  THREE concurrent agents (Rust-runtime-bundle, T4, T6).

Wave 2 — independent C# (max 3 parallel since these don't touch shared files):
  T5  NotificationService  (depends on T4)
  T11 PopoverWindow + VM   (depends on T4)
  T7  SettingsGeneralPage  (depends on T6)
  T8  SettingsSchedulePage (depends on T6)
  T9  SettingsHistoryPage  (depends on T6)
  T10 SettingsAboutPage    (depends on T6)

  These six all touch ONLY their own new files plus
  SettingsViewModel.cs (which T7-T10 share). Dispatch in two sub-waves
  of three to keep SettingsViewModel.cs edits sequential:
    Wave 2a: T5, T11, T7
    Wave 2b: T8, T9, T10

Wave 3 — integration (sequential, 1 at a time — all touch App.xaml*.cs / TrayIconController):
  T12 TrayIconController left-click + Settings menu
  T13 App.xaml.cs uses router (drops M3 tray-loop hack)
  T14 WizardWindow + VM use router

Wave 4 — optional advanced:
  T15 Review-mode subprocess pass-through  (skippable for M4 gate; M5 if it slips)

Wave 5 — user-driven:
  T16 Smoke + tag
```

---

## Task 1: RFC3339 timestamp formatter

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\format.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\runtime.rs`

Replaces the M3 `format_iso8601` placeholder that emits `@{unix_secs}` with a proper RFC3339 UTC formatter. Hand-rolled to avoid pulling in a date crate for what amounts to ~20 lines of arithmetic.

- [ ] **Step 1: Write failing tests**

Create `src/daemon/format.rs`:

```rust
//! RFC3339 timestamp emission for history entries. Hand-rolled so we
//! don't take a chrono dep just for this. Format is the strict subset
//! `YYYY-MM-DDTHH:MM:SSZ` (UTC, second precision, no fractional, no
//! offset variations).

/// Format an absolute unix-second timestamp as RFC3339 UTC.
/// Example: 1779559179 -> "2026-05-23T17:59:39Z".
pub fn rfc3339(unix_secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = unix_to_ymdhms(unix_secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Current time as RFC3339 UTC string. Convenience wrapper.
pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

/// Convert unix seconds to (year, month, day, hour, minute, second)
/// in UTC. Public for testing.
pub fn unix_to_ymdhms(unix_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    let ss = (secs_of_day % 60) as u32;

    // Civil-from-days, Howard Hinnant's algorithm:
    // http://howardhinnant.github.io/date_algorithms.html#civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64 + era * 400) as u32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_correctly() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_formats_correctly() {
        // 2026-05-23T17:59:39Z = 1779559179 (the user-encountered
        // timestamp from the M3 smoke log).
        assert_eq!(rfc3339(1_779_559_179), "2026-05-23T17:59:39Z");
    }

    #[test]
    fn leap_day_2024_formats_correctly() {
        // 2024-02-29T00:00:00Z = 1709164800
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn non_leap_century_2100_formats_correctly() {
        // 2100-03-01T00:00:00Z = 4107542400 (2100 is NOT a leap year
        // — divisible by 100 but not 400).
        assert_eq!(rfc3339(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_now_is_well_formed() {
        let s = rfc3339_now();
        assert!(s.len() == 20, "expected 20-char fixed length, got: {s}");
        assert!(s.ends_with('Z'), "expected trailing Z, got: {s}");
        assert!(s.chars().nth(4) == Some('-'), "expected dash at pos 4: {s}");
        assert!(s.chars().nth(10) == Some('T'), "expected T at pos 10: {s}");
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
pub mod format;
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

- [ ] **Step 3: Run tests, expect PASS**

```powershell
cargo test --lib daemon::format 2>&1 | Select-String "test result"
```

Expected: PASS (5 tests).

- [ ] **Step 4: Wire into runtime.rs**

In `src/daemon/runtime.rs`, find the `format_iso8601` function and the `make_history_entry` function that uses it. Replace `format_iso8601(now)` with `crate::daemon::format::rfc3339(now)`. Delete the `format_iso8601` helper.

Find this block:

```rust
fn format_iso8601(unix_secs: u64) -> String {
    // Minimal ISO8601 without a chrono dep; UTC.
    use std::time::{Duration, UNIX_EPOCH};
    let _ = UNIX_EPOCH + Duration::from_secs(unix_secs);
    // Just emit the unix ts as a placeholder string. UI displays
    // history.timestamp verbatim; M4 popover will format properly.
    format!("@{unix_secs}")
}
```

Delete it. Then in `make_history_entry`, change:

```rust
    HistoryEntry {
        timestamp: format_iso8601(now),
```

to:

```rust
    HistoryEntry {
        timestamp: crate::daemon::format::rfc3339(now),
```

- [ ] **Step 5: Run all tests**

```powershell
cargo test --lib 2>&1 | Select-String "test result" | Select-Object -Last 2
```

Expected: all tests still pass (149 + 5 new = 154).

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/format.rs src/daemon/mod.rs src/daemon/runtime.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): RFC3339 timestamp formatter (replaces M3 placeholder)"
```

---

## Task 2: Forward sync subprocess events to UI clients

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipc_daemon.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\sync_orchestrator.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\runtime.rs`

M3 left `SyncOrchestrator::run`'s `event_tx` parameter intentionally unused (the docstring says "wired in Task 6 / runtime" but it never was — see `LEARNINGS.md` M3 entry). M4 needs the popover + ProgressPage to see live TrackStart / TrackDone / Log / Error events. Adds a new `DaemonEvent::SyncEvent { line: String }` variant that wraps the raw subprocess line (preserves wire format without re-modeling every M1 type at the daemon level), broadcasts it on every parsed subprocess event, and the C# `DaemonClient` already passes through unknown discriminators as M1 `IpcEvent` (per the M3 peek-discriminator fix), so consumers just deserialize the wrapped line.

- [ ] **Step 1: Add SyncEvent variant**

In `src/ipc_daemon.rs`, find the `DaemonEvent` enum and add the new variant. The full updated enum body:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    Hello {
        protocol_version: String,
        core_version: String,
    },
    StatusUpdate {
        state: DaemonStateLabel,
        configured: bool,
        ipod_connected: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_sync: Option<HistoryEntry>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_scheduled_unix_secs: Option<u64>,
    },
    ConfigUpdate {
        source: Option<String>,
        daemon: Option<DaemonSettings>,
        ipod: Option<IpodIdentity>,
    },
    HistoryUpdate {
        entries: Vec<HistoryEntry>,
    },
    DeviceConnected {
        serial: String,
        model_label: String,
        drive: String,
    },
    DeviceDisconnected {
        serial: String,
    },
    SyncRejected {
        reason: SyncRejectReason,
    },
    /// Forwarded sync-subprocess event. `line` is the raw JSON line
    /// the subprocess emitted on its stdout, unparsed. UI clients
    /// deserialize it as an M1 `IpcEvent`. Wrapping rather than
    /// re-modeling keeps the daemon protocol decoupled from the M1
    /// stdio protocol — bumping M1 doesn't bump daemon-protocol
    /// semver.
    SyncEvent {
        line: String,
    },
}
```

- [ ] **Step 2: Write a failing test for the SyncEvent serialization shape**

Append to `src/ipc_daemon.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn sync_event_serializes_with_line_field() {
        let evt = DaemonEvent::SyncEvent {
            line: r#"{"type":"track_done"}"#.to_string(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"sync_event""#));
        assert!(json.contains(r#""line":"{\"type\":\"track_done\"}""#),
                "got: {json}");
    }
```

- [ ] **Step 3: Run, expect PASS**

```powershell
cargo test --lib ipc_daemon 2>&1 | Select-String "test result"
```

Expected: PASS (all ipc_daemon tests including the new one).

- [ ] **Step 4: Wire forwarding into the orchestrator**

In `src/daemon/sync_orchestrator.rs`, the `run` function's subprocess-read loop currently parses lines for stats but doesn't forward. Replace the loop body:

Find:

```rust
    while let Some(line) = reader.next_line().await? {
        let Some(value) = serde_json::from_str::<Value>(&line).ok() else { continue };
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
```

and replace with:

```rust
    while let Some(line) = reader.next_line().await? {
        // Forward EVERY parseable line to the daemon's broadcast channel
        // so UI clients see live sync progress. Wrapping the raw line in
        // a SyncEvent envelope keeps the daemon protocol independent
        // from M1 stdio-IPC semver.
        let _ = event_tx.send(DaemonEvent::SyncEvent { line: line.clone() });

        let Some(value) = serde_json::from_str::<Value>(&line).ok() else { continue };
        let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ty {
```

Then delete the existing `let _ = event_tx;` placeholder line near the top of `run`.

- [ ] **Step 5: Update runtime's production spawn_sync to use the real event_tx**

In `src/daemon/runtime.rs`, find the production `run_daemon`:

```rust
    let exe = std::env::current_exe()?;
    let spawn_sync: SpawnFn = Arc::new(move |drive: String| {
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
```

This is the dummy-channel construction. The real `event_tx` is created later inside `run_daemon_with_deps`. To plumb it through we restructure: build the `event_tx` first (move `spawn_server` ahead of `spawn_sync` construction), then build the closure that captures `event_tx.clone()`. Replace `run_daemon` body with:

```rust
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

    // Build the broadcast event_tx FIRST so the spawn_sync closure can
    // capture a clone — that way orchestrator events flow through the
    // same channel UI clients are subscribed to.
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let exe = std::env::current_exe()?;
    let event_tx_for_spawn = event_tx.clone();
    let spawn_sync: SpawnFn = Arc::new(move |drive: String| {
        let exe = exe.clone();
        let event_tx = event_tx_for_spawn.clone();
        Box::pin(async move {
            sync_orchestrator::run(exe, drive, event_tx).await
        })
    });

    let deps = DaemonDeps {
        configured_serial,
        watcher: Box::new(PollingDeviceWatcher::new_production()),
        spawn_sync,
        schedule_minutes,
        preset_event_tx: Some(event_tx),
    };
    run_daemon_with_deps(deps).await
}
```

Then update the `DaemonDeps` struct to accept the preset channel:

```rust
pub struct DaemonDeps {
    pub configured_serial: Option<String>,
    pub watcher: Box<dyn DeviceWatcher>,
    pub spawn_sync: SpawnFn,
    pub schedule_minutes: u32,
    /// If Some, the runtime uses this pre-built sender instead of
    /// constructing its own. Production passes the same one it gave
    /// to the spawn_sync closure so orchestrator events broadcast on
    /// the same channel UI clients subscribe to.
    pub preset_event_tx: Option<broadcast::Sender<DaemonEvent>>,
}
```

And in `run_daemon_with_deps`, replace the `spawn_server().await` block so it honours the preset:

Find:

```rust
    let (event_tx, mut cmd_rx) = spawn_server().await?;
```

Replace with:

```rust
    let (event_tx, mut cmd_rx) = match deps.preset_event_tx {
        Some(tx) => {
            // Production: reuse the channel that spawn_sync already
            // captured a clone of. ipc_server::spawn_server needs to
            // share the same sender — pass it in.
            spawn_server_with_event_tx(tx).await?
        }
        None => spawn_server().await?,  // test path
    };
```

`spawn_server_with_event_tx` is a new helper added in T3 (alongside the new-client snapshot work). For now, define a stub:

```rust
async fn spawn_server_with_event_tx(
    _preset: broadcast::Sender<DaemonEvent>,
) -> Result<(broadcast::Sender<DaemonEvent>, mpsc::UnboundedReceiver<crate::daemon::ipc_server::ClientCommand>)> {
    // T3 swaps this for the real impl that wires ipc_server with
    // the supplied event_tx + the new-client signal channel.
    spawn_server().await
}
```

- [ ] **Step 6: Update integration tests for the new DaemonDeps field**

Find the three test setups in `tests/daemon_runtime_integration.rs` and add `preset_event_tx: None,` to each `DaemonDeps { ... }` literal:

```rust
    let deps = DaemonDeps {
        configured_serial: Some("0xABC".to_string()),
        watcher: Box::new(watcher),
        spawn_sync: Arc::new(spawn_fn),
        schedule_minutes: 0,
        preset_event_tx: None,
    };
```

(Replicate for the other two tests.)

- [ ] **Step 7: Run tests, expect PASS**

```powershell
cargo test --lib 2>&1 | Select-String "test result" | Select-Object -Last 2
cargo test --test daemon_runtime_integration 2>&1 | Select-String "test result"
```

Expected: lib + integration tests all green.

- [ ] **Step 8: Commit**

```powershell
git -C F:\repos\ipod-sync add src/ipc_daemon.rs src/daemon/sync_orchestrator.rs src/daemon/runtime.rs tests/daemon_runtime_integration.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): forward sync subprocess events to UI clients via SyncEvent envelope"
```

---

## Task 3: Snapshot StatusUpdate on every new client connection

**Files:**
- Modify: `F:\repos\ipod-sync\src\daemon\ipc_server.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\runtime.rs`

Currently `tokio::sync::broadcast` doesn't replay missed messages to new subscribers — a UI that connects after the daemon already broadcast `DeviceConnected` never sees it. M3 partially papered over this by having App.xaml.cs send `GetStatus` after connecting, but it's fragile (popover-on-open would have the same race against an in-flight broadcast). The proper fix: `ipc_server`'s per-client handler signals the runtime when a new client connects; the runtime broadcasts a fresh `StatusUpdate` snapshot in response. All clients (including the new one) get the snapshot — small redundancy is fine.

- [ ] **Step 1: Extend ipc_server::spawn_server with a new-client signal**

In `src/daemon/ipc_server.rs`, add a second version of `spawn_server` that accepts the broadcast sender as an argument AND returns an additional `mpsc::UnboundedReceiver<()>` for new-client signals. Existing `spawn_server` stays as a thin wrapper for tests.

Replace the existing `spawn_server` function with:

```rust
/// Test-friendly entry: creates a fresh broadcast channel.
pub async fn spawn_server() -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
)> {
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let (sender, cmd_rx, _new_client_rx) = spawn_server_full(event_tx.clone()).await?;
    Ok((sender, cmd_rx))
}

/// Production entry: caller supplies the broadcast sender so it can
/// be shared with the sync orchestrator (which also publishes to it).
/// Returns an extra mpsc receiver that fires once per new client
/// connection — the runtime uses this to publish a snapshot
/// StatusUpdate so newly-connected UIs don't miss earlier broadcasts.
pub async fn spawn_server_full(
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
    mpsc::UnboundedReceiver<()>,
)> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
    let (new_client_tx, new_client_rx) = mpsc::unbounded_channel::<()>();

    let event_tx_clone = event_tx.clone();
    let new_client_tx_clone = new_client_tx.clone();
    tokio::spawn(async move {
        let mut next_client_id: u64 = 1;
        let mut server = match ServerOptions::new()
            .first_pipe_instance(true)
            .create(PIPE_NAME)
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("ipc-server: failed to create initial named pipe: {e}");
                return;
            }
        };
        tracing::info!("ipc-server: listening on {PIPE_NAME}");

        loop {
            if let Err(e) = server.connect().await {
                tracing::warn!("ipc-server: connect failed: {e}");
                continue;
            }
            let connected = server;
            server = match ServerOptions::new().create(PIPE_NAME) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("ipc-server: failed to create next pipe instance: {e}");
                    return;
                }
            };
            let client_id = next_client_id;
            next_client_id += 1;
            let event_rx = event_tx_clone.subscribe();
            let cmd_tx = cmd_tx.clone();
            let new_client_tx = new_client_tx_clone.clone();
            tokio::spawn(handle_client(client_id, connected, event_rx, cmd_tx, new_client_tx));
        }
    });

    Ok((event_tx, cmd_rx, new_client_rx))
}
```

- [ ] **Step 2: Signal after Hello write in handle_client**

In the same file, update `handle_client` to accept the new sender and fire after Hello succeeds:

```rust
async fn handle_client(
    client_id: u64,
    pipe: NamedPipeServer,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
    new_client_tx: mpsc::UnboundedSender<()>,
) {
    tracing::info!("ipc-server: client {client_id} connected");
    let (reader_half, mut writer_half) = tokio::io::split(pipe);

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<DaemonEvent>();

    let hello = DaemonEvent::Hello {
        protocol_version: DAEMON_PROTOCOL_VERSION.to_string(),
        core_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if write_event(&mut writer_half, &hello).await.is_err() {
        return;
    }

    // Signal the runtime to broadcast a snapshot StatusUpdate so this
    // newly-connected client sees current state without needing to
    // race against any in-flight broadcasts.
    let _ = new_client_tx.send(());

    // (rest of handle_client body unchanged)
    let mut reader = BufReader::new(reader_half);
    let mut line_buf = String::new();
    loop {
        // ... existing select! body ...
    }
}
```

Keep the rest of the function body as-is (the `tokio::select!` over read_line / broadcast_event / reply_event remains unchanged).

- [ ] **Step 3: Replace the stub spawn_server_with_event_tx in runtime.rs**

In `src/daemon/runtime.rs`, find the T2 stub:

```rust
async fn spawn_server_with_event_tx(
    _preset: broadcast::Sender<DaemonEvent>,
) -> Result<...> {
    spawn_server().await
}
```

Delete it. In `run_daemon_with_deps`, replace the preset/match block:

```rust
    let (event_tx, mut cmd_rx) = match deps.preset_event_tx {
        Some(tx) => spawn_server_with_event_tx(tx).await?,
        None => spawn_server().await?,
    };
```

with:

```rust
    let (event_tx, mut cmd_rx, mut new_client_rx) = match deps.preset_event_tx {
        Some(tx) => crate::daemon::ipc_server::spawn_server_full(tx).await?,
        None => {
            let (tx, rx) = spawn_server().await?;
            // Test path: synthesize an empty new-client channel that
            // never fires. The integration tests don't exercise snapshot
            // semantics; production goes through spawn_server_full.
            let (_dummy_tx, dummy_rx) = mpsc::unbounded_channel::<()>();
            (tx, rx, dummy_rx)
        }
    };
```

Then add a new select arm AFTER the existing `Some(internal) = internal_rx.recv()` arm and BEFORE the scheduler arm:

```rust
            Some(()) = new_client_rx.recv() => {
                // A fresh UI connected. Publish a snapshot StatusUpdate
                // so the new subscriber's tray + popover initialize
                // with current state, even if earlier broadcasts (e.g.
                // DeviceConnected from polling at daemon startup) went
                // out before they subscribed.
                broadcast_status(&event_tx, &state, &connected, &config_path, &history);
            }
```

- [ ] **Step 4: Run tests, expect PASS**

```powershell
cargo test --lib daemon 2>&1 | Select-String "test result"
cargo test --test daemon_runtime_integration 2>&1 | Select-String "test result"
```

Expected: green. (Existing tests don't exercise the new-client channel; they take the `None` arm which gives them a never-firing receiver.)

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/ipc_server.rs src/daemon/runtime.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): snapshot StatusUpdate on every new client connection"
```

---

## Task 4: DaemonEventRouter — typed event fan-out for C# consumers

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\DaemonEventRouter.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\DaemonEventRouterTests.cs`

The architectural fix from M3 LEARNINGS: a central component owns the only `DaemonClient.Events` reader and dispatches typed events to N concurrent subscribers via .NET events. Kills the wizard-vs-tray exclusivity hack.

- [ ] **Step 1: Write failing tests**

Create `ui-windows/IpodSync.UI.Tests/DaemonEventRouterTests.cs`:

```csharp
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using Xunit;

public class DaemonEventRouterTests
{
    [Fact]
    public async Task Routes_status_update_to_typed_subscribers()
    {
        var channel = Channel.CreateUnbounded<object>();
        StatusUpdateEvent? received = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += s => received = s;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);

        Assert.NotNull(received);
        Assert.Equal("idle", received!.State);
        router.Stop();
    }

    [Fact]
    public async Task Multiple_subscribers_all_receive_event()
    {
        var channel = Channel.CreateUnbounded<object>();
        int count1 = 0, count2 = 0;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += _ => count1++;
        router.StatusUpdated += _ => count2++;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);

        Assert.Equal(1, count1);
        Assert.Equal(1, count2);
        router.Stop();
    }

    [Fact]
    public async Task Routes_device_connected_separately_from_status()
    {
        var channel = Channel.CreateUnbounded<object>();
        StatusUpdateEvent? status = null;
        DeviceConnectedEvent? device = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += s => status = s;
        router.DeviceConnected += d => device = d;

        router.Start();
        await channel.Writer.WriteAsync(new DeviceConnectedEvent("0xABC", "iPod 7G", "G:\\"));
        await Task.Delay(50);

        Assert.Null(status);
        Assert.NotNull(device);
        Assert.Equal("0xABC", device!.Serial);
        router.Stop();
    }

    [Fact]
    public async Task Unsubscribe_stops_delivery()
    {
        var channel = Channel.CreateUnbounded<object>();
        int count = 0;
        void Handler(StatusUpdateEvent _) => count++;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += Handler;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);
        Assert.Equal(1, count);

        router.StatusUpdated -= Handler;
        await channel.Writer.WriteAsync(new StatusUpdateEvent("syncing", true, true, null, null));
        await Task.Delay(50);
        Assert.Equal(1, count);  // unchanged
        router.Stop();
    }

    [Fact]
    public async Task Sync_event_is_re_parsed_as_ipc_event_and_routed()
    {
        var channel = Channel.CreateUnbounded<object>();
        IpcEvent? routed = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.IpcEventReceived += e => routed = e;

        router.Start();
        // Wrapped sync subprocess event:
        await channel.Writer.WriteAsync(new SyncEventEnvelope(@"{""type"":""track_done""}"));
        await Task.Delay(50);

        Assert.NotNull(routed);
        Assert.IsType<TrackDoneEvent>(routed);
        router.Stop();
    }
}
```

- [ ] **Step 2: Run, expect FAIL**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~DaemonEventRouterTests" 2>&1 | Select-Object -Last 5
```

Expected: FAIL — `DaemonEventRouter` and `SyncEventEnvelope` don't exist yet.

- [ ] **Step 3: Add SyncEventEnvelope to DaemonEvent.cs**

In `ui-windows/IpodSync.UI.Core/Ipc/DaemonEvent.cs`, find the `[JsonDerivedType]` block and add the new variant + record:

```csharp
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StatusUpdateEvent), "status_update")]
[JsonDerivedType(typeof(ConfigUpdateEvent), "config_update")]
[JsonDerivedType(typeof(HistoryUpdateEvent), "history_update")]
[JsonDerivedType(typeof(DeviceConnectedEvent), "device_connected")]
[JsonDerivedType(typeof(DeviceDisconnectedEvent), "device_disconnected")]
[JsonDerivedType(typeof(SyncRejectedEvent), "sync_rejected")]
[JsonDerivedType(typeof(SyncEventEnvelope), "sync_event")]
public abstract record DaemonEvent;

// ... existing records unchanged ...

public sealed record SyncEventEnvelope(
    [property: JsonPropertyName("line")] string Line
) : DaemonEvent;
```

Then update `DaemonEventDiscriminators` in `DaemonClient.cs` to include the new type:

```csharp
    private static readonly HashSet<string> DaemonEventDiscriminators = new(StringComparer.Ordinal)
    {
        "status_update", "config_update", "history_update",
        "device_connected", "device_disconnected", "sync_rejected",
        "sync_event",
    };
```

- [ ] **Step 4: Implement DaemonEventRouter**

Create `ui-windows/IpodSync.UI.Core/Ipc/DaemonEventRouter.cs`:

```csharp
using System;
using System.Diagnostics;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace IpodSync_UI.Ipc;

/// <summary>
/// Owns the only consumer of <see cref="DaemonClient.Events"/> and
/// dispatches typed events to N concurrent .NET subscribers. Solves
/// the M3 "wizard vs tray loop have exclusive read on the channel"
/// architectural gap.
///
/// Subscribers attach via standard <c>+=</c> on the typed events.
/// All handlers fire on a background task (not the UI thread);
/// subscribers that mutate UI state must marshal via
/// <c>DispatcherQueue.TryEnqueue</c> themselves.
///
/// Lifecycle: <see cref="Start"/> spawns the reader task;
/// <see cref="Stop"/> cancels it. Idempotent on both.
/// </summary>
public sealed class DaemonEventRouter : IDisposable
{
    private readonly ChannelReader<object> _source;
    private CancellationTokenSource? _cts;
    private Task? _readerTask;

    public DaemonEventRouter(ChannelReader<object> source)
    {
        _source = source;
    }

    public event Action<StatusUpdateEvent>? StatusUpdated;
    public event Action<ConfigUpdateEvent>? ConfigUpdated;
    public event Action<HistoryUpdateEvent>? HistoryUpdated;
    public event Action<DeviceConnectedEvent>? DeviceConnected;
    public event Action<DeviceDisconnectedEvent>? DeviceDisconnected;
    public event Action<SyncRejectedEvent>? SyncRejected;
    public event Action<IpcEvent>? IpcEventReceived;

    public void Start()
    {
        if (_cts is not null) return;
        _cts = new CancellationTokenSource();
        _readerTask = Task.Run(() => ReaderLoop(_cts.Token));
    }

    public async Task StopAsync()
    {
        _cts?.Cancel();
        if (_readerTask is not null)
        {
            try { await _readerTask.ConfigureAwait(false); } catch { /* expected */ }
        }
        _readerTask = null;
        _cts?.Dispose();
        _cts = null;
    }

    public void Stop() => StopAsync().GetAwaiter().GetResult();

    private async Task ReaderLoop(CancellationToken ct)
    {
        try
        {
            await foreach (var evt in _source.ReadAllAsync(ct))
            {
                Dispatch(evt);
            }
        }
        catch (OperationCanceledException) { /* expected */ }
        catch (Exception e)
        {
            Debug.WriteLine($"daemon-event-router: reader terminated: {e}");
        }
    }

    private void Dispatch(object evt)
    {
        switch (evt)
        {
            case StatusUpdateEvent s:
                StatusUpdated?.Invoke(s);
                break;
            case ConfigUpdateEvent c:
                ConfigUpdated?.Invoke(c);
                break;
            case HistoryUpdateEvent h:
                HistoryUpdated?.Invoke(h);
                break;
            case DeviceConnectedEvent dc:
                DeviceConnected?.Invoke(dc);
                break;
            case DeviceDisconnectedEvent dd:
                DeviceDisconnected?.Invoke(dd);
                break;
            case SyncRejectedEvent sr:
                SyncRejected?.Invoke(sr);
                break;
            case SyncEventEnvelope env:
                // Re-parse the wrapped line as an M1 IpcEvent and
                // dispatch via the IpcEvent channel.
                try
                {
                    var inner = JsonSerializer.Deserialize<IpcEvent>(env.Line);
                    if (inner is not null) IpcEventReceived?.Invoke(inner);
                }
                catch (Exception e)
                {
                    Debug.WriteLine($"daemon-event-router: bad sync_event line `{env.Line}`: {e.Message}");
                }
                break;
            case IpcEvent ie:
                // M1 events that arrive directly (e.g. Hello during
                // connect already consumed by DaemonClient; this
                // covers daemon-forwarded events that the daemon
                // happens to emit un-wrapped — defensive).
                IpcEventReceived?.Invoke(ie);
                break;
            default:
                Debug.WriteLine($"daemon-event-router: unrouted event type {evt.GetType().Name}");
                break;
        }
    }

    public void Dispose() => Stop();
}
```

- [ ] **Step 5: Run tests, expect PASS**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~DaemonEventRouterTests" 2>&1 | Select-Object -Last 5
```

Expected: 5/5 pass.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI.Core/Ipc/DaemonEventRouter.cs ui-windows/IpodSync.UI.Core/Ipc/DaemonEvent.cs ui-windows/IpodSync.UI.Core/Ipc/DaemonClient.cs ui-windows/IpodSync.UI.Tests/DaemonEventRouterTests.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): DaemonEventRouter — typed fan-out for daemon events"
```

---

## Task 5: NotificationService — toasts driven by StatusUpdate

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Notifications\NotificationService.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\NotificationServiceTests.cs`

Wraps `AppNotificationManager.Default` to fire toasts on sync state transitions. Filters by current `notify_on` setting (all / errors_only / none). Listens to the router's `StatusUpdated` event. Stores last broadcast state locally so it only fires on TRANSITIONS (not every periodic broadcast).

- [ ] **Step 1: Write failing test**

Create `ui-windows/IpodSync.UI.Tests/NotificationServiceTests.cs`:

```csharp
using IpodSync_UI.Ipc;
using IpodSync_UI.Notifications;
using Xunit;

public class NotificationServiceTests
{
    private static StatusUpdateEvent Status(string state, string? errorMessage = null)
    {
        var lastSync = errorMessage is null
            ? new HistoryEntry("2026-05-25T10:00:00Z", 5, "plug_in", "ok", null,
                new SyncSummary(1, 0, 0, 0, 0))
            : new HistoryEntry("2026-05-25T10:00:00Z", 5, "plug_in", "error", errorMessage, null);
        return new StatusUpdateEvent(state, true, true, lastSync, null);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_ok_outcome_fires_complete_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Complete, decision!.Kind);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_error_outcome_fires_error_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing",
            newStatus: Status("idle", errorMessage: "Source unreachable"),
            notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Error, decision!.Kind);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_ok_does_not_fire_on_errors_only()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "errors_only");
        Assert.Null(decision);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_error_fires_on_errors_only()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing",
            newStatus: Status("idle", errorMessage: "Source unreachable"),
            notifyOn: "errors_only");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Error, decision!.Kind);
    }

    [Fact]
    public void DecideToast_anything_returns_null_when_notify_on_none()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "none");
        Assert.Null(decision);
    }

    [Fact]
    public void DecideToast_syncing_after_idle_fires_started_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "idle", newStatus: Status("syncing"), notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Started, decision!.Kind);
    }

    [Fact]
    public void DecideToast_no_transition_returns_null()
    {
        var decision = NotificationService.DecideToast(
            previousState: "idle", newStatus: Status("idle"), notifyOn: "all");
        Assert.Null(decision);
    }
}
```

- [ ] **Step 2: Run, expect FAIL**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~NotificationServiceTests" 2>&1 | Select-Object -Last 5
```

Expected: FAIL — types don't exist.

- [ ] **Step 3: Implement NotificationService**

Create `ui-windows/IpodSync.UI/Notifications/NotificationService.cs`:

```csharp
using System;
using System.Diagnostics;
using IpodSync_UI.Ipc;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace IpodSync_UI.Notifications;

public enum ToastKind { Started, Complete, Error }

public sealed record ToastDecision(ToastKind Kind, string Title, string Body);

/// <summary>
/// Fires Windows toast notifications via AppNotificationManager when
/// daemon StatusUpdate events report a sync state transition. Filter
/// honors the user's notify_on config (all / errors_only / none).
/// </summary>
public sealed class NotificationService : IDisposable
{
    private readonly DaemonEventRouter _router;
    private readonly Func<string> _getNotifyOn;
    private string _previousState = "idle";
    private bool _registered;

    public NotificationService(DaemonEventRouter router, Func<string> getNotifyOn)
    {
        _router = router;
        _getNotifyOn = getNotifyOn;
    }

    public void Initialize()
    {
        if (!_registered)
        {
            // Packaged WinUI apps get AUMID from manifest automatically.
            // Register is idempotent but logs on duplicate calls.
            try { AppNotificationManager.Default.Register(); _registered = true; }
            catch (Exception e) { Debug.WriteLine($"notify: register failed: {e.Message}"); }
        }
        _router.StatusUpdated += OnStatusUpdated;
    }

    private void OnStatusUpdated(StatusUpdateEvent s)
    {
        var decision = DecideToast(_previousState, s, _getNotifyOn());
        _previousState = s.State;
        if (decision is null) return;
        FireToast(decision);
    }

    /// <summary>
    /// Pure decision function (no AppNotificationManager dependency) so
    /// tests can exercise the matrix without a packaged-app fixture.
    /// </summary>
    public static ToastDecision? DecideToast(
        string previousState, StatusUpdateEvent newStatus, string notifyOn)
    {
        if (notifyOn == "none") return null;
        // Only act on transitions, not repeated broadcasts of the same state.
        if (previousState == newStatus.State) return null;

        // syncing -> idle: completion (ok or error).
        if (previousState == "syncing" && newStatus.State == "idle")
        {
            var outcome = newStatus.LastSync?.Outcome ?? "ok";
            if (outcome == "ok")
            {
                if (notifyOn == "errors_only") return null;
                var summary = newStatus.LastSync?.Summary;
                var body = summary is null
                    ? "Sync complete."
                    : $"Sync complete: +{summary.Add} ~{summary.Modify} -{summary.Remove}"
                      + (summary.Skipped > 0 ? $", {summary.Skipped} skipped" : "");
                return new ToastDecision(ToastKind.Complete, "ipod-sync", body);
            }
            else
            {
                var msg = newStatus.LastSync?.ErrorMessage ?? "Sync failed.";
                return new ToastDecision(ToastKind.Error, "ipod-sync — sync failed", msg);
            }
        }

        // idle -> syncing: starting.
        if (previousState == "idle" && newStatus.State == "syncing")
        {
            if (notifyOn == "errors_only") return null;
            return new ToastDecision(ToastKind.Started, "ipod-sync", "Syncing iPod…");
        }

        return null;
    }

    private void FireToast(ToastDecision d)
    {
        try
        {
            var builder = new AppNotificationBuilder()
                .AddText(d.Title)
                .AddText(d.Body);
            AppNotificationManager.Default.Show(builder.BuildNotification());
        }
        catch (Exception e)
        {
            Debug.WriteLine($"notify: toast fire failed: {e.Message}");
        }
    }

    public void Dispose()
    {
        _router.StatusUpdated -= OnStatusUpdated;
    }
}
```

- [ ] **Step 4: Run tests, expect PASS**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~NotificationServiceTests" 2>&1 | Select-Object -Last 5
```

Expected: 7/7 pass.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Notifications/NotificationService.cs ui-windows/IpodSync.UI.Tests/NotificationServiceTests.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): NotificationService — toasts driven by StatusUpdate transitions"
```

---

## Task 6: SettingsWindow shell + NavigationView

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsWindow.xaml`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsWindow.xaml.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\SettingsViewModel.cs` (shell only; tab sub-VMs land in T7–T10)

Just the shell — NavigationView with 4 menu items, hosts a `Frame` that loads each tab `Page`. Save / Cancel buttons in footer.

- [ ] **Step 1: Create SettingsWindow.xaml**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Window
    x:Class="IpodSync_UI.Views.SettingsWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    xmlns:d="http://schemas.microsoft.com/expression/blend/2008"
    xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
    mc:Ignorable="d">
    <Grid RowDefinitions="*,Auto">
        <NavigationView
            x:Name="Nav"
            Grid.Row="0"
            IsBackButtonVisible="Collapsed"
            IsSettingsVisible="False"
            PaneDisplayMode="Left"
            OpenPaneLength="180"
            SelectionChanged="Nav_SelectionChanged">
            <NavigationView.MenuItems>
                <NavigationViewItem Content="General" Tag="general" />
                <NavigationViewItem Content="Schedule" Tag="schedule" />
                <NavigationViewItem Content="History" Tag="history" />
                <NavigationViewItem Content="About" Tag="about" />
            </NavigationView.MenuItems>
            <Frame x:Name="ContentFrame" />
        </NavigationView>
        <Grid Grid.Row="1" Background="{ThemeResource LayerFillColorDefaultBrush}" Padding="16,12">
            <StackPanel Orientation="Horizontal" HorizontalAlignment="Right" Spacing="8">
                <Button Content="Cancel" Click="OnCancel" />
                <Button Content="Save"
                        Click="OnSave"
                        Style="{ThemeResource AccentButtonStyle}" />
            </StackPanel>
        </Grid>
    </Grid>
</Window>
```

- [ ] **Step 2: Create SettingsWindow.xaml.cs**

```csharp
using System;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI.Views;

public sealed partial class SettingsWindow : Window
{
    public SettingsViewModel ViewModel { get; }

    public SettingsWindow(SettingsViewModel vm)
    {
        ViewModel = vm;
        InitializeComponent();
        Title = "ipod-sync settings";
        // Default to General tab.
        Nav.SelectedItem = Nav.MenuItems[0];
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem item) return;
        var tag = item.Tag as string;
        Type? pageType = tag switch
        {
            "general"  => typeof(SettingsGeneralPage),
            "schedule" => typeof(SettingsSchedulePage),
            "history"  => typeof(SettingsHistoryPage),
            "about"    => typeof(SettingsAboutPage),
            _          => null,
        };
        if (pageType is null) return;
        ContentFrame.Navigate(pageType, ViewModel);
    }

    private async void OnSave(object sender, RoutedEventArgs e)
    {
        await ViewModel.SaveAsync();
        Close();
    }

    private void OnCancel(object sender, RoutedEventArgs e) => Close();
}
```

- [ ] **Step 3: Create SettingsViewModel shell**

```csharp
using System;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// Shell ViewModel for SettingsWindow. Holds the live PersistedConfig
/// snapshot the user is editing and exposes per-tab sub-ViewModels.
/// T7–T10 add the sub-VM bodies + bindings.
/// </summary>
public partial class SettingsViewModel : ObservableObject
{
    private readonly DaemonClient _daemon;
    private readonly DaemonEventRouter _router;

    public SettingsViewModel(DaemonClient daemon, DaemonEventRouter router, ConfigUpdateEvent currentConfig)
    {
        _daemon = daemon;
        _router = router;
        General = new SettingsGeneralViewModel(currentConfig);
        Schedule = new SettingsScheduleViewModel(currentConfig);
        History = new SettingsHistoryViewModel(daemon, router);
        About = new SettingsAboutViewModel();
    }

    public SettingsGeneralViewModel General { get; }
    public SettingsScheduleViewModel Schedule { get; }
    public SettingsHistoryViewModel History { get; }
    public SettingsAboutViewModel About { get; }

    /// <summary>
    /// Aggregate dirty fields across tabs into a single SaveConfigCommand.
    /// </summary>
    public async Task SaveAsync()
    {
        var cmd = new SaveConfigCommand(
            Source: General.IsSourceDirty ? General.SourcePath : null,
            Daemon: BuildDaemonSettings(),
            Ipod: null  // Re-identify flow is M5
        );
        try { await _daemon.SendAsync(cmd); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"settings: save failed: {e}"); }
    }

    private DaemonSettings? BuildDaemonSettings()
    {
        if (!General.IsAnyDaemonFieldDirty && !Schedule.IsAnyDirty) return null;
        return new DaemonSettings(
            Enabled: true,
            AutostartWithWindows: Schedule.AutostartWithWindows,
            FirstSyncMode: General.FirstSyncMode,
            SubsequentSyncMode: General.SubsequentSyncMode,
            ScheduleMinutes: (uint)Schedule.ScheduleMinutes,
            NotifyOn: General.NotifyOn);
    }
}

// Sub-VM stubs — filled in by T7–T10. Defined here so SettingsViewModel
// compiles in T6's standalone wave; T7–T10 add the [ObservableProperty]
// fields + Save logic for each tab.

public partial class SettingsGeneralViewModel : ObservableObject
{
    public SettingsGeneralViewModel(ConfigUpdateEvent c) { /* T7 */ }
    public string SourcePath { get; set; } = "";
    public bool IsSourceDirty => false;  // T7
    public bool IsAnyDaemonFieldDirty => false;  // T7
    public string FirstSyncMode { get; set; } = "review";
    public string SubsequentSyncMode { get; set; } = "auto_apply";
    public string NotifyOn { get; set; } = "all";
}

public partial class SettingsScheduleViewModel : ObservableObject
{
    public SettingsScheduleViewModel(ConfigUpdateEvent c) { /* T8 */ }
    public int ScheduleMinutes { get; set; } = 30;
    public bool AutostartWithWindows { get; set; }
    public bool IsAnyDirty => false;  // T8
}

public partial class SettingsHistoryViewModel : ObservableObject
{
    public SettingsHistoryViewModel(DaemonClient d, DaemonEventRouter r) { /* T9 */ }
}

public partial class SettingsAboutViewModel : ObservableObject
{
    public SettingsAboutViewModel() { /* T10 */ }
}
```

- [ ] **Step 4: Create the four tab Page stubs**

Each of these is a placeholder so navigation doesn't crash. T7–T10 replace them with real implementations. Create four files with identical structure (different class names):

`Views/SettingsGeneralPage.xaml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<Page
    x:Class="IpodSync_UI.Views.SettingsGeneralPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <StackPanel Padding="24" Spacing="12">
        <TextBlock Text="General" Style="{ThemeResource TitleTextBlockStyle}" />
        <TextBlock Text="(T7 fills this in.)" Opacity="0.6" />
    </StackPanel>
</Page>
```

`Views/SettingsGeneralPage.xaml.cs`:

```csharp
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsGeneralPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsGeneralPage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
    }
}
```

Repeat for `SettingsSchedulePage` (T8), `SettingsHistoryPage` (T9), `SettingsAboutPage` (T10) — same shell, change class name and title text.

- [ ] **Step 5: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED|error CS" | Select-Object -Last 5
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/SettingsWindow.xaml ui-windows/IpodSync.UI/Views/SettingsWindow.xaml.cs ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs ui-windows/IpodSync.UI/Views/SettingsGeneralPage.xaml ui-windows/IpodSync.UI/Views/SettingsGeneralPage.xaml.cs ui-windows/IpodSync.UI/Views/SettingsSchedulePage.xaml ui-windows/IpodSync.UI/Views/SettingsSchedulePage.xaml.cs ui-windows/IpodSync.UI/Views/SettingsHistoryPage.xaml ui-windows/IpodSync.UI/Views/SettingsHistoryPage.xaml.cs ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): SettingsWindow shell + 4 tab page stubs"
```

---

## Task 7: SettingsGeneralPage — source + iPod + sync mode + notify level

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsGeneralPage.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsGeneralPage.xaml.cs` (just bindings; nothing complex)
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\SettingsViewModel.cs` (real `SettingsGeneralViewModel`)

- [ ] **Step 1: Replace SettingsGeneralViewModel stub with real impl**

In `SettingsViewModel.cs`, replace the `public partial class SettingsGeneralViewModel` block with:

```csharp
public partial class SettingsGeneralViewModel : ObservableObject
{
    private readonly string _originalSource;
    private readonly DaemonSettings? _originalDaemon;

    public SettingsGeneralViewModel(ConfigUpdateEvent c)
    {
        _originalSource = c.Source ?? "";
        _originalDaemon = c.Daemon;
        SourcePath = _originalSource;
        IpodModelLabel = c.Ipod?.ModelLabel ?? "(not configured)";
        IpodSerial = c.Ipod?.Serial ?? "";
        FirstSyncMode = c.Daemon?.FirstSyncMode ?? "review";
        SubsequentSyncMode = c.Daemon?.SubsequentSyncMode ?? "auto_apply";
        NotifyOn = c.Daemon?.NotifyOn ?? "all";
    }

    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private string ipodModelLabel = "";
    [ObservableProperty] private string ipodSerial = "";
    [ObservableProperty] private string firstSyncMode = "review";
    [ObservableProperty] private string subsequentSyncMode = "auto_apply";
    [ObservableProperty] private string notifyOn = "all";

    public bool IsSourceDirty => SourcePath != _originalSource;
    public bool IsAnyDaemonFieldDirty =>
        FirstSyncMode != (_originalDaemon?.FirstSyncMode ?? "review") ||
        SubsequentSyncMode != (_originalDaemon?.SubsequentSyncMode ?? "auto_apply") ||
        NotifyOn != (_originalDaemon?.NotifyOn ?? "all");
}
```

- [ ] **Step 2: Replace SettingsGeneralPage.xaml with real bindings**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Page
    x:Class="IpodSync_UI.Views.SettingsGeneralPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <ScrollViewer>
        <StackPanel Padding="24" Spacing="20" MaxWidth="540" HorizontalAlignment="Left">
            <TextBlock Text="General" Style="{ThemeResource TitleTextBlockStyle}" />

            <StackPanel Spacing="8">
                <TextBlock Text="Music source folder" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <Grid ColumnDefinitions="*,Auto" ColumnSpacing="8">
                    <TextBox Grid.Column="0"
                             Text="{x:Bind ViewModel.General.SourcePath, Mode=TwoWay, UpdateSourceTrigger=PropertyChanged}"
                             IsReadOnly="True" />
                    <Button Grid.Column="1" Content="Change…" Click="OnPickSource" />
                </Grid>
            </StackPanel>

            <StackPanel Spacing="8">
                <TextBlock Text="iPod identity" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <TextBlock Text="{x:Bind ViewModel.General.IpodModelLabel, Mode=OneWay}" />
                <TextBlock Text="{x:Bind ViewModel.General.IpodSerial, Mode=OneWay}"
                           Opacity="0.7" FontFamily="Consolas" />
                <Button Content="Re-identify (coming in M5)" IsEnabled="False" />
            </StackPanel>

            <StackPanel Spacing="8">
                <TextBlock Text="Sync mode" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <TextBlock Text="First sync after setup" />
                <ComboBox SelectedValue="{x:Bind ViewModel.General.FirstSyncMode, Mode=TwoWay}"
                          SelectedValuePath="Tag">
                    <ComboBoxItem Content="Review before applying" Tag="review" />
                    <ComboBoxItem Content="Apply automatically" Tag="auto_apply" />
                </ComboBox>
                <TextBlock Text="Subsequent syncs" Margin="0,8,0,0" />
                <ComboBox SelectedValue="{x:Bind ViewModel.General.SubsequentSyncMode, Mode=TwoWay}"
                          SelectedValuePath="Tag">
                    <ComboBoxItem Content="Review before applying" Tag="review" />
                    <ComboBoxItem Content="Apply automatically" Tag="auto_apply" />
                </ComboBox>
            </StackPanel>

            <StackPanel Spacing="8">
                <TextBlock Text="Notifications" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <ComboBox SelectedValue="{x:Bind ViewModel.General.NotifyOn, Mode=TwoWay}"
                          SelectedValuePath="Tag">
                    <ComboBoxItem Content="All sync events" Tag="all" />
                    <ComboBoxItem Content="Errors only" Tag="errors_only" />
                    <ComboBoxItem Content="None" Tag="none" />
                </ComboBox>
            </StackPanel>
        </StackPanel>
    </ScrollViewer>
</Page>
```

- [ ] **Step 3: Add OnPickSource handler in SettingsGeneralPage.xaml.cs**

```csharp
using System;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Windows.Storage.Pickers;

namespace IpodSync_UI.Views;

public sealed partial class SettingsGeneralPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsGeneralPage() { InitializeComponent(); }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }

    private async void OnPickSource(object sender, RoutedEventArgs e)
    {
        if (ViewModel is null) return;
        var picker = new FolderPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, App.WindowHandle);
        picker.FileTypeFilter.Add("*");
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null) ViewModel.General.SourcePath = folder.Path;
    }
}
```

- [ ] **Step 4: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

Expected: 0 errors.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/SettingsGeneralPage.xaml ui-windows/IpodSync.UI/Views/SettingsGeneralPage.xaml.cs ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): SettingsGeneralPage — source + iPod + sync mode + notify level"
```

---

## Task 8: SettingsSchedulePage — interval + autostart toggle stub

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsSchedulePage.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsSchedulePage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\SettingsViewModel.cs` (real `SettingsScheduleViewModel`)

- [ ] **Step 1: Replace SettingsScheduleViewModel stub**

In `SettingsViewModel.cs`, replace the `public partial class SettingsScheduleViewModel` block with:

```csharp
public partial class SettingsScheduleViewModel : ObservableObject
{
    private readonly DaemonSettings? _originalDaemon;

    public SettingsScheduleViewModel(ConfigUpdateEvent c)
    {
        _originalDaemon = c.Daemon;
        ScheduleMinutes = (int)(c.Daemon?.ScheduleMinutes ?? 30);
        AutostartWithWindows = c.Daemon?.AutostartWithWindows ?? false;
    }

    [ObservableProperty] private int scheduleMinutes = 30;
    [ObservableProperty] private bool autostartWithWindows;

    public bool IsAnyDirty =>
        ScheduleMinutes != (int)(_originalDaemon?.ScheduleMinutes ?? 30) ||
        AutostartWithWindows != (_originalDaemon?.AutostartWithWindows ?? false);

    public string ScheduleLabel => ScheduleMinutes == 0
        ? "Disabled"
        : ScheduleMinutes < 60
            ? $"Every {ScheduleMinutes} minutes"
            : $"Every {ScheduleMinutes / 60.0:0.#} hours";

    partial void OnScheduleMinutesChanged(int value) => OnPropertyChanged(nameof(ScheduleLabel));
}
```

- [ ] **Step 2: Replace SettingsSchedulePage.xaml**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Page
    x:Class="IpodSync_UI.Views.SettingsSchedulePage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <ScrollViewer>
        <StackPanel Padding="24" Spacing="24" MaxWidth="540" HorizontalAlignment="Left">
            <TextBlock Text="Schedule" Style="{ThemeResource TitleTextBlockStyle}" />

            <StackPanel Spacing="8">
                <TextBlock Text="Periodic sync interval" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <TextBlock Text="{x:Bind ViewModel.Schedule.ScheduleLabel, Mode=OneWay}" />
                <Slider Minimum="0" Maximum="1440"
                        StepFrequency="5" TickFrequency="60"
                        Value="{x:Bind ViewModel.Schedule.ScheduleMinutes, Mode=TwoWay}" />
                <TextBlock Text="0 disables the periodic schedule; the iPod still syncs on plug-in."
                           Opacity="0.7" TextWrapping="Wrap" />
            </StackPanel>

            <StackPanel Spacing="8">
                <TextBlock Text="Startup" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <ToggleSwitch Header="Launch ipod-sync at Windows sign-in"
                              IsOn="{x:Bind ViewModel.Schedule.AutostartWithWindows, Mode=TwoWay}"
                              IsEnabled="False" />
                <TextBlock Text="Coming in M5: requires StartupTask registration via Package.appxmanifest."
                           Opacity="0.7" TextWrapping="Wrap" />
            </StackPanel>
        </StackPanel>
    </ScrollViewer>
</Page>
```

- [ ] **Step 3: Update SettingsSchedulePage.xaml.cs to wire ViewModel**

```csharp
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsSchedulePage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsSchedulePage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }
}
```

- [ ] **Step 4: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/SettingsSchedulePage.xaml ui-windows/IpodSync.UI/Views/SettingsSchedulePage.xaml.cs ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): SettingsSchedulePage — interval slider + autostart stub"
```

---

## Task 9: SettingsHistoryPage — full history list with details

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsHistoryPage.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsHistoryPage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\SettingsViewModel.cs` (real `SettingsHistoryViewModel` + `HistoryEntryViewModel`)

- [ ] **Step 1: Replace SettingsHistoryViewModel stub + add HistoryEntryViewModel**

In `SettingsViewModel.cs`, replace the stub and add a row VM at the bottom of the file:

```csharp
public partial class SettingsHistoryViewModel : ObservableObject
{
    private readonly DaemonClient _daemon;

    public SettingsHistoryViewModel(DaemonClient daemon, DaemonEventRouter router)
    {
        _daemon = daemon;
        router.HistoryUpdated += OnHistoryUpdated;
        Entries = new System.Collections.ObjectModel.ObservableCollection<HistoryEntryViewModel>();
        _ = LoadAsync();
    }

    public System.Collections.ObjectModel.ObservableCollection<HistoryEntryViewModel> Entries { get; }

    private async Task LoadAsync()
    {
        try { await _daemon.SendAsync(new GetHistoryCommand(Limit: 50)); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"history: load failed: {e}"); }
    }

    private void OnHistoryUpdated(HistoryUpdateEvent e)
    {
        // Dispatcher marshal happens in callers that need UI thread.
        // The collection's CollectionChanged is fired on whatever
        // thread invokes Add; SettingsHistoryPage marshals before
        // calling into this method by binding-dispatcher contract.
        // For safety we dispatch here.
        App.DispatcherQueue.TryEnqueue(() =>
        {
            Entries.Clear();
            // Reverse so newest is first.
            for (int i = e.Entries.Count - 1; i >= 0; i--)
            {
                Entries.Add(new HistoryEntryViewModel(e.Entries[i]));
            }
        });
    }
}

public partial class HistoryEntryViewModel : ObservableObject
{
    public HistoryEntryViewModel(HistoryEntry e)
    {
        Timestamp = e.Timestamp;
        DurationSecs = e.DurationSecs;
        Trigger = e.Trigger;
        Outcome = e.Outcome;
        ErrorMessage = e.ErrorMessage;
        Summary = e.Summary;
    }

    public string Timestamp { get; }
    public ulong DurationSecs { get; }
    public string Trigger { get; }
    public string Outcome { get; }
    public string? ErrorMessage { get; }
    public SyncSummary? Summary { get; }

    public string OutcomeGlyph => Outcome switch
    {
        "ok"      => "✓",  // check
        "error"   => "!",
        "aborted" => "✗",  // cross
        _         => "?",
    };

    public string SummaryText => Summary is null
        ? (ErrorMessage ?? "")
        : $"+{Summary.Add} ~{Summary.Modify} -{Summary.Remove}" +
          (Summary.Skipped > 0 ? $", {Summary.Skipped} skipped" : "");

    public string DurationText => DurationSecs < 60
        ? $"{DurationSecs}s"
        : $"{DurationSecs / 60}m {DurationSecs % 60}s";
}
```

- [ ] **Step 2: Replace SettingsHistoryPage.xaml**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Page
    x:Class="IpodSync_UI.Views.SettingsHistoryPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    xmlns:vm="using:IpodSync_UI.ViewModels">
    <Grid Padding="24" RowDefinitions="Auto,*">
        <TextBlock Grid.Row="0" Text="History" Style="{ThemeResource TitleTextBlockStyle}" Margin="0,0,0,12" />
        <ListView Grid.Row="1"
                  ItemsSource="{x:Bind ViewModel.History.Entries, Mode=OneWay}"
                  SelectionMode="None">
            <ListView.ItemTemplate>
                <DataTemplate x:DataType="vm:HistoryEntryViewModel">
                    <Expander HorizontalAlignment="Stretch" HorizontalContentAlignment="Stretch">
                        <Expander.Header>
                            <Grid ColumnDefinitions="Auto,*,Auto,Auto" ColumnSpacing="12">
                                <TextBlock Grid.Column="0"
                                           Text="{x:Bind OutcomeGlyph}"
                                           FontWeight="Bold"
                                           VerticalAlignment="Center" />
                                <StackPanel Grid.Column="1">
                                    <TextBlock Text="{x:Bind Timestamp}" />
                                    <TextBlock Text="{x:Bind SummaryText}" Opacity="0.7" FontSize="12" />
                                </StackPanel>
                                <TextBlock Grid.Column="2" Text="{x:Bind Trigger}" Opacity="0.6" VerticalAlignment="Center" />
                                <TextBlock Grid.Column="3" Text="{x:Bind DurationText}" Opacity="0.6" VerticalAlignment="Center" />
                            </Grid>
                        </Expander.Header>
                        <StackPanel Spacing="4" Padding="0,8,0,0">
                            <TextBlock Text="{x:Bind ErrorMessage}"
                                       Visibility="{x:Bind ErrorMessage, Converter={StaticResource StringToVisibility}}"
                                       Foreground="{ThemeResource SystemFillColorCriticalBrush}"
                                       TextWrapping="Wrap" />
                        </StackPanel>
                    </Expander>
                </DataTemplate>
            </ListView.ItemTemplate>
        </ListView>
    </Grid>
</Page>
```

If `StringToVisibility` isn't yet registered as a global resource, replace its line with:

```xml
                            <TextBlock Text="{x:Bind ErrorMessage}"
                                       Foreground="{ThemeResource SystemFillColorCriticalBrush}"
                                       TextWrapping="Wrap" />
```

(The empty-string case just renders nothing.)

- [ ] **Step 3: Update SettingsHistoryPage.xaml.cs**

```csharp
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsHistoryPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsHistoryPage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }
}
```

- [ ] **Step 4: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/SettingsHistoryPage.xaml ui-windows/IpodSync.UI/Views/SettingsHistoryPage.xaml.cs ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): SettingsHistoryPage — expandable history list"
```

---

## Task 10: SettingsAboutPage — version + license + log folder

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsAboutPage.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\SettingsAboutPage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\SettingsViewModel.cs` (real `SettingsAboutViewModel`)

- [ ] **Step 1: Replace SettingsAboutViewModel stub**

```csharp
public partial class SettingsAboutViewModel : ObservableObject
{
    public SettingsAboutViewModel()
    {
        var asm = System.Reflection.Assembly.GetExecutingAssembly();
        UiVersion = asm.GetName().Version?.ToString() ?? "unknown";
    }

    public string UiVersion { get; }
    public string LicenseText => "MIT OR Apache-2.0";
    public string GitHubUrl => "https://github.com/itsmichaelwest/ipod-sync";
}
```

- [ ] **Step 2: Replace SettingsAboutPage.xaml**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Page
    x:Class="IpodSync_UI.Views.SettingsAboutPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <ScrollViewer>
        <StackPanel Padding="24" Spacing="16" MaxWidth="540" HorizontalAlignment="Left">
            <TextBlock Text="About" Style="{ThemeResource TitleTextBlockStyle}" />

            <StackPanel Spacing="4">
                <TextBlock Text="ipod-sync" Style="{ThemeResource SubtitleTextBlockStyle}" />
                <TextBlock>
                    <Run Text="UI version: " />
                    <Run Text="{x:Bind ViewModel.About.UiVersion, Mode=OneWay}" FontFamily="Consolas" />
                </TextBlock>
                <TextBlock Text="Windows-native FLAC-to-iPod-Classic sync." Opacity="0.7" />
            </StackPanel>

            <StackPanel Spacing="4">
                <TextBlock Text="License" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <TextBlock Text="{x:Bind ViewModel.About.LicenseText, Mode=OneWay}" />
            </StackPanel>

            <StackPanel Spacing="8">
                <TextBlock Text="Links" Style="{ThemeResource BodyStrongTextBlockStyle}" />
                <HyperlinkButton Content="GitHub repository"
                                 NavigateUri="{x:Bind ViewModel.About.GitHubUrl, Mode=OneWay}" />
                <Button Content="Show log folder" Click="OnShowLogFolder" />
            </StackPanel>
        </StackPanel>
    </ScrollViewer>
</Page>
```

- [ ] **Step 3: Update SettingsAboutPage.xaml.cs**

```csharp
using System;
using System.Diagnostics;
using System.IO;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsAboutPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsAboutPage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }

    private void OnShowLogFolder(object sender, RoutedEventArgs e)
    {
        var path = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "ipod-sync", "logs");
        Directory.CreateDirectory(path);
        try { Process.Start(new ProcessStartInfo("explorer.exe", $"\"{path}\"") { UseShellExecute = true }); }
        catch (Exception ex) { Debug.WriteLine($"about: open log folder failed: {ex.Message}"); }
    }
}
```

- [ ] **Step 4: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml.cs ui-windows/IpodSync.UI/ViewModels/SettingsViewModel.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): SettingsAboutPage — version + license + log-folder shortcut"
```

---

## Task 11: PopoverWindow + PopoverViewModel

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\PopoverWindow.xaml`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\PopoverWindow.xaml.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\PopoverViewModel.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\PopoverViewModelTests.cs`

The flagship visible surface. 360×dynamic, Mica backdrop, frameless, anchored above tray icon, light-dismiss on deactivation. Bound to PopoverViewModel.

- [ ] **Step 1: Write failing VM tests**

Create `ui-windows/IpodSync.UI.Tests/PopoverViewModelTests.cs`:

```csharp
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Xunit;

public class PopoverViewModelTests
{
    private static StatusUpdateEvent Status(string state, bool ipodConnected, HistoryEntry? last = null)
        => new StatusUpdateEvent(state, true, ipodConnected, last, null);

    [Fact]
    public void Initial_status_text_is_offline_when_no_status_received_yet()
    {
        var vm = new PopoverViewModel();
        Assert.Equal("iPod not connected", vm.StatusText);
    }

    [Fact]
    public void Update_with_idle_and_connected_shows_up_to_date()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("idle", ipodConnected: true));
        Assert.StartsWith("Up to date", vm.StatusText);
    }

    [Fact]
    public void Update_with_syncing_shows_syncing()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        Assert.Equal("Syncing iPod…", vm.StatusText);
    }

    [Fact]
    public void Update_with_idle_and_disconnected_shows_offline()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("idle", ipodConnected: false));
        Assert.Equal("iPod not connected", vm.StatusText);
    }

    [Fact]
    public void Update_with_error_history_shows_error_text()
    {
        var vm = new PopoverViewModel();
        var failed = new HistoryEntry("2026-05-25T10:00:00Z", 5, "manual", "error",
            "Source unreachable", null);
        vm.Update(Status("idle", ipodConnected: true, last: failed));
        Assert.Contains("Last sync failed", vm.StatusText);
    }
}
```

- [ ] **Step 2: Run, expect FAIL**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~PopoverViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: FAIL — `PopoverViewModel` doesn't exist.

- [ ] **Step 3: Create PopoverViewModel**

```csharp
using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

public partial class PopoverViewModel : ObservableObject
{
    [ObservableProperty] private string statusText = "iPod not connected";
    [ObservableProperty] private string deviceLabel = "";
    [ObservableProperty] private bool syncing;
    [ObservableProperty] private int progressCurrent;
    [ObservableProperty] private int progressTotal;
    [ObservableProperty] private string currentTrackLabel = "";

    public ObservableCollection<HistoryEntryViewModel> Recent { get; } = new();

    public void Update(StatusUpdateEvent s)
    {
        Syncing = s.State == "syncing";
        if (Syncing)
        {
            StatusText = "Syncing iPod…";
            return;
        }
        if (!s.IpodConnected)
        {
            StatusText = "iPod not connected";
            return;
        }
        // Idle + connected.
        var last = s.LastSync;
        if (last is not null && last.Outcome != "ok")
        {
            StatusText = $"Last sync failed: {last.ErrorMessage ?? "unknown error"}";
        }
        else
        {
            StatusText = last is null
                ? "Up to date · iPod connected"
                : $"Up to date · last sync {RelativeTime(last.Timestamp)}";
        }
    }

    public void ApplyHistory(HistoryUpdateEvent h)
    {
        Recent.Clear();
        // Newest 5.
        var start = Math.Max(0, h.Entries.Count - 5);
        for (int i = h.Entries.Count - 1; i >= start; i--)
        {
            Recent.Add(new HistoryEntryViewModel(h.Entries[i]));
        }
    }

    public void ApplyIpcProgress(IpcEvent evt)
    {
        switch (evt)
        {
            case TrackStartEvent t:
                ProgressCurrent = t.Current;
                ProgressTotal = t.Total;
                CurrentTrackLabel = t.Label;
                break;
        }
    }

    private static string RelativeTime(string rfc3339)
    {
        if (!DateTimeOffset.TryParse(rfc3339, out var dt)) return "recently";
        var delta = DateTimeOffset.UtcNow - dt;
        if (delta.TotalMinutes < 1) return "just now";
        if (delta.TotalMinutes < 60) return $"{(int)delta.TotalMinutes} min ago";
        if (delta.TotalHours < 24) return $"{(int)delta.TotalHours} hr ago";
        return $"{(int)delta.TotalDays} days ago";
    }
}
```

- [ ] **Step 4: Create PopoverWindow.xaml**

```xml
<?xml version="1.0" encoding="utf-8"?>
<Window
    x:Class="IpodSync_UI.Views.PopoverWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    xmlns:vm="using:IpodSync_UI.ViewModels">
    <Grid RowDefinitions="Auto,*,Auto" Padding="16" MinHeight="220" MaxHeight="480">
        <!-- Header -->
        <StackPanel Grid.Row="0" Spacing="4" Margin="0,0,0,12">
            <TextBlock Text="ipod-sync" Style="{ThemeResource SubtitleTextBlockStyle}" />
            <TextBlock Text="{x:Bind ViewModel.StatusText, Mode=OneWay}"
                       TextWrapping="Wrap" />
            <ProgressBar Visibility="{x:Bind ViewModel.Syncing, Mode=OneWay}"
                         Maximum="{x:Bind ViewModel.ProgressTotal, Mode=OneWay}"
                         Value="{x:Bind ViewModel.ProgressCurrent, Mode=OneWay}"
                         Margin="0,8,0,0" />
            <TextBlock Visibility="{x:Bind ViewModel.Syncing, Mode=OneWay}"
                       Text="{x:Bind ViewModel.CurrentTrackLabel, Mode=OneWay}"
                       Opacity="0.7" FontSize="12" TextTrimming="CharacterEllipsis" />
        </StackPanel>

        <!-- Activity feed -->
        <ListView Grid.Row="1"
                  ItemsSource="{x:Bind ViewModel.Recent, Mode=OneWay}"
                  SelectionMode="None">
            <ListView.ItemTemplate>
                <DataTemplate x:DataType="vm:HistoryEntryViewModel">
                    <Grid ColumnDefinitions="Auto,*,Auto" ColumnSpacing="8" Padding="0,4">
                        <TextBlock Grid.Column="0" Text="{x:Bind OutcomeGlyph}" />
                        <StackPanel Grid.Column="1">
                            <TextBlock Text="{x:Bind SummaryText}" />
                            <TextBlock Text="{x:Bind Timestamp}" Opacity="0.6" FontSize="11" />
                        </StackPanel>
                        <TextBlock Grid.Column="2" Text="{x:Bind DurationText}" Opacity="0.6" />
                    </Grid>
                </DataTemplate>
            </ListView.ItemTemplate>
        </ListView>

        <!-- Footer actions -->
        <StackPanel Grid.Row="2" Orientation="Horizontal" Spacing="8" Margin="0,12,0,0">
            <Button Click="OnSyncNow"
                    Style="{ThemeResource AccentButtonStyle}"
                    IsEnabled="{x:Bind ViewModel.Syncing, Mode=OneWay, Converter={StaticResource InverseBoolConverter}, FallbackValue=True}">
                <StackPanel Orientation="Horizontal" Spacing="6">
                    <FontIcon Glyph="&#xE895;" FontSize="14" />
                    <TextBlock Text="Sync now" />
                </StackPanel>
            </Button>
            <Button Click="OnOpenSource" ToolTipService.ToolTip="Open source folder">
                <FontIcon Glyph="&#xE838;" FontSize="14" />
            </Button>
            <Button Click="OnOpenSettings" ToolTipService.ToolTip="Settings">
                <FontIcon Glyph="&#xE713;" FontSize="14" />
            </Button>
        </StackPanel>
    </Grid>
</Window>
```

(If `InverseBoolConverter` doesn't exist yet, remove the `Converter=` part — the Sync Now button being enabled even during sync is a minor UX wart that the daemon's `already_syncing` rejection covers.)

- [ ] **Step 5: Create PopoverWindow.xaml.cs**

```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace IpodSync_UI.Views;

public sealed partial class PopoverWindow : Window
{
    public PopoverViewModel ViewModel { get; }
    private readonly DaemonClient _daemon;
    private readonly string _sourceFolder;

    public PopoverWindow(PopoverViewModel vm, DaemonClient daemon, string sourceFolder)
    {
        ViewModel = vm;
        _daemon = daemon;
        _sourceFolder = sourceFolder;
        InitializeComponent();

        // Frameless + Mica backdrop.
        this.SystemBackdrop = new MicaBackdrop();
        var appWindow = GetAppWindow();
        appWindow.SetPresenter(AppWindowPresenterKind.CompactOverlay);
        appWindow.Resize(new Windows.Graphics.SizeInt32(360, 360));

        Activated += OnActivated;
    }

    private AppWindow GetAppWindow()
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Win32Interop.GetWindowIdFromWindow(hwnd);
        return AppWindow.GetFromWindowId(id);
    }

    /// <summary>
    /// Position the popover above the tray icon. H.NotifyIcon exposes
    /// the icon rect via its desktop coordinates; for M4 we approximate
    /// by anchoring to bottom-right of the primary display work area.
    /// M5 polish: use H.NotifyIcon.GetIconPosition once available.
    /// </summary>
    public void AnchorAboveTray()
    {
        var displayArea = DisplayArea.GetFromPoint(new Windows.Graphics.PointInt32(0, 0),
            DisplayAreaFallback.Primary);
        var work = displayArea.WorkArea;
        var appWindow = GetAppWindow();
        var x = work.X + work.Width - appWindow.Size.Width - 12;
        var y = work.Y + work.Height - appWindow.Size.Height - 12;
        appWindow.Move(new Windows.Graphics.PointInt32(x, y));
    }

    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
        // Light-dismiss: close on deactivate.
        if (args.WindowActivationState == WindowActivationState.Deactivated)
        {
            DispatcherQueue.TryEnqueue(Close);
        }
    }

    private async void OnSyncNow(object sender, RoutedEventArgs e)
    {
        try { await _daemon.SendAsync(new TriggerSyncCommand("manual")); }
        catch (Exception ex) { Debug.WriteLine($"popover: trigger_sync failed: {ex}"); }
    }

    private void OnOpenSource(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(_sourceFolder)) return;
        try
        {
            Process.Start(new ProcessStartInfo("explorer.exe", $"\"{_sourceFolder}\"")
                { UseShellExecute = true });
        }
        catch (Exception ex) { Debug.WriteLine($"popover: open source failed: {ex.Message}"); }
    }

    private void OnOpenSettings(object sender, RoutedEventArgs e)
    {
        App.RequestOpenSettings();
        Close();
    }
}
```

- [ ] **Step 6: Run VM tests, expect PASS**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~PopoverViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: 5/5 pass.

- [ ] **Step 7: Build full solution**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

If `App.RequestOpenSettings()` doesn't exist yet, leave the call stub and let T13 wire it; for build to succeed temporarily, change the call to `Debug.WriteLine("settings open requested");` and add a TODO comment. T13 replaces with the real method.

- [ ] **Step 8: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/PopoverWindow.xaml ui-windows/IpodSync.UI/Views/PopoverWindow.xaml.cs ui-windows/IpodSync.UI/ViewModels/PopoverViewModel.cs ui-windows/IpodSync.UI.Tests/PopoverViewModelTests.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): StatusPopover window + PopoverViewModel"
```

---

## Task 12: TrayIconController left-click + Settings menu item

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\TrayIconController.cs`

Adds `PopoverRequested` and `SettingsRequested` events. App.xaml gains a `SettingsCommand` XAML resource for the menu binding.

- [ ] **Step 1: Add SettingsCommand to App.xaml**

Find the existing `XamlUICommand` resources block in `App.xaml` and add:

```xml
<XamlUICommand x:Key="SettingsCommand" Label="Settings" Description="Settings" />
```

Update the `MenuFlyout` inside `TaskbarIcon.ContextFlyout` to:

```xml
<tb:TaskbarIcon.ContextFlyout>
    <MenuFlyout>
        <MenuFlyoutItem Command="{StaticResource SyncNowCommand}" />
        <MenuFlyoutItem Command="{StaticResource SettingsCommand}" />
        <MenuFlyoutSeparator />
        <MenuFlyoutItem Command="{StaticResource QuitCommand}" />
    </MenuFlyout>
</tb:TaskbarIcon.ContextFlyout>
```

Also add `LeftClickCommand` binding to the `TaskbarIcon`:

Find the existing `<tb:TaskbarIcon>` element. Add the attribute:

```xml
LeftClickCommand="{StaticResource OpenPopoverCommand}"
```

And register the command resource:

```xml
<XamlUICommand x:Key="OpenPopoverCommand" Label="Open" Description="Open status popover" />
```

- [ ] **Step 2: Wire new commands in TrayIconController.cs**

Replace `TrayIconController.cs`:

```csharp
using System;
using System.IO;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;

namespace IpodSync_UI;

public enum TrayState { Idle, Syncing, Error, Offline }

public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private XamlUICommand? _syncNowCommand;
    private XamlUICommand? _settingsCommand;
    private XamlUICommand? _openPopoverCommand;
    private TrayState _state = TrayState.Offline;

    public event Action? QuitRequested;
    public event Action? SyncNowRequested;
    public event Action? SettingsRequested;
    public event Action? PopoverRequested;

    public void Initialize()
    {
        _icon = (TaskbarIcon)Application.Current.Resources["TrayIcon"];
        _quitCommand = (XamlUICommand)Application.Current.Resources["QuitCommand"];
        _syncNowCommand = (XamlUICommand)Application.Current.Resources["SyncNowCommand"];
        _settingsCommand = (XamlUICommand)Application.Current.Resources["SettingsCommand"];
        _openPopoverCommand = (XamlUICommand)Application.Current.Resources["OpenPopoverCommand"];

        _quitCommand.ExecuteRequested += (_, _) => QuitRequested?.Invoke();
        _syncNowCommand.ExecuteRequested += (_, _) => SyncNowRequested?.Invoke();
        _settingsCommand.ExecuteRequested += (_, _) => SettingsRequested?.Invoke();
        _openPopoverCommand.ExecuteRequested += (_, _) => PopoverRequested?.Invoke();

        _icon.ForceCreate();
        SetState(TrayState.Offline, "iPod not connected");
    }

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

- [ ] **Step 3: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/App.xaml ui-windows/IpodSync.UI/TrayIconController.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): tray left-click opens popover + Settings menu item"
```

---

## Task 13: App.xaml.cs uses DaemonEventRouter (kills M3 hack)

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml.cs`

Replaces the M3 `StartTrayEventLoop` task with router subscriptions. Wires popover open / settings open / sync now. Adds `App.RequestOpenSettings()` static method called from popover. Adds `App.LatestConfig` so popover + settings open with current config.

- [ ] **Step 1: Replace App.xaml.cs**

Read the current file first, then replace its body. The new App class:

```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.Notifications;
using IpodSync_UI.ViewModels;
using IpodSync_UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace IpodSync_UI;

public partial class App : Application
{
    public static Window? Window { get; private set; }
    public static IntPtr WindowHandle { get; private set; }
    public static DispatcherQueue DispatcherQueue { get; private set; } = default!;
    public static DaemonClient? Daemon { get; private set; }
    public static DaemonEventRouter? Router { get; private set; }
    public static TrayIconController? Tray { get; private set; }
    public static NotificationService? Notifications { get; private set; }

    /// <summary>Last ConfigUpdate seen from the daemon. Popover + settings read from this.</summary>
    public static ConfigUpdateEvent? LatestConfig { get; private set; }
    /// <summary>Latest StatusUpdate. Used to drive popover initial state.</summary>
    public static StatusUpdateEvent? LatestStatus { get; private set; }
    /// <summary>Latest HistoryUpdate. Used to seed popover activity feed.</summary>
    public static HistoryUpdateEvent? LatestHistory { get; private set; }

    private static PopoverWindow? _popover;
    private static SettingsWindow? _settings;

    public App() { InitializeComponent(); }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        DispatcherQueue = DispatcherQueue.GetForCurrentThread();

        Tray = new TrayIconController();
        Tray.Initialize();
        Tray.QuitRequested += OnQuitRequested;
        Tray.SyncNowRequested += OnSyncNowRequested;
        Tray.SettingsRequested += OnSettingsRequested;
        Tray.PopoverRequested += OnPopoverRequested;

        if (!await IsDaemonRunningAsync())
        {
            SpawnDaemon();
            await Task.Delay(500);
        }

        try { Daemon = await DaemonClient.ConnectAsync(); }
        catch (Exception e)
        {
            Debug.WriteLine($"app: failed to connect to daemon: {e}");
            Tray?.Dispose();
            Environment.Exit(0);
            return;
        }

        // Start the router. All consumers (tray, popover, notifications,
        // wizard) subscribe through it instead of reading the channel
        // directly.
        Router = new DaemonEventRouter(Daemon.Events);
        Router.StatusUpdated += OnStatusUpdated;
        Router.ConfigUpdated += OnConfigUpdated;
        Router.HistoryUpdated += OnHistoryUpdated;
        Router.DeviceConnected += OnDeviceConnected;
        Router.DeviceDisconnected += OnDeviceDisconnected;
        Router.Start();

        // Notification service subscribes to router internally.
        Notifications = new NotificationService(Router,
            getNotifyOn: () => LatestConfig?.Daemon?.NotifyOn ?? "all");
        Notifications.Initialize();

        // Ask for the initial config + status + history.
        await Daemon.SendAsync(new GetConfigCommand());
        await Daemon.SendAsync(new GetStatusCommand());
        await Daemon.SendAsync(new GetHistoryCommand(Limit: 10));

        // Open wizard if config has no iPod identity. The wizard also
        // subscribes to the router (T14) so the channel-exclusivity
        // hack from M3 goes away.
        await Task.Delay(150);  // give the router time to populate LatestConfig
        if (LatestConfig?.Ipod is null)
        {
            ShowWizard();
        }
    }

    private void ShowWizard()
    {
        Window = new WizardWindow();
        WindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(Window);
        Window.Closed += (_, _) =>
        {
            Window = null;
            WindowHandle = IntPtr.Zero;
        };
        Window.Activate();
    }

    private void OnStatusUpdated(StatusUpdateEvent s)
    {
        LatestStatus = s;
        DispatcherQueue.TryEnqueue(() =>
        {
            UpdateTrayFromStatus(s);
            _popover?.ViewModel.Update(s);
        });
    }

    private void OnConfigUpdated(ConfigUpdateEvent c) => LatestConfig = c;
    private void OnHistoryUpdated(HistoryUpdateEvent h)
    {
        LatestHistory = h;
        DispatcherQueue.TryEnqueue(() => _popover?.ViewModel.ApplyHistory(h));
    }
    private void OnDeviceConnected(DeviceConnectedEvent dc)
    {
        DispatcherQueue.TryEnqueue(() =>
            Tray?.SetState(TrayState.Idle, $"iPod connected ({dc.ModelLabel})"));
    }
    private void OnDeviceDisconnected(DeviceDisconnectedEvent _)
    {
        DispatcherQueue.TryEnqueue(() =>
            Tray?.SetState(TrayState.Offline, "iPod not connected"));
    }

    private void UpdateTrayFromStatus(StatusUpdateEvent s)
    {
        if (Tray is null) return;
        var (state, tooltip) = (s.State, s.IpodConnected) switch
        {
            ("syncing", _)   => (TrayState.Syncing, "Syncing iPod…"),
            (_,    true)     => (TrayState.Idle,    "iPod connected · idle"),
            _                => (TrayState.Offline, "iPod not connected"),
        };
        Tray.SetState(state, tooltip);
    }

    private void OnPopoverRequested()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_popover is not null) { _popover.Activate(); return; }
            var vm = new PopoverViewModel();
            if (LatestStatus is not null) vm.Update(LatestStatus);
            if (LatestHistory is not null) vm.ApplyHistory(LatestHistory);
            _popover = new PopoverWindow(vm, Daemon!, LatestConfig?.Source ?? "");
            _popover.Closed += (_, _) => _popover = null;
            _popover.Activate();
            _popover.AnchorAboveTray();
        });
    }

    private void OnSettingsRequested() => RequestOpenSettings();

    public static void RequestOpenSettings()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_settings is not null) { _settings.Activate(); return; }
            if (Daemon is null || Router is null || LatestConfig is null) return;
            var vm = new SettingsViewModel(Daemon, Router, LatestConfig);
            _settings = new SettingsWindow(vm);
            _settings.Closed += (_, _) => _settings = null;
            _settings.Activate();
        });
    }

    private void OnSyncNowRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is null) return;
            try { await Daemon.SendAsync(new TriggerSyncCommand("manual")); }
            catch (Exception e) { Debug.WriteLine($"app: trigger_sync failed: {e}"); }
        });
    }

    private void OnQuitRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is not null)
            {
                try { await Daemon.SendAsync(new ShutdownCommand()); }
                catch { /* daemon may already be dead */ }
                await Daemon.DisposeAsync();
            }
            Router?.Stop();
            Tray?.Dispose();
            Environment.Exit(0);
        });
    }

    private static async Task<bool> IsDaemonRunningAsync()
    {
        try
        {
            using var pipe = new NamedPipeClientStream(
                ".", DaemonClient.PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
            await pipe.ConnectAsync(500);
            return true;
        }
        catch { return false; }
    }

    private static void SpawnDaemon()
    {
        var uiDir = AppContext.BaseDirectory;
        var coreCandidates = new[]
        {
            Path.Combine(uiDir, "ipod-sync.exe"),
            Path.Combine(Directory.GetParent(uiDir)?.FullName ?? "", "ipod-sync.exe"),
        };
        string? corePath = null;
        foreach (var c in coreCandidates)
        {
            if (File.Exists(c)) { corePath = c; break; }
        }
        if (corePath is null)
        {
            Debug.WriteLine("app: cannot find ipod-sync.exe to spawn daemon");
            return;
        }
        var psi = new ProcessStartInfo
        {
            FileName = corePath,
            ArgumentList = { "--daemon" },
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        Process.Start(psi);
    }
}
```

- [ ] **Step 2: Build, expect 0 errors**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/App.xaml.cs
git -C F:\repos\ipod-sync commit -m "refactor(ui-windows): App routes daemon events through DaemonEventRouter (kills M3 wizard-vs-tray hack)"
```

---

## Task 14: WizardWindow + VM use router

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\WizardWindow.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\WizardViewModel.cs`

Drops the wizard's direct channel-read loop in favor of router subscription. The wizard no longer "owns" the channel; multiple consumers (tray, popover) can subscribe concurrently.

- [ ] **Step 1: Rewrite the WaitForDeviceFromDaemonAsync method in WizardWindow.xaml.cs**

Find the existing `WaitForDeviceFromDaemonAsync` and replace it with:

```csharp
private async Task<IpodIdentityCandidate?> WaitForDeviceFromDaemonAsync(CancellationToken ct)
{
    var daemon = App.Daemon;
    var router = App.Router;
    if (daemon is null || router is null) return null;

    var tcs = new TaskCompletionSource<IpodIdentityCandidate?>(
        TaskCreationOptions.RunContinuationsAsynchronously);

    void Handler(DeviceConnectedEvent dc)
    {
        tcs.TrySetResult(new IpodIdentityCandidate(dc.Serial, dc.ModelLabel, dc.Drive));
    }
    router.DeviceConnected += Handler;
    await daemon.SendAsync(new SubscribeDeviceEventsCommand(), ct);

    using var reg = ct.Register(() => tcs.TrySetResult(null));
    try { return await tcs.Task; }
    finally
    {
        router.DeviceConnected -= Handler;
        try { await daemon.SendAsync(new UnsubscribeDeviceEventsCommand()); } catch { }
    }
}
```

The rest of `WizardWindow.xaml.cs` is unchanged. `SendSaveConfigAsync` keeps using `daemon.SendAsync(new SaveConfigCommand(...))`. The `ViewModel.CancelWait()` call from `Closed` still works.

- [ ] **Step 2: WizardViewModel.cs is unchanged**

Sanity check: the VM only sees the func; it doesn't know channels vs router exist. No edit needed there.

- [ ] **Step 3: Run wizard VM tests, expect PASS**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~WizardViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: 6/6 still pass (VM unchanged).

- [ ] **Step 4: Build**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|error CS" | Select-Object -Last 3
```

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs
git -C F:\repos\ipod-sync commit -m "refactor(ui-windows): wizard subscribes to DaemonEventRouter instead of reading channel directly"
```

---

## Task 15: Review-mode flow — bidirectional subprocess IPC pass-through (optional)

**Files:**
- Modify: `F:\repos\ipod-sync\src\daemon\sync_orchestrator.rs`
- Modify: `F:\repos\ipod-sync\src\ipc_daemon.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\runtime.rs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\ReviewPage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml.cs`

**Skippable for M4 gate.** If this slips, `subsequent_sync_mode = "auto_apply"` default means only the very first sync after wizard would need review — and the user can preserve that by setting `first_sync_mode = "auto_apply"` in the wizard or post-wizard via Settings General tab. Document in LEARNINGS as M5 carry-forward if dropped.

The full mechanics: daemon spawns subprocess WITHOUT `--apply` when current sync_mode is "review"; subprocess emits a `review` event; daemon forwards via `SyncEvent`; UI ReviewPage opens, captures user's Apply/DryRun/Quit decision; UI sends a new `ReviewDecision` daemon command; daemon writes the corresponding JSON to the subprocess stdin (which the orchestrator now must hold open beyond the line-read loop).

- [ ] **Step 1: Add ReviewDecision daemon command**

In `src/ipc_daemon.rs`, append to the `DaemonCommand` enum:

```rust
    ReviewDecision {
        decision: ReviewDecisionPayload,
    },
```

Add the nested type:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecisionPayload {
    Apply { no_delete: bool },
    DryRun,
    Quit,
}
```

- [ ] **Step 2: Change orchestrator to hold stdin + accept decision channel**

Modify `sync_orchestrator::run`'s signature to also take a `mpsc::Receiver<ReviewDecisionPayload>`:

```rust
pub async fn run(
    exe: PathBuf,
    drive: String,
    apply_immediately: bool,
    review_rx: mpsc::Receiver<ReviewDecisionPayload>,
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<OrchestratorOutcome> {
    // ... build_command, but conditionally include --apply only if
    // apply_immediately is true ...
    // ... in the read loop, when a "review" line arrives, await the
    // next message on review_rx, then write the corresponding
    // {"type":"review_decision","decision":{...}} to stdin.
}
```

Update `build_command` to accept the flag:

```rust
pub fn build_command(exe: &std::path::Path, drive: &str, apply_immediately: bool) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode").arg("--ipod").arg(drive);
    if apply_immediately { cmd.arg("--apply"); }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    cmd
}
```

(Adjust all callers to pass the bool.)

- [ ] **Step 3: Runtime decides apply vs review based on config + history**

In `runtime.rs`'s `start_sync_session`, before spawning the orchestrator task, decide:

```rust
let apply_immediately = if history.read().is_empty() {
    // first sync ever
    config_file::load(config_path).ok().flatten()
        .and_then(|c| c.daemon).map(|d| matches!(d.first_sync_mode, SyncMode::AutoApply))
        .unwrap_or(false)  // default review on first sync
} else {
    config_file::load(config_path).ok().flatten()
        .and_then(|c| c.daemon).map(|d| matches!(d.subsequent_sync_mode, SyncMode::AutoApply))
        .unwrap_or(true)  // default auto-apply on subsequent
};
```

Create a `mpsc::Sender<ReviewDecisionPayload>` per sync; store it in DaemonState's `SyncSession` so `ReviewDecision` IPC commands can route to the active sync.

This adds complexity. Estimated 4–6 hours of careful work. If subagent reports blockers, fall back to dropping this task and documenting in LEARNINGS.

- [ ] **Step 4: ReviewPage gets re-wired (existing M1 code)**

The existing `ReviewPage.xaml`/`ReviewViewModel` from M1 already render review summaries. App.xaml.cs's router gets a new `IpcEventReceived` handler that, when it sees `ReviewEvent`, opens a `ReviewPage`-hosting window (or routes into the popover's main pane temporarily). On user decision, send the new `ReviewDecisionCommand` via daemon.

- [ ] **Step 5: User-validate or commit-and-flag**

Run the user smoke. If review flow works end-to-end, commit. If not, revert changes and add a LEARNINGS note. Either way, post outcome.

```powershell
git -C F:\repos\ipod-sync add src/daemon/sync_orchestrator.rs src/ipc_daemon.rs src/daemon/runtime.rs ui-windows/IpodSync.UI/App.xaml.cs ui-windows/IpodSync.UI/Views/ReviewPage.xaml.cs
git -C F:\repos\ipod-sync commit -m "feat(daemon+ui-windows): bidirectional review-mode flow (opt: M4 stretch)"
```

---

## Task 16: User-driven M4 smoke + gate tag

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md`

- [ ] **Step 1: Build release**

```powershell
cargo build --release 2>&1 | Select-Object -Last 3
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Release 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

- [ ] **Step 2: Smoke checklist (operator presence required)**

1. Kill all `ipod-sync.exe` and `IpodSync.UI.exe` processes.
2. Launch UI. If wizard hasn't been completed, do it now.
3. Tray icon should be Offline if no iPod connected, or Idle within ~2s of iPod plug-in.
4. **Left-click tray** → popover opens (Mica backdrop, anchored to corner, shows current state + recent history + Sync Now / Settings / Open source folder buttons). Click outside → popover closes.
5. **Plug in iPod** → toast appears ("Syncing iPod…"), tray → Syncing, popover (re-open) shows progress.
6. **Sync completes** → "Sync complete: +N -M tracks" toast → tray → Idle → popover (re-open) shows the new history entry at top.
7. Set `notify_on = "errors_only"` via Settings → save → trigger a sync → no toast on success.
8. Set `notify_on = "none"` → sync runs silently (no toast at all).
9. **Right-click tray → Settings** → SettingsWindow opens. Navigate all 4 tabs (General / Schedule / History / About). Change source folder path → Save → window closes. Re-open Settings, confirm change persisted.
10. **History tab** shows all entries with expandable details.
11. **About tab** "Show log folder" opens Explorer at `%LOCALAPPDATA%\ipod-sync\logs`.
12. **Quit from tray** → all processes gone within 5s.

Note any deviations.

- [ ] **Step 3: Append result to LEARNINGS.md**

```markdown

## Phase 6 M4 gate — <PASS or FAIL> (2026-05-25)

E2E smoke against real iPod + real source library:

- **Popover left-click:** <result>
- **Toasts honour notify_on:** <result>
- **Settings persist:** <result>
- **History tab renders:** <result>
- **Review-mode flow (T15):** <PASS / DEFERRED-TO-M5 / FAIL>

### Issues found

- <issue 1, or "none">

### Follow-ups for M5

- Anchor popover precisely above tray icon (M4 uses corner-of-display approximation)
- Real artwork for tray-syncing.ico, tray-error.ico (M4 still has placeholders)
- StartupTask wiring for autostart-with-Windows toggle (M4 ships disabled)
- Re-identify iPod button in Settings General (M4 ships disabled)
- (If T15 deferred) Review-mode flow
```

- [ ] **Step 4: Commit + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 6 M4 gate result + M5 follow-ups"
```

If PASS:

```powershell
git -C F:\repos\ipod-sync tag -a phase-6-m4-complete -m "Phase 6 M4 complete: status popover + settings + history + toasts.

What ships:
- DaemonEventRouter (typed event fan-out; kills M3 wizard-vs-tray hack)
- StatusPopover (Mica backdrop, anchored to tray, light-dismiss, activity feed)
- SettingsWindow with 4 tabs (General / Schedule / History / About)
- NotificationService (toasts driven by StatusUpdate transitions, honours notify_on)
- Tray left-click opens popover, Settings menu item, all menu actions wired
- Wizard refactored to subscribe through router (no more channel exclusivity)
- Daemon-side: SyncOrchestrator forwards subprocess events as SyncEvent envelopes;
  snapshot StatusUpdate on every new client connection; RFC3339 history timestamps

Known limitations (LEARNINGS):
- Popover anchored at corner of display (M5: precise tray-icon-relative)
- tray-syncing.ico / tray-error.ico are placeholders (M5: real artwork)
- Autostart-with-Windows toggle disabled (M5: StartupTask wiring)
- Re-identify iPod button disabled (M5)
- (If applicable) Review-mode flow deferred to M5
"
```

---

## Self-review notes (inline)

- **Spec coverage** (§10 M4): NotificationService → T5; StatusPopover with activity feed → T11; SettingsWindow 4 tabs → T6–T10; Open source folder → T11 (popover footer); ReviewPage → T15. The M3 carry-forwards (router, snapshot, sync event forward, RFC3339) cover T1–T4. All §10 M4 deliverables mapped.
- **Placeholder scan:** no `TBD` / `implement later`. The "Re-identify (coming in M5)" button label is intentional UI copy, not a plan placeholder — the button is wired as disabled with explanatory text. Same for "Autostart-with-Windows toggle (M5)".
- **Type consistency:** `DaemonEventRouter` event signatures consistent across T4 definition + T5/T9/T11/T13/T14 consumers. `PopoverViewModel.Update(StatusUpdateEvent)` + `ApplyHistory(HistoryUpdateEvent)` + `ApplyIpcProgress(IpcEvent)` defined in T11, called from T13. `SettingsViewModel(DaemonClient, DaemonEventRouter, ConfigUpdateEvent)` constructor consistent T6 + T13. `App.RequestOpenSettings()` declared in T13, called from T11.
- **Scope check:** M4 only. M5 (autostart, dark-mode, custom iPod icons, MSIX hardening, code signing, accessibility audit) gets its own plan. Review-mode (T15) is optional within M4 per the spec being permissive about deferral.
- **Wave-race avoidance:** T1, T2, T3 all touch `src/daemon/runtime.rs`. Plan recommends running them sequentially as a single Rust-runtime-bundle agent rather than parallel. T7–T10 share `SettingsViewModel.cs`; plan recommends splitting into two sub-waves (T5+T11+T7, then T8+T9+T10) to keep edits sequential. T12–T14 all touch `App.xaml.cs` / `App.xaml` / `TrayIconController.cs`; plan explicitly states sequential execution.
- **M3 fix carry-forward:** the `App.xaml.cs` rewrite in T13 explicitly removes the wizard-channel-exclusivity hack (commented in M3 as `// (M4: introduce a real event router ...)`). Verified by the T13 code containing no `StartTrayEventLoop` method — replaced by `Router.StatusUpdated += OnStatusUpdated` etc.
- **Async contract for VMs:** `PopoverViewModel.Update` and `SettingsViewModel.SaveAsync` are explicit about thread safety. UI thread marshaling happens in App.xaml.cs handlers (via `DispatcherQueue.TryEnqueue`) before calling VM mutation methods. Tests don't need to dispatch because xUnit runs sync-context-free.
