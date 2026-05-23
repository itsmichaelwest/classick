# Phase 6 M2: Daemon Foundation + First-Launch Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the long-lived Rust daemon process (`ipod-sync --daemon`) with config + history + state-machine + named-pipe IPC server, refactor the C# UI from M1's per-sync subprocess model to a daemon-connected client, and ship a 3-step first-launch wizard. After M2, the user can install the app, complete setup, and have the daemon settled in the tray — but auto-sync triggers (M3) and notifications/popover (M4) come later.

**Architecture:** Single Rust binary (`ipod-sync.exe`) gains a `--daemon` mode alongside existing TUI / `--ipc-mode`. Daemon owns ConfigService, HistoryService, DaemonState machine, and a multi-instance named-pipe server. C# UI's `CoreProcess` (M1 subprocess spawner) is replaced by `DaemonClient` connecting to `\\.\pipe\ipod-sync`. New wizard window subscribes to daemon's device-event stream for iPod identification. UI hides to tray after setup; daemon stays running. Per spec `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

**Tech Stack:** Rust stable (x86_64-pc-windows-msvc) + Tokio + `tokio::net::windows::named_pipe`. .NET 10 + WinUI 3 + `H.NotifyIcon.WinUI` (new dep). Existing M1 wire format (`docs/ipc-protocol.md` v1.0.0) extended to v1.1.0 with new daemon-specific events/commands.

**Plan scope:** M2 only. M3 (DeviceWatcher real impl + auto-sync), M4 (notifications + popover + settings + history view), M5 (polish + distribution) get their own plans after M2 ships.

**Gate:** end-to-end manual smoke per spec §13 acceptance criteria #1-#7 — install → wizard runs → window minimizes to tray → daemon stays running → re-open via tray-click → all menu items work → Quit cleanly exits. Auto-sync from plug-in is OUT OF M2 SCOPE (M3); wizard step 2 uses a polling fallback for iPod identification.

---

## File Structure

```
F:\repos\ipod-sync\
├── Cargo.toml                                    (modify: add tokio dep)
├── src\
│   ├── cli.rs                                    (modify: --daemon flag)
│   ├── main.rs                                   (modify: branch on --daemon)
│   ├── lib.rs                                    (modify: pub mod daemon; pub mod ipc_daemon)
│   ├── config_file.rs                            (modify: [daemon] + [ipod] schema additions)
│   ├── ipc_daemon.rs                             (NEW: daemon-side IPC wire types)
│   ├── ipod\device.rs                            (modify: scan_for_ipod helper)
│   └── daemon\                                   (NEW module dir)
│       ├── mod.rs                                (re-exports)
│       ├── history.rs                            (NEW: HistoryService + HistoryEntry)
│       ├── state.rs                              (NEW: DaemonState enum + transitions)
│       └── ipc_server.rs                         (NEW: named-pipe server + dispatch)
├── ui-windows\
│   ├── IpodSync.UI.Core\
│   │   ├── Ipc\
│   │   │   ├── DaemonClient.cs                   (NEW: replaces CoreProcess)
│   │   │   ├── DaemonEvent.cs                    (NEW: status_update, device_*, etc.)
│   │   │   ├── DaemonCommand.cs                  (NEW: get_status, save_config, etc.)
│   │   │   └── CoreProcess.cs                    (DELETE)
│   │   └── CoreLocator.cs                        (DELETE)
│   ├── IpodSync.UI\
│   │   ├── App.xaml.cs                           (modify: hidden startup + daemon probe)
│   │   ├── MainPage.xaml + .xaml.cs              (DELETE)
│   │   ├── AppController.cs                      (modify: daemon-client lifecycle)
│   │   ├── TrayIconController.cs                 (NEW: H.NotifyIcon wrapper)
│   │   ├── Views\WizardWindow.xaml + .xaml.cs    (NEW: 3-step wizard window)
│   │   ├── ViewModels\WizardViewModel.cs         (NEW)
│   │   ├── Views\ReviewPage.* + Views\ProgressPage.*  (KEEP from M1; used in M4)
│   │   └── ViewModels\ReviewViewModel.cs + ProgressViewModel.cs  (KEEP)
│   ├── IpodSync.UI.Tests\
│   │   ├── DaemonClientTests.cs                  (NEW)
│   │   ├── WizardViewModelTests.cs               (NEW)
│   │   └── CoreLocatorTests.cs                   (DELETE)
└── docs\
    └── ipc-protocol.md                           (modify: append v1.1.0 daemon verbs)
```

### Module responsibility delta

- **`src/cli.rs`** — new `--daemon` flag (mutually exclusive with `--ipc-mode`).
- **`src/main.rs`** — branches on `cli.daemon`: if true, runs daemon main loop; else existing TUI/ipc-mode behavior unchanged.
- **`src/config_file.rs`** — gains `DaemonSettings` + `IpodIdentity` types. All new fields `#[serde(default)]` so Phase 3.z configs load cleanly.
- **`src/daemon/history.rs`** — `HistoryService` writes `history.json` atomically, caps at 50 entries, recovers from corruption by renaming and starting fresh.
- **`src/daemon/state.rs`** — `DaemonState` enum (Idle / Syncing); `try_transition_to_syncing()` is the centralized concurrent-trigger drop point.
- **`src/daemon/ipc_server.rs`** — multi-instance named-pipe server on `\\.\pipe\ipod-sync`. Spawns per-connection Tokio task. Broadcasts daemon events to all connected clients; dispatches client commands to services.
- **`src/ipc_daemon.rs`** — new wire types for daemon-side IPC (`status_update`, `get_status`, `save_config`, etc.). Distinct module from M1's `src/ipc.rs` because the daemon protocol is a superset; future macOS/Linux frontends use the same module.
- **`src/ipod/device.rs`** — adds `scan_for_ipod()` that enumerates drive letters looking for `iPod_Control\Device\SysInfo`, reads serial via existing `read_firewire_guid`. Used by wizard's polling fallback (M2) and as the basis for M3's `DeviceWatcher`.
- **`ui-windows/IpodSync.UI.Core/Ipc/DaemonClient.cs`** — connects to named pipe, sends `IpcCommand`/`DaemonCommand`, receives `IpcEvent`/`DaemonEvent`. Reconnect-with-backoff (3 attempts: 1s/2s/4s). API surface mirrors `CoreProcess` so consumers refactor minimally.
- **`ui-windows/IpodSync.UI/App.xaml.cs`** — startup probes daemon, spawns one if absent, opens wizard if config missing, else hides to tray.
- **`ui-windows/IpodSync.UI/AppController.cs`** — was per-sync orchestrator. Becomes daemon-connection manager. Subscribes to status events. Routes to ViewModels.
- **`ui-windows/IpodSync.UI/TrayIconController.cs`** — wraps `H.NotifyIcon.WinUI`. M2 ships idle/offline states + Quit menu. Syncing/error states + Sync Now + Settings menu items land in M3/M4.
- **`ui-windows/IpodSync.UI/Views/WizardWindow.xaml`** — single window, 3 steps via TabView or Frame navigation. Bound to WizardViewModel.

---

## Task 1: Cargo dependencies for daemon mode

**Files:**
- Modify: `F:\repos\ipod-sync\Cargo.toml`

Tokio is needed for async runtime + named-pipe server. Add minimal feature set to keep binary size reasonable.

- [ ] **Step 1: Add tokio dep**

Edit `Cargo.toml`, add to `[dependencies]`:

```toml
tokio = { version = "1.40", features = ["rt-multi-thread", "macros", "net", "io-util", "time", "sync", "process"] }
```

- [ ] **Step 2: Verify build still works**

```powershell
cargo build --release 2>&1 | Select-Object -Last 3
cargo test --lib 2>&1 | Select-String "test result"
```

Expected: clean build (warns about vendor/refalac/ are fine), 103 tests still pass.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add Cargo.toml Cargo.lock
git -C F:\repos\ipod-sync commit -m "build: add tokio dep for daemon mode"
```

---

## Task 2: Config schema additions for daemon + iPod identity

**Files:**
- Modify: `F:\repos\ipod-sync\src\config_file.rs`

Adds `[daemon]` and `[ipod]` sections per spec §6. All new fields `#[serde(default)]` so Phase 3.z manifests load cleanly without daemon settings.

- [ ] **Step 1: Write the back-compat failing test**

Add to `src/config_file.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn config_without_daemon_section_loads_with_defaults() {
    let toml_text = r#"
source = '\\HOST\share\music'
encoder = "ffmpeg"
"#;
    let cfg: PersistedConfig = toml::from_str(toml_text).expect("parse");
    let daemon = cfg.daemon.expect("daemon section synthesized via default");
    assert!(daemon.enabled);
    assert!(!daemon.autostart_with_windows);
    assert_eq!(daemon.first_sync_mode, SyncMode::Review);
    assert_eq!(daemon.subsequent_sync_mode, SyncMode::AutoApply);
    assert_eq!(daemon.schedule_minutes, 30);
    assert_eq!(daemon.notify_on, NotifyLevel::All);
    assert!(cfg.ipod.is_none());  // unconfigured
}
```

- [ ] **Step 2: Run test to verify it fails**

```powershell
cargo test --lib config_file::tests::config_without_daemon_section_loads_with_defaults 2>&1 | Select-Object -Last 10
```

Expected: FAIL with "no variant or field" or "no field named `daemon`".

- [ ] **Step 3: Implement DaemonSettings + IpodIdentity types**

In `src/config_file.rs`, add (above the `PersistedConfig` struct):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Review,
    AutoApply,
}

