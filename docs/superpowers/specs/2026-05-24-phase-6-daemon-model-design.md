# Phase 6 — Daemon-model design (supersedes 2026-05-24 WinUI-app spec)

**Status:** Draft pending user review (2026-05-24).
**Supersedes:** `docs/superpowers/specs/2026-05-24-phase-6-winui-app.md` (M1 stays; M2-M4 in that doc are replaced by M2-M5 here).
**Inherits:** M1 work shipped under commits `94227f5` through `c48d1c9` (IPC protocol v1.0.0, CoreProcess subprocess management, ReviewViewModel + ProgressViewModel, bundled Rust binary). Most M1 code stays; the per-spawn `CoreProcess` becomes a per-connection `DaemonClient` against a named pipe.

## 1. Why this redesign

M1 built a "launch app, click Start" sync flow. During Phase 6 review the user reframed the MVP as a **background sync daemon** that auto-syncs when the iPod is plugged in — the OneDrive / Syncthing pattern, not the iTunes pattern. The deferred iTunes-Lite library browser is dropped from scope entirely; the project's center of gravity is "hands-off auto-sync with a thin status UI."

The user also surfaced a cross-platform concern: future macOS (SwiftUI) and Linux (GTK/Adwaita) frontends should reuse maximum Rust code and write minimal per-platform UI code. The original M2-M4 plan had a C# daemon owning device detection, scheduling, history, and state — each future port would reimplement all of that in its own language. This redesign moves that logic into a long-lived Rust daemon process, leaving each UI port as a thin presentation layer.

Net consequence: M1 binary and protocol are still useful, but the orchestration model inverts. The C# UI no longer spawns sync subprocesses directly — it connects to a daemon over a named pipe, and the daemon spawns the sync subprocesses.

## 2. Architecture overview

```
┌──────────────────────────────────────────────────────────────────────┐
│  ipod-sync.exe --daemon  (long-lived, ONE Rust binary, ONE process)  │
│                                                                      │
│   ┌──────────────────────────────────────────────────────────────┐   │
│   │  Daemon core (cross-platform Rust)                           │   │
│   │   DeviceWatcher trait                                        │   │
│   │     • #[cfg(windows)] WindowsWatcher (SetupDi)               │   │
│   │     • #[cfg(macos)]   IokitWatcher (later)                   │   │
│   │     • #[cfg(linux)]   UdevWatcher (later)                    │   │
│   │   SyncScheduler  (tokio interval timer, configurable)        │   │
│   │   StateMachine   (Idle | Syncing — drops concurrent triggers)│   │
│   │   HistoryService (%LOCALAPPDATA%\ipod-sync\history.json)     │   │
│   │   ConfigService  (config.toml load/save/watch)               │   │
│   │   SyncOrchestrator                                           │   │
│   │     • spawns `ipod-sync --ipc-mode --apply` per sync         │   │
│   │     • consumes events from sync subprocess (M1 IPC protocol) │   │
│   │     • broadcasts events to all connected UI clients          │   │
│   │  IPC server                                                  │   │
│   │   Windows: named pipe \\.\pipe\ipod-sync                     │   │
│   │   Mac/Linux: Unix socket ~/.ipod-sync/daemon.sock            │   │
│   │   wire format: same JSON envelope as M1 + new daemon verbs   │   │
│   │   multi-instance / multi-client                              │   │
│   └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
                            ▲                                ▲
                            │ named pipe                     │ named pipe
                            │                                │
              ┌─────────────┴────────┐         ┌─────────────┴────────┐
              │ IpodSync.UI (WinUI 3)│         │ (future) macOS / GTK │
              │  • Tray icon         │         │  UI ports — same     │
              │  • Wizard window     │         │  daemon, same IPC    │
              │  • Settings window   │         └──────────────────────┘
              │  • Status popover    │
              │  • Notifications     │
              │  Thin: UI + IPC only │
              └──────────────────────┘

                       Daemon spawns per sync:
                  ┌─────────────────────────────────────────┐
                  │  ipod-sync.exe --ipc-mode --apply       │
                  │  (fresh subprocess, M1 IPC over stdio)  │
                  └─────────────────────────────────────────┘
                                  │
                                  ▼
                   libgpod FFI → iTunesDB on iPod
```