impl Default for SyncMode {
    fn default() -> Self { SyncMode::Review }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyLevel {
    All,
    ErrorsOnly,
    None,
}

impl Default for NotifyLevel {
    fn default() -> Self { NotifyLevel::All }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub autostart_with_windows: bool,
    #[serde(default = "default_review_mode")]
    pub first_sync_mode: SyncMode,
    #[serde(default = "default_auto_apply_mode")]
    pub subsequent_sync_mode: SyncMode,
    #[serde(default = "default_schedule_minutes")]
    pub schedule_minutes: u32,
    #[serde(default)]
    pub notify_on: NotifyLevel,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            autostart_with_windows: false,
            first_sync_mode: SyncMode::Review,
            subsequent_sync_mode: SyncMode::AutoApply,
            schedule_minutes: 30,
            notify_on: NotifyLevel::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpodIdentity {
    pub serial: String,
    #[serde(default)]
    pub model_label: String,
}

fn default_true() -> bool { true }
fn default_review_mode() -> SyncMode { SyncMode::Review }
fn default_auto_apply_mode() -> SyncMode { SyncMode::AutoApply }
fn default_schedule_minutes() -> u32 { 30 }
```

Modify the `PersistedConfig` struct to include the new fields:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedConfig {
    // ... existing fields (source, encoder, passthrough_wav, force_reencode) ...

    #[serde(default = "default_daemon_settings")]
    pub daemon: Option<DaemonSettings>,
    #[serde(default)]
    pub ipod: Option<IpodIdentity>,
}

fn default_daemon_settings() -> Option<DaemonSettings> { Some(DaemonSettings::default()) }
```

- [ ] **Step 4: Run test to verify it passes**

```powershell
cargo test --lib config_file::tests::config_without_daemon_section_loads_with_defaults 2>&1 | Select-Object -Last 5
```

Expected: PASS.

- [ ] **Step 5: Add round-trip test**

```rust
#[test]
fn config_with_daemon_and_ipod_round_trips() {
    let cfg = PersistedConfig {
        source: Some(PathBuf::from(r"\\HOST\share\music")),
        encoder: Some(EncoderChoice::Refalac),
        passthrough_wav: Some(false),
        force_reencode: Some(false),
        daemon: Some(DaemonSettings {
            enabled: true,
            autostart_with_windows: true,
            first_sync_mode: SyncMode::AutoApply,
            subsequent_sync_mode: SyncMode::AutoApply,
            schedule_minutes: 60,
            notify_on: NotifyLevel::ErrorsOnly,
        }),
        ipod: Some(IpodIdentity {
            serial: "EXAMPLE1234".to_string(),
            model_label: "iPod Classic 7G".to_string(),
        }),
    };
    let toml_text = toml::to_string(&cfg).expect("serialize");
    let parsed: PersistedConfig = toml::from_str(&toml_text).expect("round-trip");
    assert_eq!(cfg, parsed);
}
```

```powershell
cargo test --lib config_file::tests::config_with_daemon_and_ipod_round_trips 2>&1 | Select-Object -Last 5
```

Expected: PASS.

- [ ] **Step 6: Verify all existing tests still pass**

```powershell
cargo test --lib 2>&1 | Select-String "test result"
```

Expected: 105+ tests pass (103 baseline + 2 new).

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src/config_file.rs
git -C F:\repos\ipod-sync commit -m "feat(config): add daemon + ipod schema sections (back-compat preserved)"
```

---

## Task 3: HistoryService

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\mod.rs`
- Create: `F:\repos\ipod-sync\src\daemon\history.rs`
- Modify: `F:\repos\ipod-sync\src\lib.rs` (add `pub mod daemon;`)

Atomic-write append-only history file capped at 50 entries. Corrupt file = rename + start fresh.

- [ ] **Step 1: Wire the new module into lib.rs**

Edit `src/lib.rs`, add in alphabetical order:

```rust
pub mod daemon;
```

Create `src/daemon/mod.rs`:

```rust
//! Long-lived daemon mode (`ipod-sync --daemon`): device watching,
//! scheduling, sync orchestration, history persistence, and IPC server.
//! See `docs/superpowers/specs/2026-05-24-phase-6-daemon-model-design.md`.

pub mod history;
```

- [ ] **Step 2: Write failing tests for HistoryService**

Create `src/daemon/history.rs`:

```rust
//! Persistent log of past sync operations. Backed by a small JSON file
//! at `%LOCALAPPDATA%\ipod-sync\history.json`. Cap at 50 entries (oldest
//! evicted). Corrupt file is renamed to `.bak-{unix_secs}` and a fresh
//! file is started.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTrigger {
    PlugIn,
    Scheduled,
    Manual,
    Coalesced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcome {
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSummary {
    pub add: usize,
    pub modify: usize,
    pub remove: usize,
    pub unchanged: usize,
    #[serde(default)]
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,           // RFC3339
    pub duration_secs: u64,
    pub trigger: SyncTrigger,
    pub outcome: SyncOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SyncSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryFile {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
}

impl Default for HistoryFile {
    fn default() -> Self { Self { version: 1, entries: Vec::new() } }
}

const MAX_ENTRIES: usize = 50;

pub struct HistoryService {
    path: PathBuf,
}

impl HistoryService {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    /// Read all entries. Returns empty list if the file is missing OR
    /// corrupt (and renames corrupt file to .bak-{ts}).
    pub fn read(&self) -> Vec<HistoryEntry> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<HistoryFile>(&text) {
                Ok(f) => f.entries,
                Err(_) => {
                    self.rename_corrupt_file();
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    /// Append an entry. Caps total at MAX_ENTRIES (oldest evicted).
    /// Atomic via tmp + rename.
    pub fn append(&self, entry: HistoryEntry) -> Result<()> {
        let mut existing = self.read();
        existing.push(entry);
        let start = existing.len().saturating_sub(MAX_ENTRIES);
        let trimmed: Vec<_> = existing.into_iter().skip(start).collect();
        let file = HistoryFile { version: 1, entries: trimmed };

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&file).context("serialize history")?;
        std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    fn rename_corrupt_file(&self) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bak = self.path.with_extension(format!("json.bak-{ts}"));
        let _ = std::fs::rename(&self.path, &bak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ipod-sync-history-test-{}-{}.json",
            name, std::process::id()))
    }

    fn make_entry(ts: &str, outcome: SyncOutcome) -> HistoryEntry {
        HistoryEntry {
            timestamp: ts.to_string(),
            duration_secs: 5,
            trigger: SyncTrigger::Manual,
            outcome,
            error_message: None,
            summary: Some(SyncSummary { add: 1, modify: 0, remove: 0, unchanged: 0, skipped: 0 }),
        }
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let p = tmp_path("read-missing");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p);
        assert!(svc.read().is_empty());
    }

    #[test]
    fn append_then_read_round_trips() {
        let p = tmp_path("append-read");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        svc.append(make_entry("2026-05-24T10:00:00Z", SyncOutcome::Ok)).unwrap();
        let entries = svc.read();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "2026-05-24T10:00:00Z");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn append_caps_at_50_evicting_oldest() {
        let p = tmp_path("cap-50");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        for i in 0..55 {
            let ts = format!("2026-05-24T10:{:02}:00Z", i);
            svc.append(make_entry(&ts, SyncOutcome::Ok)).unwrap();
        }
        let entries = svc.read();
        assert_eq!(entries.len(), 50);
        // Oldest 5 should have been evicted; first remaining timestamp is :05
        assert_eq!(entries[0].timestamp, "2026-05-24T10:05:00Z");
        assert_eq!(entries[49].timestamp, "2026-05-24T10:54:00Z");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_renamed_and_fresh_start() {
        let p = tmp_path("corrupt");
        std::fs::write(&p, "this isn't JSON at all { ]").unwrap();
        let svc = HistoryService::new(p.clone());
        let entries = svc.read();
        assert!(entries.is_empty(), "corrupt file should read as empty");
        // Original file should be gone (renamed to .bak-{ts})
        assert!(!p.exists(), "corrupt original should have been renamed away");
        // Cleanup .bak files
        if let Some(dir) = p.parent() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(p.file_name().unwrap().to_string_lossy().as_ref())
                    && name.contains(".bak-")
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run tests, expect them to pass**

```powershell
cargo test --lib daemon::history 2>&1 | Select-Object -Last 10
```

Expected: 4 tests pass.

- [ ] **Step 4: Verify full suite still passes**

```powershell
cargo test --lib 2>&1 | Select-String "test result"
```

Expected: 109+ tests pass (105 + 4 new).

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src/lib.rs src/daemon/mod.rs src/daemon/history.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): HistoryService with atomic write + 50-entry cap + corrupt recovery"
```

---

## Task 4: DaemonState machine

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\state.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs` (add `pub mod state;`)

Centralizes Idle/Syncing transitions + concurrent-trigger drop policy.

- [ ] **Step 1: Wire module + write failing tests**

Edit `src/daemon/mod.rs`:

```rust
pub mod history;
pub mod state;
```

Create `src/daemon/state.rs`:

```rust
//! Daemon state machine: tracks whether a sync is currently in flight
//! and centralizes the "should this trigger be accepted?" policy. Per
//! spec §4: concurrent triggers during Syncing are dropped (not queued).

use crate::daemon::history::SyncTrigger;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    Idle,
    Syncing(SyncSession),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSession {
    pub started_at_unix_secs: u64,
    pub trigger: SyncTrigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerOutcome {
    Accepted,
    DroppedAlreadySyncing,
}

pub struct StateMachine {
    state: DaemonState,
}

impl StateMachine {
    pub fn new() -> Self { Self { state: DaemonState::Idle } }

    pub fn state(&self) -> &DaemonState { &self.state }

    pub fn is_idle(&self) -> bool { matches!(self.state, DaemonState::Idle) }

    /// Try to accept a sync trigger. Returns `Accepted` if state was Idle
    /// (and transitions to Syncing); returns `DroppedAlreadySyncing` if
    /// state was Syncing (state unchanged).
    pub fn try_start_sync(&mut self, trigger: SyncTrigger) -> TriggerOutcome {
        match &self.state {
            DaemonState::Idle => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                self.state = DaemonState::Syncing(SyncSession {
                    started_at_unix_secs: now,
                    trigger,
                });
                TriggerOutcome::Accepted
            }
            DaemonState::Syncing(_) => TriggerOutcome::DroppedAlreadySyncing,
        }
    }

    /// Called when the sync subprocess finishes (success or failure).
    /// Returns the SyncSession that was active.
    pub fn finish_sync(&mut self) -> Option<SyncSession> {
        match std::mem::replace(&mut self.state, DaemonState::Idle) {
            DaemonState::Syncing(s) => Some(s),
            DaemonState::Idle => None,
        }
    }
}

impl Default for StateMachine {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_idle() {
        let sm = StateMachine::new();
        assert!(sm.is_idle());
    }

    #[test]
    fn try_start_accepts_when_idle() {
        let mut sm = StateMachine::new();
        let result = sm.try_start_sync(SyncTrigger::PlugIn);
        assert_eq!(result, TriggerOutcome::Accepted);
        assert!(matches!(sm.state(), DaemonState::Syncing(_)));
    }

    #[test]
    fn try_start_drops_when_syncing() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::PlugIn);
        let result = sm.try_start_sync(SyncTrigger::Scheduled);
        assert_eq!(result, TriggerOutcome::DroppedAlreadySyncing);
        // Still in Syncing, still with the original trigger.
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.trigger, SyncTrigger::PlugIn);
        } else {
            panic!("expected Syncing");
        }
    }

    #[test]
    fn finish_returns_session_and_resets_to_idle() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::Manual);
        let session = sm.finish_sync().expect("session present");
        assert_eq!(session.trigger, SyncTrigger::Manual);
        assert!(sm.is_idle());
    }

    #[test]
    fn finish_from_idle_returns_none() {
        let mut sm = StateMachine::new();
        assert!(sm.finish_sync().is_none());
        assert!(sm.is_idle());
    }
}
```

- [ ] **Step 2: Run tests, expect pass**

```powershell
cargo test --lib daemon::state 2>&1 | Select-Object -Last 10
```

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/mod.rs src/daemon/state.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): DaemonState machine with drop-concurrent semantics"
```

---