**One Rust binary, three runtime modes** (`ipod-sync.exe`):
- Default (no flag): TUI mode. Existing Phase 0-3.z behavior. Standalone — does not talk to daemon.
- `--ipc-mode`: per-sync subprocess. Emits JSON events on stdout, accepts JSON commands on stdin. Used by daemon to run actual sync work. Unchanged from M1.
- `--daemon`: long-lived background process. Owns device watching, scheduling, state, history, IPC server. Spawns `--ipc-mode` subprocesses to do sync work.

**Daemon process lifecycle:**
- UI launches daemon on first run if no daemon is detected (named-pipe probe).
- Daemon survives UI close. UI Quit-from-tray cleanly shuts it down.
- Autostart-with-Windows (opt-in via settings) launches the daemon at login via Windows StartupTask — UI follows when user opens it.
- Daemon stays alive as long as it's configured (always-on, so plug-in detection works). No auto-exit-on-idle in MVP; consumes ~30-50 MB resident.

## 3. Component breakdown

### Rust daemon (new code)

| Component | Responsibility |
|---|---|
| `daemon::main` | Entry point for `--daemon` mode. Wires services + runs Tokio runtime + IPC server loop. |
| `device_watcher` | Trait `DeviceWatcher { fn watch(&self) -> mpsc::Receiver<DeviceEvent>; }` with `#[cfg]`-gated impls per OS. Windows impl uses `windows-rs` SetupDi notifications. |
| `sync_scheduler` | Wraps `tokio::time::interval`. Fires `Tick` events at configurable interval. Disabled when interval = 0. |
| `state_machine` | Enum `DaemonState { Idle, Syncing(SyncSession) }`. Centralizes "should this trigger fire?" decision. Drops concurrent triggers during Syncing. |
| `sync_orchestrator` | Spawns `Command::new("ipod-sync").args(["--ipc-mode", "--apply", "--ipod", drive])`. Reads child stdout, parses JSON, broadcasts to subscribed UI clients. Force-kills child after 5s on graceful shutdown timeout (Phase 3.z pattern). |
| `history_service` | Append-only writes to `history.json` (atomic via temp + rename). Cap at 50 entries (oldest evicted). Corrupt file → renamed `.bak-{ts}`, fresh file. |
| `config_service` | Extends existing `config_file` module. New `[daemon]` and `[ipod]` sections (schema in §6). `#[serde(default)]` on all new fields for back-compat with Phase 3.z manifests. |
| `ipc_server` | Multi-instance named-pipe server. Accepts connections, broadcasts daemon events to all clients, dispatches client commands to orchestrator/scheduler/state. |

### C# UI (mostly refactored from M1)

| Component | Status | Notes |
|---|---|---|
| `DaemonClient` (NEW, replaces `CoreProcess`) | New | Connects to `\\.\pipe\ipod-sync`. Same wire format as M1 IPC. Reconnect logic: 3 attempts at 1s/2s/4s backoff. |
| `CoreLocator` | Removed | Daemon's path is known (sibling to UI exe via MSIX bundling). Path-lookup logic from M1 deleted. |
| `IpcEvent` / `IpcCommand` records | Kept | M1's polymorphic records. Add new commands: `GetStatusCommand`, `TriggerSyncCommand`, `SaveConfigCommand`, `ShutdownCommand`. Add new events: `StatusUpdate`, `DeviceConnected`, `DeviceDisconnected`. |
| `AppController` | Refactored | Was per-sync orchestrator. Becomes daemon-connection manager. Subscribes to events, routes to ViewModels. |
| `TrayIconController` | NEW | Wraps `H.NotifyIcon.WinUI`. Maps daemon state → tray icon variant (idle/syncing/error/offline). |
| `WizardWindow` + VM | NEW | 3-step wizard (source / iPod identification / confirm). Subscribes to daemon's `DeviceConnected` for step 2. |
| `SettingsWindow` + VMs | NEW | NavigationView with 4 tabs (General / Schedule / History / About). Loads config from daemon, sends `SaveConfigCommand` on save. |
| `StatusPopover` + VM | NEW | 360×300 window, Mica backdrop, anchored to tray icon. Shows current status + recent history. |
| `NotificationService` | NEW | Wraps `AppNotificationManager`. Fires toasts per `notify_on` setting. |
| `MainPage` (M1's Start screen) | Removed | Daemon-mode UI has no "Start" button — the tray icon IS the entry point. |
| `ReviewPage` + `ReviewViewModel` | Kept | Shown when sync mode is "Review" (first-sync default or user-configured). Otherwise skipped. |
| `ProgressPage` + `ProgressViewModel` | Kept | Shown when user opens UI during an active sync OR after Review's Apply. Otherwise sync runs silently with tray icon + toast. |

## 4. User flows

State machine for the daemon:

```
              ┌──────────────────────────────────────┐
              │                                      │
trigger from  │                                      │
watcher /     ▼                                      │
timer /       ┌────────────┐    sync subprocess      │
manual    ┌──▶│   IDLE     │    finishes (ok or err) │
          │   └─────┬──────┘                         │
          │         │                                │
          │         │ accept trigger,                │
   any    │         │ spawn ipod-sync --apply,       │
 trigger  │         │ broadcast events to UIs        │
 while    │         ▼                                │
 busy =   │   ┌────────────┐                         │
 DROP     └───│  SYNCING   │─────────────────────────┘
              └────────────┘
```

### Flow 1: Fresh install → first-launch wizard

1. User installs MSIX. Nothing starts automatically.
2. User opens **ipod-sync** from Start menu.
3. UI launches → probes named pipe → no daemon → spawns `ipod-sync.exe --daemon` as detached child.
4. UI connects to daemon's named pipe.
5. UI sends `GetStatusCommand`. Daemon responds with `StatusUpdate { configured: false }`.
6. UI opens WizardWindow. Tray icon shows "needs setup" state.
7. **Step 1 (Source):** native folder picker.
8. **Step 2 (iPod):** UI subscribes to daemon's device-event stream. "Plug in your iPod now..." spinner. Daemon's watcher fires `IpodConnected(serial)` (immediately if already plugged in, or when user plugs in). UI shows "Detected **iPod Classic 7G · 160 GB**". User confirms.
9. **Step 3 (Confirm):** summary. User clicks Finish. UI sends `SaveConfigCommand { source, ipod_serial }`. Daemon writes config.toml.
10. Wizard closes. UI hides to tray. Tray icon → idle. Daemon arms scheduler + device watcher for the configured iPod.

### Flow 2: iPod plugged in → auto-sync (the main event)

1. Daemon's `DeviceWatcher` fires `IpodConnected(serial="EXAMPLE...")`.
2. State machine: serial matches configured iPod, state is Idle, accept trigger.
3. Daemon polls `<drive>\iPod_Control\` for readiness (timeout 30s, 1s poll interval).
4. Transition to Syncing. Spawn `ipod-sync --ipc-mode --apply --ipod <drive>`.
5. Daemon forwards events from subprocess to all connected UI clients.
6. NotificationService fires "Sync started" toast (if `notify_on` includes start events).
7. Tray icon → syncing. Tooltip "Syncing X tracks...". Updates on each `TrackDone` event.
8. Subprocess sends `Finish { success: true }`, exits.
9. Daemon writes history entry. Transitions to Idle. Fires "Sync complete: +N -M tracks" toast.
10. Tray icon → idle. Popover (if open) refreshes status.

### Flow 3: User clicks "Sync Now" from tray menu

1. User right-clicks tray → "Sync Now".
2. UI sends `TriggerSyncCommand { source: "manual" }`.
3. Daemon: if Syncing → responds `AlreadySyncing`, UI shows brief "Already syncing..." toast. If iPod not connected → responds `NoIpod`, UI shows "iPod not connected" toast. If Idle + iPod connected → same as Flow 2 step 3 onward, with trigger source recorded as "manual" in history.

### Flow 4: User opens Settings → changes a preference

1. User left-clicks tray → status popover opens. Clicks Settings (gear icon).
2. UI opens SettingsWindow with tabs. Loads current config via `GetConfigCommand`.
3. User toggles "Autostart with Windows" on.
4. User clicks Save. UI sends `SaveConfigCommand { autostart_with_windows: true }`.
5. Daemon writes config.toml AND creates the Windows StartupTask entry. Responds OK.
6. UI shows "Settings saved" toast. Window stays open (user closes when ready).

### Flow 5: User Quits from tray menu

1. User right-clicks tray → "Quit".
2. UI shows confirm dialog "Quit ipod-sync? Auto-sync will stop until next launch."
3. User confirms. UI sends `ShutdownCommand`.
4. Daemon: if syncing, attempts graceful subprocess shutdown (cancel command + 5s wait + force-kill). Writes final history entry. Stops watchers + scheduler. Closes IPC server. Exits.
5. UI: closes pipe connection. Disposes tray icon. Exits.

### Edge cases

| Case | Handling |
|---|---|
| Source unreachable mid-sync | Sync subprocess fails → error event → daemon writes history entry → toast "Source unreachable — will retry on next trigger". No user prompt. |
| Unknown iPod plugged in (serial ≠ configured) | Daemon logs + ignores trigger (MVP silent). M5 polish: toast "Unknown iPod — open ipod-sync to configure". |
| Plug-in trigger fires during active sync | State machine drops second trigger. No user-visible disruption. Logged in history as `coalesced`. |
| Daemon dies unexpectedly | UI detects pipe disconnect → reconnect with backoff (1s, 2s, 4s) → on give-up, tray notification "ipod-sync daemon stopped. Click to restart." Restart spawns fresh daemon. |
| Sync subprocess hangs | Daemon's 5s bounded-join force-kills (Phase 3.z pattern, applied at daemon ↔ subprocess boundary). Synthetic error event. Daemon stays alive. |
| Sync subprocess panics / non-zero exit without Finish | Daemon broadcasts synthetic `error` event "Sync exited unexpectedly (code N). See log." Toast. History entry. |
| Core emits Prompt/Form mid-sync (ffmpeg missing, etc.) | Daemon force-cancels sync. Broadcasts the prompt message as an error event. Toast "Sync needs attention — click for details." History entry records the prompt text. |
| Per-track failure in auto-mode | Sync subprocess auto-skips the track and continues (Phase 3.z "Skip" semantics applied without user prompting in `--apply` mode). Summary toast reports "+12 tracks, 1 skipped (errors)." Skipped tracks listed in history detail. |
| iPod ejected mid-sync (cable pulled) | libgpod write fails → error event → toast "Sync interrupted — eject properly next time." iPod's iTunesDB partially updated; next sync's diff repairs it. |
| iPod fails to mount within 30s | Daemon gives up polling. Toast "iPod failed to mount." No sync attempted. |
| Multiple iPods simultaneously | First one matching configured serial gets synced. Others silently ignored. Single-iPod-per-config is a documented MVP limit. |
| config.toml corrupt | Daemon falls back to defaults + exits with log. UI on next launch detects no daemon, restarts wizard with "previous config corrupt" banner. |
| history.json corrupt | Renamed to `.bak-{ts}`, fresh file. Warning logged. User sees fewer history entries; not blocking. |
| Permission denied on history.json | Sync still completes; history not persisted. Toast "Couldn't write history — check %LOCALAPPDATA% permissions." |

## 5. Visual design

### Tray icon — 4 states

| State | Visual | Tooltip |
|---|---|---|
| Idle | Blue rounded square + sync-arrow glyph | `ipod-sync · idle` |
| Syncing | Blue + animated spinner glyph | `Syncing 12/45 tracks` |
| Error | Red + exclamation glyph | `Last sync failed · click for details` |
| Offline (no iPod) | Grey + sync-arrow (dimmed) | `iPod not connected` |

Right-click any state → menu: **Status**, **Sync Now**, **Settings**, **Quit**.
Left-click → opens Status popover.

### Status popover — Windows 11 file-provider flyout style

Dimensions: 360 × dynamic (max 480). Mica backdrop. Rounded 8px corners. Anchored above tray icon. Light-dismiss (click outside closes). Esc closes. No system chrome.

**Idle state layout:**
- Header (~56px): app icon + name + status text ("Up to date · iPod Classic 7G connected").
- Activity feed: 3-5 most recent history entries. Each row = status icon (✓ / !) + summary + relative timestamp + duration. "Show all history →" link footer.
- Footer (~52px): Sync now (accent button, primary action) + Settings (gear icon) + Open source folder (folder icon).

**Syncing state layout:**
- Header: spinner icon + "Syncing iPod..." + "12 of 45 tracks · ETA 2 min" + thin progress bar.
- Current section: current track label (truncated to fit one line).
- Activity feed: collapsed (1 entry max — last completed sync).
- Footer: Cancel sync (destructive red, with confirm dialog) + Settings.

Respects system light/dark theme via WinUI 3 `RequestedTheme`.

### Wizard — 3 steps

Single window, 640×480. Progress dots at top (●○○, ●●○, ●●●). Cancel button always visible (closes wizard, deletes incomplete config).

- **Step 1 (Source):** "Pick your music library" + folder-path field + Browse button.
- **Step 2 (iPod):** "Plug in your iPod" + spinner. When `IpodConnected` event arrives, replaces spinner with green checkmark card: "✓ Detected **iPod Classic 7G** · 160 GB · Black · serial EXAMPLE1234".
- **Step 3 (Confirm):** summary of source + iPod + default sync settings. Finish button (accent).

### Settings window

700×500. NavigationView (left sidebar) with 4 tabs:

- **General**: source path (with Change button), iPod identity (with Re-identify button), sync mode dropdown, notification level dropdown.
- **Schedule**: periodic interval slider (0 = disabled, 5-1440 minutes), Autostart-with-Windows toggle.
- **History**: scrollable list of past syncs (timestamp, duration, +N/-M counts, success/error icon). Click entry → detail panel with full event log excerpt. "Clear history" button.
- **About**: version (UI + core), license, GitHub link, "Show log folder" button.

Save / Cancel buttons in window footer.

### iPod iconography

- **M2-M3 (Approach A):** generic music-device glyph (Fluent Icons set, free MIT) for the visual mark. Model identity surfaced as text everywhere ("iPod Classic 7G · 160 GB · Black").
- **M5 (Approach B):** designer adds custom wireframe icons for major generations (Classic 1G-7G, Nano, Touch, Shuffle, Mini). Single illustrator, consistent style. SVG sprite shipped with MSIX. Maps libgpod model enum → icon asset.

Approach D (Apple's iTunes assets) is explicitly **not** an option — copyright.

## 6. Configuration schema

Extends existing `config.toml` (Phase 3.z). All new fields `#[serde(default)]` for back-compat.

```toml
# Existing (Phase 3.z)
source = '\\HOST\data\media\music'
encoder = "ffmpeg"
passthrough_wav = false
force_reencode = false

# NEW: daemon settings
[daemon]
enabled = true                       # default true; user can disable daemon mode
autostart_with_windows = false       # opt-in via settings, never via wizard
first_sync_mode = "review"           # "review" | "auto-apply"
subsequent_sync_mode = "auto-apply"  # "review" | "auto-apply"
schedule_minutes = 30                # 0 disables periodic
notify_on = "all"                    # "all" | "errors_only" | "none"

# NEW: iPod identity (set by wizard, read by device watcher for match)
[ipod]
serial = "EXAMPLE1234"
model_label = "iPod Classic 7G · 160 GB"  # cached for display; re-derived on Re-identify
```

History file: `%LOCALAPPDATA%\ipod-sync\history.json`. Max 50 entries.

```json
{
  "version": 1,
  "entries": [
    {
      "timestamp": "2026-05-24T10:30:00Z",
      "duration_secs": 45,
      "trigger": "plug-in",
      "outcome": "ok",
      "summary": {"add": 12, "modify": 3, "remove": 0, "unchanged": 1260, "skipped": 0}
    },
    {
      "timestamp": "2026-05-24T08:15:00Z",
      "duration_secs": 12,
      "trigger": "manual",
      "outcome": "error",
      "error_message": "Source unreachable at \\\\HOST\\data\\media\\music",
      "summary": null
    }
  ]
}
```

## 7. IPC protocol (M1 v1.0.0 + daemon extensions → v1.1.0)

The wire format from M1 is unchanged: newline-delimited JSON envelopes with a `type` discriminator (snake_case). The IPC SERVER inside the daemon speaks the same protocol over named pipes that the M1 stdio IPC spoke between UI and per-sync subprocess. The daemon also continues to use M1 v1.0.0 over stdio to talk to its spawned sync subprocesses.

### New events (daemon → UI)

| Event | Payload | Emitted when |
|---|---|---|
| `status_update` | `{state: "idle"|"syncing", configured: bool, ipod_connected: bool, last_sync: HistoryEntry?, next_scheduled: ISO8601?}` | UI sends `get_status` OR daemon state changes |
| `device_connected` | `{serial: string, model_label: string, drive: string}` | Device watcher fires for a configured-matching iPod, OR for any iPod during wizard subscription |
| `device_disconnected` | `{serial: string}` | Device watcher fires on disconnect |
| (all M1 events) | (M1 payloads) | Forwarded from sync subprocess during a sync |

### New commands (UI → daemon)

| Command | Payload | Daemon response |
|---|---|---|
| `get_status` | `{}` | Replies with `status_update` event |
| `get_config` | `{}` | Replies with `config_update` event carrying current settings |
| `save_config` | `{changes: {...}}` | Writes config.toml, applies live (re-arms scheduler if interval changed, registers StartupTask if autostart toggled). Replies with `config_update`. |
| `trigger_sync` | `{source: "manual"|"scheduled"}` | If accepted, transitions to Syncing + spawns subprocess. If rejected, replies with `error` event (`already_syncing` or `no_ipod`). |
| `get_history` | `{limit: int}` | Replies with `history_update` event carrying last N entries |
| `subscribe_device_events` | `{}` | Wizard uses this; daemon starts forwarding all `device_connected` events (not just configured-matching ones) to this client |
| `unsubscribe_device_events` | `{}` | Wizard cleanup |
| `shutdown` | `{}` | Triggers graceful daemon shutdown (drains current sync if any, then exits) |

### Process lifecycle additions

- **Hello** from M1 stays: daemon emits `hello { protocol_version: "1.1.0", core_version: ... }` on every new client connection. UI validates `protocol_version.starts_with("1.")`.
- **Subscribe semantics:** by default a client receives status + sync-related events (Header / Summary / Review / Prompt / Form / TrackStart / TrackDone / Log / Error / Finish). Device events are opt-in via `subscribe_device_events` (used only by the wizard).
- **Multi-client:** each client gets independent subscriptions. Broadcasting to N clients = N writes. No per-client filtering beyond the device-event opt-in.

### Pipe paths

- Windows: `\\.\pipe\ipod-sync` (multi-instance via `PipeOptions::WriteThrough | PipeOptions::Asynchronous`, max 8 concurrent clients).
- macOS / Linux (future): `~/.ipod-sync/daemon.sock` (Unix domain socket).

## 8. Error handling

See §4 edge cases table for the per-failure handling matrix.

User-facing patterns:
- **Silent (log only)** — unrecoverable internals (e.g., temp file cleanup failed).
- **Toast** — sync-related events (start, complete, error). Configurable per `notify_on`.
- **Tray icon → error state** — persistent issues (daemon can't start, last sync failed, unknown iPod detected).
- **Popover error banner** — replaces "Up to date" header text with "Last sync failed · click for details" when icon is in error state.
- **Settings → History tab** — full error message + truncated log excerpt for each failed sync.

Phase 3.z patterns inherited:
- Source-change safeguard (manifest stores `last_source_root`).
- libgpod Play Counts.bak race auto-retry (one attempt before bubbling).
- Walker SMB transient-error retries with exponential backoff.
- Bounded-time join with force-exit (Phase 3.z fix `466dbe5`) applied at daemon ↔ subprocess boundary.

Phase 6 additions:
- Per-track failure in auto-mode skips track + records in summary (option B from §4 brainstorm).
- UI reconnect-with-backoff for daemon disconnect (1s, 2s, 4s, give-up).
- Daemon multi-client broadcast with no per-client filtering.

## 9. Testing strategy

| Level | Tools | Scope |
|---|---|---|
| Unit (Rust) | `cargo test` | DaemonState machine, HistoryService, ConfigService, DeviceWatcher trait + mock impl, SyncOrchestrator with stub subprocess, IPC server multi-client broadcast. Existing 103 tests stay. |
| Unit (C#) | `dotnet test` + xUnit | DaemonClient (replaces CoreProcess) with mock pipe, reconnect backoff, Settings VM, Wizard VM, StatusPopover VM. Existing 41 tests stay (drop CoreLocator's 4). |
| Integration (Rust) | `cargo test --test daemon_integration` | Stub sync binary in workspace; daemon spawns it; verify event flow + graceful + crash + hang scenarios. |
| Integration (C# ↔ Rust) | `dotnet test` skipped in CI | C# DaemonClient connects to real daemon process running against mock device watcher. Gated for environments with the Rust binary. |
| E2E smoke | User-driven | Real iPod + real source library. Same gate format as Phase 3.z / M1. Results in LEARNINGS.md. |

Test seams: trait-based DI for DeviceWatcher (`#[cfg]` for production impls); closure-based `SpawnFn` for SyncOrchestrator; Path injection for History/Config services; IPipeStream abstraction for DaemonClient.

Coverage discipline: every state transition and every IPC message has at least one test. No 100% coverage target.

## 10. Milestone breakdown

### M2 — Daemon foundation + first-launch wizard (~1.5 weeks)

- Rust `--daemon` mode boots, parses CLI, runs Tokio runtime, listens on `\\.\pipe\ipod-sync`.
- ConfigService and HistoryService implemented + unit tests.
- DaemonState machine implemented + unit tests.
- C# DaemonClient replaces CoreProcess (most M1 code stays — pipe transport swap).
- C# UI app starts hidden in tray if config exists; opens WizardWindow if not.
- WizardWindow flows (steps 1-3); wizard subscribes to device events via `subscribe_device_events`.
- TrayIconController with H.NotifyIcon + right-click menu (Quit only — other items added M3).
- MainPage from M1 removed.
- Tag `phase-6-m2-complete`.

### M3 — Device detection + auto-sync (~1.5 weeks)

- DeviceWatcher trait + Windows impl (windows-rs SetupDi notifications). Emits IpodConnected/Disconnected events scoped by serial match.
- SyncScheduler with configurable interval.
- SyncOrchestrator spawns `ipod-sync --ipc-mode --apply --ipod <drive>` subprocesses, forwards events to subscribed clients.
- Per-track skip semantics in auto-mode (apply Phase 3.z Skip behavior without user prompt when `--apply` is set).
- Tray icon state updates (idle / syncing / error / offline).
- "Sync Now" tray menu item wired.
- iPod model label + generic icon (Approach A) surfaced in wizard and tray tooltip.
- Tag `phase-6-m3-complete`.

### M4 — Toast notifications + status popover + history (~1 week)

- NotificationService with AppNotificationManager. Per `notify_on` setting.
- StatusPopover window with Mica backdrop, anchored to tray.
- Activity feed in popover loads from HistoryService via `get_history` command.
- SettingsWindow with General + Schedule + History + About tabs.
- "Open source folder" footer action.
- ReviewPage shown when sync mode is "Review" (first-sync default or user-configured).
- Tag `phase-6-m4-complete`.

### M5 — Polish + distribution (~1.5 weeks)

- Autostart-with-Windows registers StartupTask via Package.appxmanifest. Toggle in Settings.
- Dark mode pass + accessibility audit (AutomationProperties.Name on all interactive elements, keyboard navigation, focus order).
- Custom per-generation iPod icons (Approach B). 6-8 SVGs. Map libgpod model enum to assets.
- MSIX packaging hardening: code signing strategy decision (signed via self-cert for sideload, OR EV cert for SmartScreen acceptance).
- Settings window polish (animations, validation, error states).
- Error-state pass: every failure path produces a useful user-visible message and a history entry.
- User-driven E2E gate (7+ scenarios).
- Tag `phase-6-m5-complete` → effectively `phase-6-complete`.

## 11. Risks and open questions

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| WinUI 3 hidden-on-startup is finicky; H.NotifyIcon docs sparse for this | Medium | Low | 1-2 days of trial-and-error budgeted in M2 |
| `windows-rs` SetupDi notifications have undocumented quirks (e.g., already-present devices not emitted on subscribe) | Medium | Medium | Enumerate present devices on startup separately; subscribe for future events |
| Named pipe multi-client race conditions in Rust | Low | Medium | Use `tokio::net::windows::named_pipe::NamedPipeServer` (battle-tested) + per-client task |
| Daemon auto-exit-on-idle is tempting but breaks plug-in detection | Documented | N/A | Daemon always-on while configured; ~30-50 MB resident; revisit only if it becomes a real complaint |
| Concurrent sync detection works for daemon-side triggers but not if user runs `ipod-sync` standalone TUI while daemon is also configured | Low | Low | Document as unsupported; TUI mode is opt-in alternative for users who don't want the daemon |
| MSIX code signing — EV cert costs $400+/yr | Documented | Medium | M5 decision: self-cert for v1 (users sideload + accept SmartScreen warning), EV cert only if there's real distribution demand |
| Cross-platform device-watcher backends are real per-OS work | High (later) | Medium (later) | macOS / Linux ports are explicitly out of scope for Phase 6 MVP; architecture supports them without redesign |

Open question for M2 start:
- **Daemon binary name and CLI subcommand convention** — `ipod-sync --daemon` (current spec) vs `ipod-sync-daemon` (separate binary) vs `ipod-sync daemon` (subcommand). Recommendation: stick with `--daemon` flag on the existing binary (no new Cargo target, no new build artifact, consistent with `--ipc-mode` from M1).

## 12. Out of scope (deferred)

- Multi-iPod support (one configured iPod per install)
- Source-library file-watching (Lidarr-triggered syncs)
- iTunes-Lite library browser (explicitly dropped — see §1)
- Cross-platform frontends (macOS SwiftUI, Linux GTK) — architecture supports; not in MVP
- Cloud config sync between user's devices
- Auto-update from GitHub releases (Phase 7+)
- Playback / track preview
- Smart playlists / play-count writeback (Phase 5a, separately tracked)
- m3u playlist sync (Phase 4, deferred separately)
- Library browser with sorting / filtering / search

## 13. Acceptance criteria (Phase 6 MVP gate)

1. User installs MSIX → opens app → wizard runs → completes Steps 1-3 → window minimizes to tray.
2. User unplugs and re-plugs iPod → toast "Syncing iPod..." appears → sync runs in background → toast "Sync complete: +N -M tracks" appears at end. iPod's iTunesDB reflects the sync.
3. User right-clicks tray → menu shows Status, Sync Now, Settings, Quit. All items work.
4. User left-clicks tray → popover shows last-sync time + activity feed + Sync Now button. Click outside dismisses.
5. User clicks Settings → SettingsWindow opens with 4 tabs. Changes to source / schedule / autostart persist.
6. User closes any window via X → daemon stays running in tray. Re-opening window restores state.
7. User Quits from tray → confirms intent → daemon exits cleanly within 5s. Process gone.
8. First sync after install honors `first_sync_mode = "review"` default — ReviewPage appears for user confirmation.
9. Per-track failure during auto-sync → track skipped, sync continues, toast reports "+N tracks, 1 skipped". History entry records skipped track.
10. Source unreachable → toast "Source unreachable — will retry on next trigger". No daemon crash. History entry with error.

## Self-review notes (inline)

- **Placeholder scan:** no `TBD` / `TODO` blocks. The "open question" at end of §11 is genuinely deferred to implementation start, not a gap in the design.
- **Internal consistency:** §2 architecture, §3 components, §7 IPC protocol, and §10 milestones all reference the same daemon binary + same client model. State machine in §4 matches the one used in §3 (StateMachine component) and §7 (transitions in `status_update` events).
- **Scope check:** single implementation plan is M2 (foundation + wizard). M3-M5 are sketched here, will get their own implementation plans when M2 is done and we know what we learned.
- **Ambiguity check:** §7 IPC protocol explicitly enumerates new events + commands; wire format defers to existing v1.0.0 docs. §6 config schema is explicit about field types + defaults. Per-track skip behavior in §4 / §8 is explicitly the "Phase 3.z Skip semantics applied without user prompt when `--apply` is set" — a known existing behavior, not a new design.