## Task 5: scan_for_ipod helper (wizard polling fallback)

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\device.rs`

Adds drive-letter enumeration + iPod detection used by the wizard's "Plug in your iPod" step. M3 will use this same logic to seed the DeviceWatcher.

- [ ] **Step 1: Inspect existing code for context**

Read current `src/ipod/device.rs` to understand the `read_firewire_guid` function and existing patterns. The new helper builds on it.

- [ ] **Step 2: Write the failing test**

Add to `src/ipod/device.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn scan_for_ipod_returns_none_when_no_drives_match() {
    // Create a tmpdir that doesn't have iPod_Control. scan_for_ipod's
    // injectable variant should return None.
    let tmp = std::env::temp_dir().join(format!("ipod-scan-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let result = scan_drive_for_ipod(&tmp);
    assert!(result.is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn scan_drive_for_ipod_detects_serial_when_sysinfo_present() {
    let tmp = std::env::temp_dir().join(format!("ipod-scan-found-test-{}", std::process::id()));
    let sysinfo_dir = tmp.join("iPod_Control").join("Device");
    std::fs::create_dir_all(&sysinfo_dir).unwrap();
    // Minimal valid SysInfo with FirewireGuid
    std::fs::write(
        sysinfo_dir.join("SysInfo"),
        "FirewireGuid: 0xEXAMPLE1234\nModelNumStr: xMB029\n",
    ).unwrap();
    let detected = scan_drive_for_ipod(&tmp).expect("should detect");
    assert_eq!(detected.serial, "0xEXAMPLE1234");
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 3: Run, expect fail (function not defined)**

```powershell
cargo test --lib ipod::device::tests::scan 2>&1 | Select-Object -Last 5
```

Expected: FAIL — `scan_drive_for_ipod` not found.

- [ ] **Step 4: Implement scan_for_ipod + scan_drive_for_ipod**

Add to `src/ipod/device.rs`:

```rust
/// Detected iPod identity returned by drive-scan helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedIpod {
    pub serial: String,
    pub model_label: String,
    pub drive: String,    // e.g. "G:\\"
}

/// Scan all Windows drive letters for an iPod (presence of
/// iPod_Control\Device\SysInfo). Returns the first match.
///
/// Used by the M2 wizard's polling fallback. M3's DeviceWatcher emits
/// per-device events and bypasses this scan, but reuses
/// `scan_drive_for_ipod` for the SysInfo read on each notification.
pub fn scan_for_ipod() -> Option<DetectedIpod> {
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:\\", letter as char);
        let drive_path = Path::new(&drive);
        if !drive_path.exists() { continue; }
        if let Some(detected) = scan_drive_for_ipod(drive_path) {
            return Some(detected);
        }
    }
    None
}

/// Test-friendly variant: check a specific drive (or any path) for the
/// iPod_Control\Device\SysInfo file and read identity from it.
pub fn scan_drive_for_ipod(drive: &Path) -> Option<DetectedIpod> {
    let sysinfo = drive.join("iPod_Control").join("Device").join("SysInfo");
    if !sysinfo.exists() { return None; }
    let text = std::fs::read_to_string(&sysinfo).ok()?;
    let serial = parse_sysinfo_field(&text, "FirewireGuid")?;
    let model_num = parse_sysinfo_field(&text, "ModelNumStr").unwrap_or_default();
    let model_label = describe_model(&model_num);
    Some(DetectedIpod {
        serial,
        model_label,
        drive: drive.to_string_lossy().into_owned(),
    })
}

fn parse_sysinfo_field(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Best-effort human-friendly label from ModelNumStr. M5 will replace
/// this with libgpod's full model lookup; M2 just shows enough to confirm.
fn describe_model(model_num: &str) -> String {
    // Common Classic 7G codes: MB029 (80GB), MB147 (120GB), MB565 (160GB).
    // Use a permissive prefix match.
    let upper = model_num.trim_start_matches('x').to_uppercase();
    match upper.as_str() {
        "MB029" | "MB147" | "MB565" => format!("iPod Classic 7G ({upper})"),
        _ if !upper.is_empty() => format!("iPod ({upper})"),
        _ => "iPod (model unknown)".to_string(),
    }
}
```

- [ ] **Step 5: Run tests, expect pass**

```powershell
cargo test --lib ipod::device::tests::scan 2>&1 | Select-Object -Last 10
```

Expected: 2 tests pass.

- [ ] **Step 6: Verify full suite still passes**

```powershell
cargo test --lib 2>&1 | Select-String "test result"
```

Expected: 116+ tests pass (114 + 2 new).

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src/ipod/device.rs
git -C F:\repos\ipod-sync commit -m "feat(ipod): scan_for_ipod helper for wizard polling fallback"
```

---

## Task 6: Daemon-side IPC wire types

**Files:**
- Create: `F:\repos\ipod-sync\src\ipc_daemon.rs`
- Modify: `F:\repos\ipod-sync\src\lib.rs` (add `pub mod ipc_daemon;`)

Defines the new events and commands the daemon speaks to UI clients over the named pipe. Separate from `src/ipc.rs` (the M1 sync-subprocess wire types). Same envelope shape (`{"type": "...", ...}`), additive.

- [ ] **Step 1: Wire module + write failing serialization tests**

Edit `src/lib.rs`:

```rust
pub mod ipc;
pub mod ipc_daemon;  // NEW (alphabetical)
```

Create `src/ipc_daemon.rs`:

```rust
//! Daemon-side IPC wire types for the UI ↔ daemon channel (named pipe
//! / Unix socket). Distinct from `src/ipc.rs` (which is the daemon ↔
//! sync-subprocess channel). Same envelope shape: newline-delimited
//! JSON, snake_case "type" discriminator, additive.
//!
//! Spec §7. Protocol semver: daemon emits hello with
//! `protocol_version = "1.1.0"` since this extends M1's "1.0.0".

use crate::config_file::{DaemonSettings, IpodIdentity};
use crate::daemon::history::HistoryEntry;
use serde::{Deserialize, Serialize};

pub const DAEMON_PROTOCOL_VERSION: &str = "1.1.0";

/// Events from daemon → UI clients (in addition to forwarded sync-
/// subprocess events from `src/ipc.rs`).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Same shape as M1 Hello but bumped protocol_version.
    Hello {
        protocol_version: String,
        core_version: String,
    },
    /// Snapshot of daemon state. Replies to `get_status`; also fired
    /// proactively when state transitions.
    StatusUpdate {
        state: DaemonStateLabel,
        configured: bool,
        ipod_connected: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_sync: Option<HistoryEntry>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_scheduled_unix_secs: Option<u64>,
    },
    /// Full config snapshot. Replies to `get_config`; also fired after
    /// a successful `save_config`.
    ConfigUpdate {
        source: Option<String>,
        daemon: Option<DaemonSettings>,
        ipod: Option<IpodIdentity>,
    },
    /// History snapshot. Replies to `get_history`.
    HistoryUpdate {
        entries: Vec<HistoryEntry>,
    },
    /// Device watcher detected a matching iPod (for daemon mode) or
    /// any iPod (for wizard subscribers).
    DeviceConnected {
        serial: String,
        model_label: String,
        drive: String,
    },
    /// Device watcher detected disconnect.
    DeviceDisconnected {
        serial: String,
    },
    /// Daemon rejected a `trigger_sync` command. Includes a reason
    /// the UI can show as a toast.
    SyncRejected {
        reason: SyncRejectReason,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStateLabel {
    Idle,
    Syncing,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejectReason {
    AlreadySyncing,
    NoIpod,
    NotConfigured,
}

/// Commands from UI → daemon.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    GetStatus,
    GetConfig,
    /// Persist config changes. Daemon writes config.toml + applies live
    /// (re-arms scheduler if interval changed, registers StartupTask if
    /// autostart toggled). Replies with `config_update`.
    SaveConfig {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        daemon: Option<DaemonSettings>,
        #[serde(default)]
        ipod: Option<IpodIdentity>,
    },
    /// Request a sync. Daemon's state machine decides whether to accept.
    TriggerSync {
        source: TriggerSource,
    },
    GetHistory {
        #[serde(default = "default_history_limit")]
        limit: usize,
    },
    /// Wizard uses this to receive `device_connected` events for ANY
    /// iPod (not just the configured-matching one).
    SubscribeDeviceEvents,
    UnsubscribeDeviceEvents,
    /// Graceful daemon shutdown.
    Shutdown,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    Manual,
    Scheduled,
    PlugIn,
}

fn default_history_limit() -> usize { 10 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_serializes_with_protocol_version() {
        let event = DaemonEvent::Hello {
            protocol_version: DAEMON_PROTOCOL_VERSION.to_string(),
            core_version: "0.0.1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"hello""#));
        assert!(json.contains(r#""protocol_version":"1.1.0""#));
    }

    #[test]
    fn get_status_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"get_status"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetStatus));
    }

    #[test]
    fn save_config_with_partial_payload_deserializes() {
        let json = r#"{"type":"save_config","ipod":{"serial":"X","model_label":"iPod 7G"}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveConfig { source, daemon, ipod } => {
                assert!(source.is_none());
                assert!(daemon.is_none());
                let ipod = ipod.expect("ipod present");
                assert_eq!(ipod.serial, "X");
                assert_eq!(ipod.model_label, "iPod 7G");
            }
            _ => panic!("expected SaveConfig"),
        }
    }

    #[test]
    fn trigger_sync_round_trips() {
        let json = r#"{"type":"trigger_sync","source":"manual"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, DaemonCommand::TriggerSync { source: TriggerSource::Manual }));
    }

    #[test]
    fn device_connected_event_serializes_with_required_fields() {
        let event = DaemonEvent::DeviceConnected {
            serial: "0xABC".to_string(),
            model_label: "iPod 7G".to_string(),
            drive: "G:\\".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"device_connected""#));
        assert!(json.contains(r#""drive":"G:\\""#));
    }
}
```

- [ ] **Step 2: Run tests, expect pass**

```powershell
cargo test --lib ipc_daemon 2>&1 | Select-Object -Last 10
```

Expected: 5 tests pass.

- [ ] **Step 3: Verify full suite still passes**

```powershell
cargo test --lib 2>&1 | Select-String "test result"
```

Expected: 121+ tests pass.

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src/lib.rs src/ipc_daemon.rs
git -C F:\repos\ipod-sync commit -m "feat(ipc_daemon): wire types for v1.1.0 daemon protocol"
```

---

## Task 7: IPC server (named pipe + dispatch)

**Files:**
- Create: `F:\repos\ipod-sync\src\daemon\ipc_server.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs` (add `pub mod ipc_server;`)

Multi-instance named-pipe server on Windows. Each connection is a Tokio task. Daemon broadcasts events to all clients; routes commands to a central handler.

- [ ] **Step 1: Wire module**

Edit `src/daemon/mod.rs`:

```rust
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
pub mod state;
```

(IPC server is Windows-only in M2; macOS/Linux Unix socket impl comes with those frontends.)

- [ ] **Step 2: Write the server scaffold**

Create `src/daemon/ipc_server.rs`:

```rust
//! Multi-instance named-pipe IPC server on Windows. Accepts UI client
//! connections, broadcasts daemon events to all clients, routes client
//! commands to a central handler.
//!
//! Pipe path: `\\.\pipe\ipod-sync`. Wire format: newline-delimited JSON
//! per `docs/ipc-protocol.md` (v1.1.0).

use crate::ipc_daemon::{DaemonCommand, DaemonEvent, DAEMON_PROTOCOL_VERSION};
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{broadcast, mpsc};

pub const PIPE_NAME: &str = r"\\.\pipe\ipod-sync";

/// Incoming command from a connected client, tagged with the client id
/// so the handler can reply back via the per-client sender.
pub struct ClientCommand {
    pub client_id: u64,
    pub command: DaemonCommand,
    pub reply: mpsc::UnboundedSender<DaemonEvent>,
}

/// Spawn the IPC server on a Tokio runtime. Returns:
///   - a `broadcast::Sender<DaemonEvent>` the daemon uses to publish
///     events to all connected clients
///   - a `mpsc::UnboundedReceiver<ClientCommand>` the daemon's command
///     handler drains to process incoming commands
pub async fn spawn_server() -> Result<(
    broadcast::Sender<DaemonEvent>,
    mpsc::UnboundedReceiver<ClientCommand>,
)> {
    let (event_tx, _) = broadcast::channel::<DaemonEvent>(256);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();

    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        let mut next_client_id: u64 = 1;
        // Create the first instance up-front.
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

            // Create the next instance immediately so the next client
            // connecting doesn't see "no instances available."
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
            tokio::spawn(handle_client(client_id, connected, event_rx, cmd_tx));
        }
    });

    Ok((event_tx, cmd_rx))
}

async fn handle_client(
    client_id: u64,
    pipe: NamedPipeServer,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
) {
    tracing::info!("ipc-server: client {client_id} connected");
    let (reader_half, mut writer_half) = tokio::io::split(pipe);

    // Per-client reply channel: handler sends DaemonEvents here to be
    // written specifically to this client. Used for GetStatus responses
    // and similar request-scoped replies.
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<DaemonEvent>();

    // Send the Hello event first.
    let hello = DaemonEvent::Hello {
        protocol_version: DAEMON_PROTOCOL_VERSION.to_string(),
        core_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if write_event(&mut writer_half, &hello).await.is_err() {
        return;
    }

    let mut reader = BufReader::new(reader_half);
    let mut line_buf = String::new();
    loop {
        tokio::select! {
            read_result = reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => {
                        tracing::info!("ipc-server: client {client_id} disconnected");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if !trimmed.is_empty() {
                            match serde_json::from_str::<DaemonCommand>(trimmed) {
                                Ok(cmd) => {
                                    let _ = cmd_tx.send(ClientCommand {
                                        client_id,
                                        command: cmd,
                                        reply: reply_tx.clone(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "ipc-server: client {client_id} sent unparseable command {trimmed:?}: {e}"
                                    );
                                }
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        tracing::warn!("ipc-server: client {client_id} read error: {e}");
                        break;
                    }
                }
            }
            broadcast_event = event_rx.recv() => {
                match broadcast_event {
                    Ok(event) => {
                        if write_event(&mut writer_half, &event).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::warn!("ipc-server: client {client_id} lagged broadcast");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            reply_event = reply_rx.recv() => {
                match reply_event {
                    Some(event) => {
                        if write_event(&mut writer_half, &event).await.is_err() { break; }
                    }
                    None => break,
                }
            }
        }
    }
}

async fn write_event<W>(writer: &mut W, event: &DaemonEvent) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let json = serde_json::to_string(event).context("serialize event")?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
```

- [ ] **Step 3: Build to verify**

```powershell
cargo build --release 2>&1 | Select-Object -Last 5
```

Expected: clean build (the IPC server has no unit tests — it's exercised by integration tests in Task 15).

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src/daemon/mod.rs src/daemon/ipc_server.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): named-pipe IPC server with multi-client broadcast"
```

---

## Task 8: Daemon entry point + main loop

**Files:**
- Modify: `F:\repos\ipod-sync\src\cli.rs`
- Modify: `F:\repos\ipod-sync\src\main.rs`
- Create: `F:\repos\ipod-sync\src\daemon\runtime.rs`
- Modify: `F:\repos\ipod-sync\src\daemon\mod.rs`

Adds `--daemon` CLI flag and the daemon main loop. The loop wires together IPC server, state machine, config service, history service, and dispatches commands. Device watching is OUT OF M2 SCOPE — the wizard's polling (Task 5) fills the gap.

- [ ] **Step 1: Add --daemon flag**

In `src/cli.rs`, add to the `Cli` struct:

```rust
/// Run as a long-lived background daemon. Listens on a named pipe for
/// UI clients, handles device events + scheduling, spawns sync
/// subprocesses on demand. See spec/2026-05-24-phase-6-daemon-model-design.md.
/// Mutually exclusive with --ipc-mode and --no-tui.
#[arg(long, conflicts_with_all = ["ipc_mode", "no_tui"])]
pub daemon: bool,
```

Add unit test in the same file:

```rust
#[test]
fn parses_daemon_flag() {
    let cli = Cli::try_parse_from(["ipod-sync", "--daemon"]).unwrap();
    assert!(cli.daemon);
}

#[test]
fn daemon_and_ipc_mode_conflict() {
    let result = Cli::try_parse_from(["ipod-sync", "--daemon", "--ipc-mode"]);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run, expect pass**

```powershell
cargo test --lib cli::tests::parses_daemon_flag 2>&1 | Select-Object -Last 5
cargo test --lib cli::tests::daemon_and_ipc_mode_conflict 2>&1 | Select-Object -Last 5
```

Expected: both pass.

- [ ] **Step 3: Create daemon runtime module**

Edit `src/daemon/mod.rs`:

```rust
pub mod history;
#[cfg(windows)]
pub mod ipc_server;
#[cfg(windows)]
pub mod runtime;
pub mod state;
```

Create `src/daemon/runtime.rs`:

```rust
//! Daemon main loop. Wires IPC server, state machine, config + history
//! services, and dispatches client commands.
//!
//! M2 scope: respond to GetStatus / GetConfig / SaveConfig / GetHistory
//! / Subscribe-/UnsubscribeDeviceEvents / Shutdown. TriggerSync replies
//! with `sync_rejected { reason: not_configured }` until M3 wires the
//! sync orchestrator.

use crate::config_file::{self, PersistedConfig};
use crate::daemon::history::HistoryService;
use crate::daemon::ipc_server::{spawn_server, ClientCommand};
use crate::daemon::state::StateMachine;
use crate::ipc_daemon::{DaemonCommand, DaemonEvent, DaemonStateLabel, SyncRejectReason};
use anyhow::Result;
use std::sync::Mutex;

pub async fn run_daemon() -> Result<()> {
    tracing::info!("daemon: starting");

    let history_path = history_file_path()?;
    let history = HistoryService::new(history_path.clone());
    let config_path = config_file::default_path()?;
    let state = Mutex::new(StateMachine::new());

    let (event_tx, mut cmd_rx) = spawn_server().await?;

    tracing::info!("daemon: ready");

    while let Some(client_cmd) = cmd_rx.recv().await {
        handle_command(client_cmd, &history, &config_path, &state, &event_tx).await;
    }

    tracing::info!("daemon: exiting (command channel closed)");
    Ok(())
}

async fn handle_command(
    ClientCommand { client_id, command, reply }: ClientCommand,
    history: &HistoryService,
    config_path: &std::path::Path,
    state: &Mutex<StateMachine>,
    event_tx: &tokio::sync::broadcast::Sender<DaemonEvent>,
) {
    tracing::info!("daemon: client {client_id} command: {command:?}");
    match command {
        DaemonCommand::GetStatus => {
            let configured = config_file::load(config_path)
                .ok()
                .flatten()
                .and_then(|c| c.ipod)
                .is_some();
            let state_label = match state.lock().unwrap().state() {
                crate::daemon::state::DaemonState::Idle => DaemonStateLabel::Idle,
                crate::daemon::state::DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
            };
            let entries = history.read();
            let last_sync = entries.last().cloned();
            let _ = reply.send(DaemonEvent::StatusUpdate {
                state: state_label,
                configured,
                ipod_connected: false,  // M3 wires this
                last_sync,
                next_scheduled_unix_secs: None,  // M3 wires this
            });
        }
        DaemonCommand::GetConfig => {
            let cfg = config_file::load(config_path).ok().flatten();
            let _ = reply.send(build_config_update(cfg));
        }
        DaemonCommand::SaveConfig { source, daemon, ipod } => {
            let mut current = config_file::load(config_path).ok().flatten().unwrap_or_default();
            if let Some(s) = source { current.source = Some(std::path::PathBuf::from(s)); }
            if let Some(d) = daemon { current.daemon = Some(d); }
            if let Some(i) = ipod { current.ipod = Some(i); }
            if let Err(e) = config_file::save(config_path, &current) {
                tracing::error!("daemon: failed to save config: {e}");
                return;
            }
            // Broadcast the new config to ALL clients so any settings window
            // hosted in another UI instance updates.
            let _ = event_tx.send(build_config_update(Some(current)));
        }
        DaemonCommand::GetHistory { limit } => {
            let mut entries = history.read();
            let start = entries.len().saturating_sub(limit);
            entries.drain(..start);
            let _ = reply.send(DaemonEvent::HistoryUpdate { entries });
        }
        DaemonCommand::TriggerSync { .. } => {
            // M2: real sync is M3. Reject with NotConfigured until then.
            let _ = reply.send(DaemonEvent::SyncRejected {
                reason: SyncRejectReason::NotConfigured,
            });
        }
        DaemonCommand::SubscribeDeviceEvents | DaemonCommand::UnsubscribeDeviceEvents => {
            // M2: no DeviceWatcher yet (M3). No-op; wizard uses polling
            // via the C# side calling scan_for_ipod via SaveConfig flow.
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
            ipod: c.ipod,
        },
        None => DaemonEvent::ConfigUpdate { source: None, daemon: None, ipod: None },
    }
}

fn history_file_path() -> Result<std::path::PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA unavailable"))?
        .join("ipod-sync");
    Ok(base.join("history.json"))
}
```

- [ ] **Step 4: Branch main.rs on --daemon**

Edit `src/main.rs`. After parsing CLI and BEFORE the existing TUI/ipc-mode setup:

```rust
fn main() -> anyhow::Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = ipod_sync::cli::Cli::parse();

    if cli.daemon {
        // Daemon mode bypasses TUI / progress / orchestrate entirely.
        // Logging is routed to file (like ipc-mode).
        ipod_sync::logging::init(cli.verbose, /*use_tui*/ false, /*ipc_mode*/ true);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(ipod_sync::daemon::runtime::run_daemon());
    }

    // ... existing TUI / ipc-mode dispatch unchanged ...
}
```

- [ ] **Step 5: Build clean**

```powershell
cargo build --release 2>&1 | Select-Object -Last 5
```

Expected: clean.

- [ ] **Step 6: Manual smoke (one-shot)**

```powershell
$proc = Start-Process -FilePath "F:\repos\ipod-sync\target\release\ipod-sync.exe" -ArgumentList "--daemon" -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 1
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "ipod-sync", [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous)
$pipe.Connect(2000)
$reader = New-Object System.IO.StreamReader($pipe)
$writer = New-Object System.IO.StreamWriter($pipe)
$writer.AutoFlush = $true
"Hello received: $($reader.ReadLine())"
$writer.WriteLine('{"type":"get_status"}')
"Status: $($reader.ReadLine())"
$writer.WriteLine('{"type":"shutdown"}')
$pipe.Dispose()
Start-Sleep -Seconds 1
"Process exited: $($proc.HasExited)"
```

Expected: hello event line, status_update line, daemon exits cleanly.

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src/cli.rs src/main.rs src/daemon/mod.rs src/daemon/runtime.rs
git -C F:\repos\ipod-sync commit -m "feat(daemon): --daemon CLI flag + main loop + command dispatch"
```

---

## Task 9: C# DaemonClient (replaces CoreProcess)

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\DaemonClient.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\DaemonEvent.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\DaemonCommand.cs`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\CoreProcess.cs`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\CoreLocator.cs`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Dialogs\CoreNotFoundDialog.cs` (was in UI proj; verify path)
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\DaemonClientTests.cs`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\CoreLocatorTests.cs`

Replaces M1's per-sync subprocess client with a persistent named-pipe client. Wire format identical (snake_case JSON envelopes).

- [ ] **Step 1: Define new C# event/command records**

Create `ui-windows/IpodSync.UI.Core/Ipc/DaemonEvent.cs`:

```csharp
using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace IpodSync_UI.Ipc;

/// <summary>
/// Daemon-side events sent over the UI ↔ daemon named pipe. Augments
/// (does not replace) the M1 <see cref="IpcEvent"/> hierarchy — sync-
/// subprocess events (Header, Summary, Review, etc.) are forwarded by
/// the daemon and arrive on the SAME pipe, deserialized via the M1
/// IpcEvent polymorphic table.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StatusUpdateEvent), "status_update")]
[JsonDerivedType(typeof(ConfigUpdateEvent), "config_update")]
[JsonDerivedType(typeof(HistoryUpdateEvent), "history_update")]
[JsonDerivedType(typeof(DeviceConnectedEvent), "device_connected")]
[JsonDerivedType(typeof(DeviceDisconnectedEvent), "device_disconnected")]
[JsonDerivedType(typeof(SyncRejectedEvent), "sync_rejected")]
public abstract record DaemonEvent;

public sealed record StatusUpdateEvent(
    [property: JsonPropertyName("state")] string State,
    [property: JsonPropertyName("configured")] bool Configured,
    [property: JsonPropertyName("ipod_connected")] bool IpodConnected,
    [property: JsonPropertyName("last_sync")] HistoryEntry? LastSync,
    [property: JsonPropertyName("next_scheduled_unix_secs")] long? NextScheduledUnixSecs
) : DaemonEvent;

public sealed record ConfigUpdateEvent(
    [property: JsonPropertyName("source")] string? Source,
    [property: JsonPropertyName("daemon")] DaemonSettings? Daemon,
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod
) : DaemonEvent;

public sealed record HistoryUpdateEvent(
    [property: JsonPropertyName("entries")] IReadOnlyList<HistoryEntry> Entries
) : DaemonEvent;

public sealed record DeviceConnectedEvent(
    [property: JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("model_label")] string ModelLabel,
    [property: JsonPropertyName("drive")] string Drive
) : DaemonEvent;

public sealed record DeviceDisconnectedEvent(
    [property: JsonPropertyName("serial")] string Serial
) : DaemonEvent;

public sealed record SyncRejectedEvent(
    [property: JsonPropertyName("reason")] string Reason
) : DaemonEvent;

public sealed record DaemonSettings(
    [property: JsonPropertyName("enabled")] bool Enabled,
    [property: JsonPropertyName("autostart_with_windows")] bool AutostartWithWindows,
    [property: JsonPropertyName("first_sync_mode")] string FirstSyncMode,
    [property: JsonPropertyName("subsequent_sync_mode")] string SubsequentSyncMode,
    [property: JsonPropertyName("schedule_minutes")] uint ScheduleMinutes,
    [property: JsonPropertyName("notify_on")] string NotifyOn
);

public sealed record IpodIdentity(
    [property: JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("model_label")] string ModelLabel
);

public sealed record HistoryEntry(
    [property: JsonPropertyName("timestamp")] string Timestamp,
    [property: JsonPropertyName("duration_secs")] ulong DurationSecs,
    [property: JsonPropertyName("trigger")] string Trigger,
    [property: JsonPropertyName("outcome")] string Outcome,
    [property: JsonPropertyName("error_message")] string? ErrorMessage,
    [property: JsonPropertyName("summary")] SyncSummary? Summary
);

public sealed record SyncSummary(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged,
    [property: JsonPropertyName("skipped")] int Skipped
);
```

Create `ui-windows/IpodSync.UI.Core/Ipc/DaemonCommand.cs`:

```csharp
using System.Text.Json.Serialization;

namespace IpodSync_UI.Ipc;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(GetStatusCommand), "get_status")]
[JsonDerivedType(typeof(GetConfigCommand), "get_config")]
[JsonDerivedType(typeof(SaveConfigCommand), "save_config")]
[JsonDerivedType(typeof(TriggerSyncCommand), "trigger_sync")]
[JsonDerivedType(typeof(GetHistoryCommand), "get_history")]
[JsonDerivedType(typeof(SubscribeDeviceEventsCommand), "subscribe_device_events")]
[JsonDerivedType(typeof(UnsubscribeDeviceEventsCommand), "unsubscribe_device_events")]
[JsonDerivedType(typeof(ShutdownCommand), "shutdown")]
public abstract record DaemonCommand;

public sealed record GetStatusCommand : DaemonCommand;
public sealed record GetConfigCommand : DaemonCommand;

public sealed record SaveConfigCommand(
    [property: JsonPropertyName("source")] string? Source = null,
    [property: JsonPropertyName("daemon")] DaemonSettings? Daemon = null,
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod = null
) : DaemonCommand;

public sealed record TriggerSyncCommand(
    [property: JsonPropertyName("source")] string Source  // "manual" | "scheduled" | "plug_in"
) : DaemonCommand;

public sealed record GetHistoryCommand(
    [property: JsonPropertyName("limit")] int Limit = 10
) : DaemonCommand;

public sealed record SubscribeDeviceEventsCommand : DaemonCommand;
public sealed record UnsubscribeDeviceEventsCommand : DaemonCommand;
public sealed record ShutdownCommand : DaemonCommand;
```

- [ ] **Step 2: Implement DaemonClient**

Create `ui-windows/IpodSync.UI.Core/Ipc/DaemonClient.cs`:

```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace IpodSync_UI.Ipc;

/// <summary>
/// Persistent named-pipe client to the running ipod-sync daemon.
/// Replaces M1's <c>CoreProcess</c> (which spawned a per-sync subprocess).
///
/// API contract:
///   - <see cref="ConnectAsync"/> opens the pipe, awaits the hello event,
///     validates protocol_version. Throws if daemon unreachable after retries.
///   - <see cref="Events"/> is a ChannelReader of incoming events (both
///     DaemonEvent and forwarded IpcEvent from the sync subprocess; consumers
///     pattern-match on type).
///   - <see cref="SendAsync"/> writes a command line. Returns when flushed.
///   - <see cref="DisposeAsync"/> closes the pipe; daemon stays running.
/// </summary>
public sealed class DaemonClient : IAsyncDisposable
{
    public const string PipeName = "ipod-sync";
    private static readonly TimeSpan HelloTimeout = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan[] ReconnectBackoff = new[]
    {
        TimeSpan.FromSeconds(1),
        TimeSpan.FromSeconds(2),
        TimeSpan.FromSeconds(4),
    };

    private readonly NamedPipeClientStream _pipe;
    private readonly Channel<object> _events;
    private readonly CancellationTokenSource _cts;
    private readonly Task _readerTask;
    private int _disposed;

    public ChannelReader<object> Events => _events.Reader;

    private DaemonClient(NamedPipeClientStream pipe, Channel<object> events, CancellationTokenSource cts, Task readerTask)
    {
        _pipe = pipe;
        _events = events;
        _cts = cts;
        _readerTask = readerTask;
    }

    public static async Task<DaemonClient> ConnectAsync(CancellationToken cancellationToken = default)
    {
        Exception? lastException = null;
        foreach (var delay in ReconnectBackoff)
        {
            try
            {
                var pipe = new NamedPipeClientStream(
                    ".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
                await pipe.ConnectAsync(2000, cancellationToken).ConfigureAwait(false);

                var events = Channel.CreateUnbounded<object>(new UnboundedChannelOptions
                {
                    SingleReader = true,
                    SingleWriter = true,
                });
                var cts = new CancellationTokenSource();
                var readerTask = Task.Run(() => ReaderLoop(pipe, events.Writer, cts.Token));

                var client = new DaemonClient(pipe, events, cts, readerTask);
                // Await hello.
                using var helloTimeout = new CancellationTokenSource(HelloTimeout);
                using var linked = CancellationTokenSource.CreateLinkedTokenSource(
                    cancellationToken, helloTimeout.Token);
                var first = await events.Reader.ReadAsync(linked.Token).ConfigureAwait(false);
                if (first is not HelloEvent hello)
                {
                    await client.DisposeAsync().ConfigureAwait(false);
                    throw new InvalidOperationException($"expected hello, got {first.GetType().Name}");
                }
                if (!hello.ProtocolVersion.StartsWith("1.", StringComparison.Ordinal))
                {
                    await client.DisposeAsync().ConfigureAwait(false);
                    throw new InvalidOperationException(
                        $"daemon protocol {hello.ProtocolVersion} not supported by UI");
                }
                return client;
            }
            catch (Exception e)
            {
                lastException = e;
                Debug.WriteLine($"daemon-client: connect attempt failed: {e.Message}; backing off {delay.TotalSeconds}s");
                await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            }
        }
        throw new InvalidOperationException(
            $"daemon unreachable after {ReconnectBackoff.Length} attempts", lastException);
    }

    private static async Task ReaderLoop(NamedPipeClientStream pipe, ChannelWriter<object> writer, CancellationToken ct)
    {
        try
        {
            using var reader = new StreamReader(pipe, new UTF8Encoding(false), leaveOpen: true);
            while (!ct.IsCancellationRequested)
            {
                var line = await reader.ReadLineAsync(ct).ConfigureAwait(false);
                if (line is null) break;
                if (string.IsNullOrWhiteSpace(line)) continue;
                if (TryDeserialize(line, out var evt))
                {
                    await writer.WriteAsync(evt!, ct).ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException) { /* expected on dispose */ }
        catch (Exception e)
        {
            Debug.WriteLine($"daemon-client: reader loop terminated: {e.Message}");
        }
        finally { writer.TryComplete(); }
    }

    private static bool TryDeserialize(string line, out object? evt)
    {
        // Try the M1 IpcEvent hierarchy first (it owns Hello + all sync-
        // subprocess events). Then try DaemonEvent (status/config/etc.).
        try
        {
            evt = JsonSerializer.Deserialize<IpcEvent>(line);
            if (evt is not null) return true;
        }
        catch (JsonException) { /* fall through */ }
        try
        {
            evt = JsonSerializer.Deserialize<DaemonEvent>(line);
            if (evt is not null) return true;
        }
        catch (JsonException jx)
        {
            Debug.WriteLine($"daemon-client: unparseable line `{line}`: {jx.Message}");
        }
        evt = null;
        return false;
    }

    public async Task SendAsync(DaemonCommand command, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(command);
        var json = JsonSerializer.Serialize<DaemonCommand>(command);
        var bytes = Encoding.UTF8.GetBytes(json + "\n");
        await _pipe.WriteAsync(bytes, cancellationToken).ConfigureAwait(false);
        await _pipe.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        _cts.Cancel();
        try { await _readerTask.ConfigureAwait(false); } catch { /* expected */ }
        _pipe.Dispose();
        _cts.Dispose();
    }
}
```

- [ ] **Step 3: Delete M1's CoreProcess, CoreLocator, and dialog**

```powershell
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\Ipc\CoreProcess.cs
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI.Core\CoreLocator.cs
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\Dialogs\CoreNotFoundDialog.cs -ErrorAction SilentlyContinue
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\CoreLocatorTests.cs
```

- [ ] **Step 4: Update any references to deleted types**

The existing `AppController.cs` (and possibly other UI code) references `CoreProcess` and `CoreLocator`. Either:
- Delete `AppController.cs` if its M1 responsibilities are entirely replaced by Task 11 (App.xaml.cs hidden startup + daemon probe).
- Or stub it out with a minimal `DaemonClient`-using version that subsequent tasks complete.

Decision for M2: delete `AppController.cs` entirely; its responsibilities split between `App.xaml.cs` (Task 11) and `TrayIconController.cs` (Task 10). Note any other consumers and remove their references too.

```powershell
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\AppController.cs
```

- [ ] **Step 5: Add unit tests**

Create `ui-windows/IpodSync.UI.Tests/DaemonClientTests.cs`:

```csharp
using System.Text.Json;
using IpodSync_UI.Ipc;
using Xunit;

public class DaemonClientWireFormatTests
{
    [Fact]
    public void StatusUpdate_event_deserializes_via_DaemonEvent()
    {
        var json = """{"type":"status_update","state":"idle","configured":true,"ipod_connected":false,"last_sync":null,"next_scheduled_unix_secs":null}""";
        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var status = Assert.IsType<StatusUpdateEvent>(evt);
        Assert.Equal("idle", status.State);
        Assert.True(status.Configured);
        Assert.False(status.IpodConnected);
    }

    [Fact]
    public void SaveConfig_command_serializes_with_ipod_only()
    {
        var cmd = new SaveConfigCommand(Ipod: new IpodIdentity("EXAMPLE1234", "iPod 7G"));
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Contains("\"type\":\"save_config\"", json);
        Assert.Contains("\"serial\":\"EXAMPLE1234\"", json);
        Assert.Contains("\"model_label\":\"iPod 7G\"", json);
    }

    [Fact]
    public void TriggerSync_command_round_trips()
    {
        var cmd = new TriggerSyncCommand("manual");
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        var back = JsonSerializer.Deserialize<DaemonCommand>(json);
        var trig = Assert.IsType<TriggerSyncCommand>(back);
        Assert.Equal("manual", trig.Source);
    }

    [Fact]
    public void Shutdown_command_serializes_with_type_only()
    {
        var cmd = new ShutdownCommand();
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Equal("{\"type\":\"shutdown\"}", json);
    }

    [Fact]
    public void DeviceConnected_event_carries_all_fields()
    {
        var json = """{"type":"device_connected","serial":"X","model_label":"iPod 7G","drive":"G:\\"}""";
        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var dev = Assert.IsType<DeviceConnectedEvent>(evt);
        Assert.Equal("X", dev.Serial);
        Assert.Equal("iPod 7G", dev.ModelLabel);
        Assert.Equal("G:\\", dev.Drive);
    }
}
```

- [ ] **Step 6: Update test project's linked-compile entries**

The test project links VM source files via csproj `<Compile Include>`. Update the test csproj to drop the `CoreProcess.cs` / `CoreLocator.cs` references and add new `DaemonClient`-related files if linked-compile is needed.

Verify `ui-windows/IpodSync.UI.Tests/IpodSync.UI.Tests.csproj` has no broken refs. The test project should reference `IpodSync.UI.Core` via `<ProjectReference>` for the IPC types; only VM-style files need link-compile entries.

- [ ] **Step 7: Verify build + tests**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --logger "console;verbosity=minimal" 2>&1 | Select-Object -Last 5
```

Expected: clean build. Test count: previous M1 had 41 tests; we removed CoreLocator's 4 (now 37) and added 5 DaemonClient wire-format tests (now 42).

- [ ] **Step 8: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI.Core/Ipc/DaemonClient.cs ui-windows/IpodSync.UI.Core/Ipc/DaemonEvent.cs ui-windows/IpodSync.UI.Core/Ipc/DaemonCommand.cs ui-windows/IpodSync.UI.Tests/DaemonClientTests.cs
git -C F:\repos\ipod-sync rm ui-windows/IpodSync.UI.Core/Ipc/CoreProcess.cs ui-windows/IpodSync.UI.Core/CoreLocator.cs ui-windows/IpodSync.UI.Tests/CoreLocatorTests.cs ui-windows/IpodSync.UI/AppController.cs
git -C F:\repos\ipod-sync rm ui-windows/IpodSync.UI/Dialogs/CoreNotFoundDialog.cs
git -C F:\repos\ipod-sync commit -m "refactor(ui-windows): DaemonClient replaces CoreProcess; delete CoreLocator + AppController + CoreNotFoundDialog"
```

---

## Task 10: TrayIconController (H.NotifyIcon)

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\IpodSync.UI.csproj` (add NuGet)
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\TrayIconController.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\` (add icon assets)

M2 ships idle/offline icon states and a Quit menu item. Syncing/error states + Sync Now + Settings menu items land in M3/M4.

- [ ] **Step 1: Add H.NotifyIcon.WinUI package**

Edit `ui-windows/IpodSync.UI/IpodSync.UI.csproj`, add to the `<ItemGroup>` with other PackageReferences:

```xml
<PackageReference Include="H.NotifyIcon.WinUI" Version="2.1.5" />
```

Verify the package version is current — check https://www.nuget.org/packages/H.NotifyIcon.WinUI. Use the latest 2.x stable.

- [ ] **Step 2: Add tray icon asset**

For M2, use a placeholder ICO file. Generate a simple blue square with a sync arrow using Inkscape or pull a Fluent Icons "Sync" SVG and convert. Save as `ui-windows/IpodSync.UI/Assets/tray-idle.ico` and `tray-offline.ico`. Two states are enough for M2.

If you don't have tooling handy, ship with placeholder 16×16 ICO from a known good source. For a true minimum, copy the existing `Assets/AppIcon.ico` to both names and tint manually later.

- [ ] **Step 3: Implement TrayIconController**

Create `ui-windows/IpodSync.UI/TrayIconController.cs`:

```csharp
using System;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI;

/// <summary>
/// Owns the system tray icon. M2 ships idle / offline states + Quit
/// menu item. M3 adds syncing / error states + Sync Now / Settings.
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private bool _disposed;

    public event Action? QuitRequested;
    public event Action? ShowSettingsRequested;  // M4 wires the Settings menu item

    public void Initialize()
    {
        var menu = new MenuFlyout();
        var quit = new MenuFlyoutItem { Text = "Quit" };
        quit.Click += (_, _) => QuitRequested?.Invoke();
        menu.Items.Add(quit);

        _icon = new TaskbarIcon
        {
            IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(
                new Uri("ms-appx:///Assets/tray-idle.ico")),
            ToolTipText = "ipod-sync · idle",
            ContextFlyout = menu,
        };
        _icon.ForceCreate();
    }

    public void SetState(TrayIconState state)
    {
        if (_icon is null) return;
        string iconAsset;
        string tooltip;
        switch (state)
        {
            case TrayIconState.Idle:
                iconAsset = "tray-idle.ico";
                tooltip = "ipod-sync · idle";
                break;
            case TrayIconState.Offline:
                iconAsset = "tray-offline.ico";
                tooltip = "iPod not connected";
                break;
            default:
                iconAsset = "tray-idle.ico";
                tooltip = $"ipod-sync · {state}";
                break;
        }
        _icon.IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(
            new Uri($"ms-appx:///Assets/{iconAsset}"));
        _icon.ToolTipText = tooltip;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _icon?.Dispose();
        _icon = null;
    }
}

public enum TrayIconState
{
    Idle,
    Syncing,  // M3
    Error,    // M3
    Offline,
}
```

- [ ] **Step 4: Build to verify**

```powershell
dotnet restore F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

Expected: clean (warnings about asset files are OK if you used placeholder icons).

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/IpodSync.UI.csproj ui-windows/IpodSync.UI/TrayIconController.cs ui-windows/IpodSync.UI/Assets/tray-idle.ico ui-windows/IpodSync.UI/Assets/tray-offline.ico
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): TrayIconController with H.NotifyIcon (idle/offline + Quit)"
```

---

## Task 11: App startup — hidden, daemon probe, wizard dispatch

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml.cs` (M1 used this as Frame host; reuse for wizard hosting)
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainPage.xaml`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainPage.xaml.cs`
- Delete: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\MainPageViewModel.cs` (M1 template VM, unused)

Daemon-mode startup flow: probe pipe → spawn daemon if needed → connect → if config missing, show wizard; else hide to tray.

- [ ] **Step 1: Delete M1 MainPage**

```powershell
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainPage.xaml
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainPage.xaml.cs
Remove-Item F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\MainPageViewModel.cs -ErrorAction SilentlyContinue
```

- [ ] **Step 2: Rewrite App.xaml.cs for daemon-startup flow**

Replace `App.xaml.cs` body:

```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
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
    public static TrayIconController? Tray { get; private set; }

    public App()
    {
        this.InitializeComponent();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        DispatcherQueue = DispatcherQueue.GetForCurrentThread();

        // 1. Set up tray icon early so something visible exists even if
        //    daemon connection takes a moment.
        Tray = new TrayIconController();
        Tray.Initialize();
        Tray.QuitRequested += OnQuitRequested;

        // 2. Ensure daemon is running.
        if (!await IsDaemonRunningAsync())
        {
            SpawnDaemon();
            // Give it a moment to create the pipe.
            await Task.Delay(500);
        }

        // 3. Connect to daemon.
        try
        {
            Daemon = await DaemonClient.ConnectAsync();
        }
        catch (Exception e)
        {
            Debug.WriteLine($"app: failed to connect to daemon: {e}");
            // Surface as a tray notification rather than a window pop.
            // For now, just quit cleanly.
            Tray?.Dispose();
            Application.Current.Exit();
            return;
        }

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
    }

    private void ShowWizard()
    {
        Window = new WizardWindow();
        WindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(Window);
        Window.Closed += (_, _) => Window = null;
        Window.Activate();
    }

    private void OnQuitRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is not null)
            {
                try { await Daemon.SendAsync(new ShutdownCommand()); } catch { /* daemon may already be dead */ }
                await Daemon.DisposeAsync();
            }
            Tray?.Dispose();
            Application.Current.Exit();
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
        catch
        {
            return false;
        }
    }

    private static void SpawnDaemon()
    {
        // Locate ipod-sync.exe (bundled alongside the UI exe).
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

- [ ] **Step 3: Build to verify**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

Expected: build fails because `WizardWindow` doesn't exist yet — Task 12 creates it. To get a green intermediate state, you can scaffold a placeholder `WizardWindow` now (empty XAML) and fill it in Task 12. Alternative: do Tasks 11 + 12 in the same commit cycle.

Recommended: scaffold an empty `WizardWindow.xaml` + `.xaml.cs` with just a placeholder TextBlock, get build green, then commit Task 11. Task 12 fleshes out the wizard.

- [ ] **Step 4: Scaffold empty WizardWindow**

Create `ui-windows/IpodSync.UI/Views/WizardWindow.xaml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<Window
    x:Class="IpodSync_UI.Views.WizardWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="ipod-sync setup">
    <Grid Padding="32">
        <TextBlock Text="Wizard scaffold — filled in Task 12" />
    </Grid>
</Window>
```

Create `ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs`:

```csharp
using Microsoft.UI.Xaml;

namespace IpodSync_UI.Views;

public sealed partial class WizardWindow : Window
{
    public WizardWindow()
    {
        this.InitializeComponent();
    }
}
```

- [ ] **Step 5: Build again, expect clean**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

Expected: clean.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync rm ui-windows/IpodSync.UI/MainPage.xaml ui-windows/IpodSync.UI/MainPage.xaml.cs
git -C F:\repos\ipod-sync rm ui-windows/IpodSync.UI/ViewModels/MainPageViewModel.cs
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/App.xaml.cs ui-windows/IpodSync.UI/Views/WizardWindow.xaml ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): hidden-startup app shell + daemon probe + wizard scaffold"
```

---

## Task 12: WizardViewModel + 3-step wizard UI

**Files:**
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\WizardViewModel.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\WizardWindow.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\WizardWindow.xaml.cs`
- Create: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\WizardViewModelTests.cs`

Three-step wizard with progress dots, source picker (Step 1), iPod identification via polling (Step 2), confirmation (Step 3).

- [ ] **Step 1: Write failing VM tests**

Create `ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs`:

```csharp
using System.Threading.Tasks;
using IpodSync_UI.ViewModels;
using Xunit;

public class WizardViewModelTests
{
    [Fact]
    public void Starts_on_step_1_with_no_source()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        Assert.Equal(1, vm.CurrentStep);
        Assert.Equal("", vm.SourcePath);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void NextCommand_enabled_when_source_set_on_step_1()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"\\HOST\share\music";
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void Next_advances_to_step_2_and_triggers_initial_scan()
    {
        var vm = new WizardViewModel(scanFunc: () => new IpodIdentityCandidate("0xABC", "iPod 7G", "G:\\"),
                                     sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        Assert.Equal(2, vm.CurrentStep);
        Assert.NotNull(vm.DetectedIpod);
        Assert.Equal("0xABC", vm.DetectedIpod!.Serial);
    }

    [Fact]
    public void Step_2_NextCommand_disabled_until_ipod_detected()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);  // advance to step 2
        Assert.Equal(2, vm.CurrentStep);
        Assert.Null(vm.DetectedIpod);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Finish_sends_save_config_with_source_and_ipod()
    {
        SaveConfigPayload? sent = null;
        var vm = new WizardViewModel(
            scanFunc: () => new IpodIdentityCandidate("X", "iPod 7G", "G:\\"),
            sendConfigFunc: p => { sent = p; return Task.CompletedTask; });
        vm.SourcePath = @"\\HOST\music";
        vm.NextCommand.Execute(null);  // step 2 (with iPod)
        vm.NextCommand.Execute(null);  // step 3
        await vm.FinishCommand.ExecuteAsync(null);
        Assert.NotNull(sent);
        Assert.Equal(@"\\HOST\music", sent!.Source);
        Assert.Equal("X", sent.IpodSerial);
        Assert.Equal("iPod 7G", sent.IpodModelLabel);
    }
}
```

- [ ] **Step 2: Run, expect fails (VM doesn't exist yet)**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~WizardViewModelTests" 2>&1 | Select-Object -Last 5
```

Expected: FAIL — `WizardViewModel` not found.

- [ ] **Step 3: Implement WizardViewModel**

Create `ui-windows/IpodSync.UI/ViewModels/WizardViewModel.cs`:

```csharp
using System;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace IpodSync_UI.ViewModels;

public sealed record IpodIdentityCandidate(string Serial, string ModelLabel, string Drive);

public sealed record SaveConfigPayload(string Source, string IpodSerial, string IpodModelLabel);

public partial class WizardViewModel : ObservableObject
{
    private readonly Func<IpodIdentityCandidate?> _scanFunc;
    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private IpodIdentityCandidate? detectedIpod;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";

    public WizardViewModel(
        Func<IpodIdentityCandidate?> scanFunc,
        Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _scanFunc = scanFunc;
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
            TriggerScan();
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

    [RelayCommand]
    private void TriggerScan()
    {
        Scanning = true;
        ScanError = "";
        try
        {
            DetectedIpod = _scanFunc();
            if (DetectedIpod is null)
            {
                ScanError = "No iPod detected. Plug in your iPod and click Retry.";
            }
        }
        catch (Exception e)
        {
            ScanError = $"Scan failed: {e.Message}";
        }
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

    public event Action? WizardFinished;
}
```

- [ ] **Step 4: Add VM source to test project's linked compile**

Edit `ui-windows/IpodSync.UI.Tests/IpodSync.UI.Tests.csproj`:

```xml
<ItemGroup>
  <Compile Include="..\IpodSync.UI\ViewModels\WizardViewModel.cs" Link="LinkedViewModels\WizardViewModel.cs" />
  <!-- existing linked VMs stay -->
</ItemGroup>
```

- [ ] **Step 5: Run tests, expect pass**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --filter "FullyQualifiedName~WizardViewModelTests" --logger "console;verbosity=minimal" 2>&1 | Select-Object -Last 5
```

Expected: 5 tests pass.

- [ ] **Step 6: Build WizardWindow XAML for all 3 steps**

Replace `ui-windows/IpodSync.UI/Views/WizardWindow.xaml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<Window
    x:Class="IpodSync_UI.Views.WizardWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="ipod-sync setup">
    <Grid Padding="0">
        <Grid.RowDefinitions>
            <RowDefinition Height="Auto"/>
            <RowDefinition Height="*"/>
            <RowDefinition Height="Auto"/>
        </Grid.RowDefinitions>

        <!-- Step indicator -->
        <StackPanel Grid.Row="0" Orientation="Horizontal" Padding="32,24,32,16" Spacing="12" HorizontalAlignment="Center">
            <Ellipse Width="10" Height="10" Fill="{x:Bind StepDotFill(1, ViewModel.CurrentStep), Mode=OneWay}"/>
            <Ellipse Width="10" Height="10" Fill="{x:Bind StepDotFill(2, ViewModel.CurrentStep), Mode=OneWay}"/>
            <Ellipse Width="10" Height="10" Fill="{x:Bind StepDotFill(3, ViewModel.CurrentStep), Mode=OneWay}"/>
        </StackPanel>

        <!-- Step body — uses Visibility binding on CurrentStep == N -->
        <Grid Grid.Row="1" Padding="32,8,32,8" MaxWidth="520" HorizontalAlignment="Center">
            <!-- Step 1 -->
            <StackPanel Visibility="{x:Bind IsStep(1, ViewModel.CurrentStep), Mode=OneWay}" Spacing="12">
                <TextBlock Text="Pick your music library" Style="{StaticResource SubtitleTextBlockStyle}"/>
                <TextBlock Text="Choose the folder ipod-sync will sync to your iPod. Usually a local music folder or a network share." TextWrapping="Wrap" Opacity="0.7"/>
                <Grid ColumnDefinitions="*,Auto" ColumnSpacing="8" Margin="0,8,0,0">
                    <TextBox Grid.Column="0" Text="{x:Bind ViewModel.SourcePath, Mode=TwoWay}" PlaceholderText="\\HOST\share\music or C:\Music"/>
                    <Button Grid.Column="1" Content="Browse…" Click="OnBrowseClick"/>
                </Grid>
            </StackPanel>

            <!-- Step 2 -->
            <StackPanel Visibility="{x:Bind IsStep(2, ViewModel.CurrentStep), Mode=OneWay}" Spacing="12">
                <TextBlock Text="Plug in your iPod" Style="{StaticResource SubtitleTextBlockStyle}"/>
                <TextBlock Text="Connect your iPod via USB. ipod-sync will identify it and remember its serial." TextWrapping="Wrap" Opacity="0.7"/>
                <Border BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="8" Padding="16" Margin="0,8,0,0">
                    <StackPanel>
                        <TextBlock Text="✓ Detected"
                                   Foreground="Green"
                                   FontWeight="SemiBold"
                                   Visibility="{x:Bind HasDetection(ViewModel.DetectedIpod), Mode=OneWay}"/>
                        <TextBlock Text="{x:Bind ViewModel.DetectedIpod.ModelLabel, Mode=OneWay, FallbackValue='(no iPod yet)'}"
                                   FontSize="16" Margin="0,4,0,0"/>
                        <TextBlock Text="{x:Bind FormatSerial(ViewModel.DetectedIpod), Mode=OneWay}"
                                   FontFamily="Consolas" Opacity="0.7" Margin="0,4,0,0"/>
                        <TextBlock Text="{x:Bind ViewModel.ScanError, Mode=OneWay}" Foreground="Red" TextWrapping="Wrap" Margin="0,8,0,0"/>
                        <Button Content="Retry scan" Click="OnRetryScan" Margin="0,8,0,0" HorizontalAlignment="Left"/>
                    </StackPanel>
                </Border>
            </StackPanel>

            <!-- Step 3 -->
            <StackPanel Visibility="{x:Bind IsStep(3, ViewModel.CurrentStep), Mode=OneWay}" Spacing="12">
                <TextBlock Text="You're ready to sync" Style="{StaticResource SubtitleTextBlockStyle}"/>
                <TextBlock Text="ipod-sync will live in your system tray. Adjust settings later from Settings." TextWrapping="Wrap" Opacity="0.7"/>
                <Border BorderBrush="{ThemeResource CardStrokeColorDefaultBrush}" BorderThickness="1" CornerRadius="8" Padding="16">
                    <StackPanel Spacing="6">
                        <Grid ColumnDefinitions="120,*">
                            <TextBlock Grid.Column="0" Text="Source" Opacity="0.7"/>
                            <TextBlock Grid.Column="1" Text="{x:Bind ViewModel.SourcePath, Mode=OneWay}" TextWrapping="Wrap"/>
                        </Grid>
                        <Grid ColumnDefinitions="120,*">
                            <TextBlock Grid.Column="0" Text="iPod" Opacity="0.7"/>
                            <TextBlock Grid.Column="1" Text="{x:Bind FormatIpodSummary(ViewModel.DetectedIpod), Mode=OneWay}"/>
                        </Grid>
                    </StackPanel>
                </Border>
            </StackPanel>
        </Grid>

        <!-- Footer buttons -->
        <Grid Grid.Row="2" Padding="32,16,32,24" ColumnDefinitions="Auto,*,Auto,Auto" ColumnSpacing="8">
            <Button Grid.Column="0" Content="Cancel" Click="OnCancelClick"/>
            <Button Grid.Column="2" Content="← Back" Command="{x:Bind ViewModel.BackCommand}"/>
            <Button Grid.Column="3" Content="Next →" Command="{x:Bind ViewModel.NextCommand}"
                    Visibility="{x:Bind NotStep(3, ViewModel.CurrentStep), Mode=OneWay}"/>
            <Button Grid.Column="3" Content="Finish" Style="{StaticResource AccentButtonStyle}"
                    Command="{x:Bind ViewModel.FinishCommand}"
                    Visibility="{x:Bind IsStep(3, ViewModel.CurrentStep), Mode=OneWay}"/>
        </Grid>
    </Grid>
</Window>
```

- [ ] **Step 7: Wire WizardWindow code-behind**

Replace `ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs`:

```csharp
using System;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace IpodSync_UI.Views;

public sealed partial class WizardWindow : Window
{
    public WizardViewModel ViewModel { get; }

    public WizardWindow()
    {
        ViewModel = new WizardViewModel(
            scanFunc: ScanForIpodViaDaemon,
            sendConfigFunc: SaveConfigViaDaemon);
        ViewModel.WizardFinished += OnWizardFinished;
        this.InitializeComponent();
    }

    private IpodIdentityCandidate? ScanForIpodViaDaemon()
    {
        // M2: synchronous polling via the daemon. The wizard sends a
        // SubscribeDeviceEvents command (M3 wires actual events) then
        // immediately uses the M2 polling fallback through SaveConfig's
        // implicit detection. For M2 simplicity, fall back to scanning
        // drive letters in-process.
        // M3 will replace this with daemon-emitted device events.
        return ScanLocalDrives();
    }

    private static IpodIdentityCandidate? ScanLocalDrives()
    {
        for (char letter = 'A'; letter <= 'Z'; letter++)
        {
            var drive = $"{letter}:\\";
            if (!System.IO.Directory.Exists(drive)) continue;
            var sysInfo = System.IO.Path.Combine(drive, "iPod_Control", "Device", "SysInfo");
            if (!System.IO.File.Exists(sysInfo)) continue;
            try
            {
                var text = System.IO.File.ReadAllText(sysInfo);
                var serial = ParseField(text, "FirewireGuid");
                if (serial is null) continue;
                var model = ParseField(text, "ModelNumStr") ?? "";
                var label = DescribeModel(model);
                return new IpodIdentityCandidate(serial, label, drive);
            }
            catch { /* skip */ }
        }
        return null;
    }

    private static string? ParseField(string text, string key)
    {
        foreach (var line in text.Split('\n'))
        {
            var trimmed = line.Trim();
            if (trimmed.StartsWith(key, StringComparison.OrdinalIgnoreCase))
            {
                var rest = trimmed.Substring(key.Length).TrimStart(':', ' ').Trim();
                if (!string.IsNullOrEmpty(rest)) return rest;
            }
        }
        return null;
    }

    private static string DescribeModel(string modelNum)
    {
        var upper = modelNum.TrimStart('x').ToUpperInvariant();
        return upper switch
        {
            "MB029" or "MB147" or "MB565" => $"iPod Classic 7G ({upper})",
            _ when !string.IsNullOrEmpty(upper) => $"iPod ({upper})",
            _ => "iPod (model unknown)",
        };
    }

    private async Task SaveConfigViaDaemon(SaveConfigPayload payload)
    {
        if (App.Daemon is null) return;
        await App.Daemon.SendAsync(new SaveConfigCommand(
            Source: payload.Source,
            Ipod: new IpodIdentity(payload.IpodSerial, payload.IpodModelLabel)));
    }

    private void OnWizardFinished() => this.Close();

    private async void OnBrowseClick(object sender, RoutedEventArgs e)
    {
        var picker = new FolderPicker();
        picker.FileTypeFilter.Add("*");
        InitializeWithWindow.Initialize(picker, App.WindowHandle);
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null) ViewModel.SourcePath = folder.Path;
    }

    private void OnRetryScan(object sender, RoutedEventArgs e) => ViewModel.TriggerScanCommand.Execute(null);

    private void OnCancelClick(object sender, RoutedEventArgs e) => this.Close();

    // x:Bind helper accessors
    public Visibility IsStep(int n, int current) => n == current ? Visibility.Visible : Visibility.Collapsed;
    public Visibility NotStep(int n, int current) => n == current ? Visibility.Collapsed : Visibility.Visible;
    public Brush StepDotFill(int n, int current)
        => new SolidColorBrush(n <= current
            ? Microsoft.UI.Colors.SteelBlue
            : Microsoft.UI.Colors.LightGray);
    public Visibility HasDetection(IpodIdentityCandidate? ipod)
        => ipod is null ? Visibility.Collapsed : Visibility.Visible;
    public string FormatSerial(IpodIdentityCandidate? ipod)
        => ipod is null ? "" : $"Serial: {ipod.Serial}";
    public string FormatIpodSummary(IpodIdentityCandidate? ipod)
        => ipod is null ? "(none)" : $"{ipod.ModelLabel} · {ipod.Serial}";
}
```

- [ ] **Step 8: Build, expect clean**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug 2>&1 | Select-String -Pattern "0 Error|Build FAILED" | Select-Object -Last 2
```

Expected: clean.

- [ ] **Step 9: Verify all tests pass**

```powershell
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --logger "console;verbosity=minimal" 2>&1 | Select-Object -Last 5
```

Expected: 42 → 47 tests (added 5 WizardViewModelTests).

- [ ] **Step 10: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows/IpodSync.UI/ViewModels/WizardViewModel.cs ui-windows/IpodSync.UI/Views/WizardWindow.xaml ui-windows/IpodSync.UI/Views/WizardWindow.xaml.cs ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs ui-windows/IpodSync.UI.Tests/IpodSync.UI.Tests.csproj
git -C F:\repos\ipod-sync commit -m "feat(ui-windows): 3-step first-launch wizard with iPod polling fallback"
```

---

## Task 13: Documentation update — IPC protocol v1.1.0

**Files:**
- Modify: `F:\repos\ipod-sync\docs\ipc-protocol.md`

Appends the v1.1.0 daemon-extension section to the existing IPC protocol doc.

- [ ] **Step 1: Append v1.1.0 section**

At the end of `docs/ipc-protocol.md`, add:

```markdown

---

## v1.1.0 — Daemon extensions (UI ↔ daemon channel)

When the wire transport is the named pipe `\\.\pipe\ipod-sync` (Windows)
or Unix domain socket `~/.ipod-sync/daemon.sock` (macOS/Linux), the
daemon emits `hello` with `protocol_version = "1.1.0"`. The v1.0.0 envelope
shape is unchanged; v1.1.0 only adds new event and command types.

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `status_update` | `state` (idle/syncing), `configured` (bool), `ipod_connected` (bool), `last_sync` (HistoryEntry?), `next_scheduled_unix_secs` (u64?) |
| `config_update` | `source` (str?), `daemon` (DaemonSettings?), `ipod` (IpodIdentity?) |
| `history_update` | `entries` (HistoryEntry[]) |
| `device_connected` | `serial` (str), `model_label` (str), `drive` (str) |
| `device_disconnected` | `serial` (str) |
| `sync_rejected` | `reason` ("already_syncing" | "no_ipod" | "not_configured") |

### New commands (UI → daemon)

| Type | Fields |
|---|---|
| `get_status` | (none) — replies with `status_update` |
| `get_config` | (none) — replies with `config_update` |
| `save_config` | `source?` (str), `daemon?` (DaemonSettings), `ipod?` (IpodIdentity) — replies with `config_update` |
| `trigger_sync` | `source` ("manual"/"scheduled"/"plug_in") — replies with `sync_rejected` or nothing (sync proceeds, sync events forwarded) |
| `get_history` | `limit` (default 10) — replies with `history_update` |
| `subscribe_device_events` | (none) — daemon starts forwarding `device_connected` events for any iPod, not just configured |
| `unsubscribe_device_events` | (none) |
| `shutdown` | (none) — daemon exits cleanly after draining current sync |

### Forwarded sync-subprocess events

When the daemon is running a sync, it spawns `ipod-sync --ipc-mode --apply`
and forwards every v1.0.0 IpcEvent (`header`, `summary`, `review`, `prompt`,
`form`, `track_start`, `track_done`, `log`, `error`, `finish`) verbatim to
subscribed UI clients. UI clients see daemon events and sync events on the
same pipe and pattern-match on `type`.
```

- [ ] **Step 2: Commit**

```powershell
git -C F:\repos\ipod-sync add docs/ipc-protocol.md
git -C F:\repos\ipod-sync commit -m "docs(ipc): v1.1.0 daemon channel extensions"
```

---

## Task 14: User smoke test + LEARNINGS entry

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md`

User runs manual scenarios per spec §13 acceptance criteria #1-#7. The auto-sync scenarios (#2, #9) are M3; the popover/settings scenarios (#4, #5) are M4. M2's testable subset:

- [ ] **Scenario M2-1: Fresh install → wizard runs → completes → minimizes to tray**

Delete config file (or start from a clean state):

```powershell
Remove-Item "$env:APPDATA\ipod-sync\config.toml" -ErrorAction SilentlyContinue
Remove-Item "$env:APPDATA\ipod-sync\manifest.json" -ErrorAction SilentlyContinue
Remove-Item "$env:LOCALAPPDATA\ipod-sync" -Recurse -Force -ErrorAction SilentlyContinue
```

```powershell
cargo build --release
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.slnx -c Debug
dotnet run --project F:\repos\ipod-sync\ui-windows\IpodSync.UI\IpodSync.UI.csproj
```

Expected: wizard window opens, tray icon appears. Walk through:
1. Step 1 — Browse, pick a folder, click Next.
2. Step 2 — Plug in iPod (or have it already plugged in). Click Retry scan if needed. Detected card shows model + serial. Click Next.
3. Step 3 — Confirmation summary shows source + iPod. Click Finish.

After Finish: wizard closes, tray icon stays. config.toml should exist with `[ipod]` section.

```powershell
Get-Content "$env:APPDATA\ipod-sync\config.toml"
```

Expected output includes `[ipod]` block with the detected serial.

- [ ] **Scenario M2-2: Re-launch with config present → starts hidden in tray**

Close the running app via tray Quit. Then re-launch:

```powershell
dotnet run --project F:\repos\ipod-sync\ui-windows\IpodSync.UI\IpodSync.UI.csproj
```

Expected: no wizard window. Tray icon appears. Right-click tray → menu has Quit. Click Quit → process exits cleanly.

- [ ] **Scenario M2-3: Daemon survives UI exit**

Re-launch:

```powershell
dotnet run --project F:\repos\ipod-sync\ui-windows\IpodSync.UI\IpodSync.UI.csproj
```

Open Task Manager → Details → look for `ipod-sync.exe`. Note the PID.

Close the UI by clicking the X on the wizard / settings window (if any open) OR by killing only the IpodSync.UI.exe process. Do NOT use Quit from tray.

The `ipod-sync.exe` daemon process should remain in Task Manager (orphaned UI parent, daemon still alive).

Verify:
```powershell
Get-Process ipod-sync -ErrorAction SilentlyContinue | Format-List Id, ProcessName
```

Expected: process exists with the same PID.

Re-launch UI again — it should connect to the existing daemon (no new daemon process spawned), and start hidden in tray.

Then Quit from tray. Daemon exits cleanly.

- [ ] **Scenario M2-4: Build + tests green**

```powershell
cargo test --lib 2>&1 | Select-String "test result"
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj --logger "console;verbosity=minimal" 2>&1 | Select-Object -Last 3
```

Expected: Rust 121+ tests pass; C# 47 tests pass.

- [ ] **Scenario M2-5: Append LEARNINGS entry**

Add to top of `LEARNINGS.md`:

```markdown
## Phase 6 M2 gate (YYYY-MM-DD) — PASS / FAIL

- **Result:** PASS / FAIL (<reason>)
- **Scenario M2-1 (fresh install + wizard):** wizard window opened, 3 steps completed, config.toml written with [ipod] section
- **Scenario M2-2 (re-launch with config):** started hidden in tray, no wizard, tray Quit menu item works
- **Scenario M2-3 (daemon survives UI exit):** verified daemon process stays alive after UI closes, new UI launch reconnects to existing daemon
- **Scenario M2-4 (build + tests):** Rust N tests, C# M tests
- **Daemon binary path resolution:** worked / failed (notes)
- **iPod identification:** detected model + serial / required Retry / failed entirely
- **Observations:** (anything unexpected — slow startup, weird tray behavior, etc.)
```

- [ ] **Step 6: Commit LEARNINGS + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 6 M2 gate result"
git -C F:\repos\ipod-sync tag -a phase-6-m2-complete -m "Daemon foundation + first-launch wizard complete

- Rust --daemon mode boots; named-pipe IPC server (multi-client)
- ConfigService + HistoryService + DaemonState machine
- C# DaemonClient replaces M1 CoreProcess
- UI hidden-on-startup, spawns daemon if absent
- 3-step first-launch wizard with iPod polling fallback
- M1 MainPage / CoreLocator / AppController removed"
```

---

## Self-review

**Spec coverage check (against `2026-05-24-phase-6-daemon-model-design.md`):**

- §2 Architecture (Rust daemon brain, thin UI) → Task 7 (IPC server), Task 8 (daemon entry), Task 11 (UI hidden startup) ✓
- §3 Component breakdown → all rows ship in M2 except DeviceWatcher (deferred to M3 per plan scope), SyncOrchestrator (M3), Wizard subscribes to device events (M2 uses polling fallback explicitly documented) ✓
- §4 User flows: Flow 1 (wizard) is the M2 happy path ✓; Flows 2-5 land in M3+
- §5 Visual designs: wizard wireframe ✓; tray idle/offline ✓; popover + settings deferred to M4 per plan scope ✓
- §6 Configuration schema → Task 2 ✓
- §7 IPC protocol v1.1.0 → Tasks 6, 7, 13 ✓
- §8 Error handling: M2 covers daemon-unreachable (DaemonClient reconnect backoff) and config-corrupt (existing Phase 3.z handling carries forward via config_file::load); other error paths exercised in M3-M5 ✓
- §9 Testing strategy: Rust unit (Tasks 2-6, 8), C# unit (Tasks 9, 12), E2E smoke (Task 14) ✓
- §10 M2 milestone breakdown → mapped 1:1 to tasks ✓
- §13 Acceptance criteria #1, #3, #6, #7 are M2-scoped; #2, #4, #5, #8, #9, #10 are M3-M5

**Placeholder scan:** no `TBD` / `TODO` / vague "handle edge cases" steps. Each step has actual code or commands.

**Type consistency check:**
- `DaemonSettings` / `IpodIdentity` defined in Task 2, used in Tasks 6, 8, 9 ✓
- `HistoryEntry` defined in Task 3, used in Tasks 6, 8, 9 ✓
- `DaemonCommand` / `DaemonEvent` defined in Task 6 (Rust) and Task 9 (C#), wire-compatible by construction ✓
- `IpodIdentityCandidate` defined in Task 12 (C# WizardViewModel), used only within that task ✓
- `SaveConfigPayload` defined in Task 12, consumed by Task 12's `SaveConfigViaDaemon` → maps to `SaveConfigCommand` from Task 9 ✓

**Scope check:** plan is M2 only. M3 (DeviceWatcher real impl, auto-sync), M4 (notifications + popover + settings + history view), M5 (polish + distribution) are explicitly deferred. The wizard's polling fallback in Task 5 + Task 12 is the documented workaround for M2 not having a real DeviceWatcher.

**Concentrated complexity:** Tasks 7 (IPC server) and 12 (wizard XAML) are the meatiest. Both have full code blocks; implementer doesn't need to invent shape. Tasks 1-5 and 13-14 are short and mostly mechanical.

No new crate or NuGet dependencies beyond Tokio (Task 1) and H.NotifyIcon.WinUI (Task 10).
