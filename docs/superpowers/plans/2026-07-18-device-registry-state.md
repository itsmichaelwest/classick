# Device Registry and Serial-keyed State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Represent every remembered and connected iPod independently, target every device operation by serial, and retain one active sync at a time behind a future-replaceable admission policy.

**Architecture:** A durable `DeviceRegistry` and live serial-keyed inventory replace singleton identity/device state. The daemon publishes full `DeviceInventorySnapshot` values. `SessionAdmission` stores keyed session objects with capacity one. Swift reduces snapshots into `[DeviceSerial: DeviceViewState]`; views and actions receive an explicit serial.

**Tech Stack:** Rust, Tokio, serde JSON, daemon newline JSON IPC, Swift 6, SwiftUI, XCTest.

## Global Constraints

- Follow the execution-index constraints and preserve all dirty runtime/Swift changes.
- `canonical_serial_key` is comparison-only; preserve the raw serial on disk and wire.
- Legacy unscoped commands are accepted only with exactly one valid target; ambiguity is an explicit rejection.
- This plan attributes progress and completion but does not implement Plan 3's finalizing cancellation transaction or Plan 5's visual redesign.

---

### Task 1: Durable registry and per-device history

**Files:** Create `crates/classick/src/daemon/device_registry.rs`; modify `crates/classick/src/daemon/mod.rs`, `crates/classick/src/config_file.rs`, `crates/classick/src/daemon/history.rs`, `crates/classick/src/daemon/runtime.rs`.

**Interfaces:**

```rust
pub(crate) fn canonical_serial_key(serial: &str) -> String;
pub(crate) struct DeviceRegistry { path: PathBuf, records: BTreeMap<String, DeviceRecord> }
impl DeviceRegistry {
    pub(crate) fn load_or_migrate(path: PathBuf, legacy: Option<&IpodIdentity>) -> Result<Self>;
    pub(crate) fn records(&self) -> Vec<DeviceRecord>;
    pub(crate) fn observe(&mut self, identity: &DetectedIpod, now: u64) -> Result<()>;
    pub(crate) fn configure(&mut self, serial: &str) -> Result<()>;
    pub(crate) fn forget(&mut self, serial: &str) -> Result<()>;
}
```

`DeviceRecord` stores raw serial, name/model metadata, configured/last-seen flags, and selection/settings/subscription revisions. Add defaulted `serial` and `session_id` to history; expose `latest_attempt(serial)` and `latest_success(serial)`. Migrate legacy history to the migrated configured serial.

- [ ] Add tests for A migration, observing unconfigured B without replacing A, forget-B, canonical-key collision rejection, atomic revision bumps, and latest-success ignoring a newer failed/cancelled attempt.
- [ ] Run `cargo test -p classick device_registry` and confirm RED for the absent registry.
- [ ] Implement the registry/migration/history APIs and rerun the focused tests GREEN.
- [ ] Commit named files: `git commit -m "feat(daemon): add durable device registry"`.

### Task 2: Collection discovery and watcher diffs

**Files:** Modify `crates/classick/src/ipod/device.rs`, `crates/classick/src/daemon/device_watcher.rs`, `crates/classick/src/daemon/iokit_watcher.rs`.

```rust
pub fn scan_for_ipods() -> Vec<DetectedIpod>;
pub(crate) fn diff_inventory(
    previous: &HashMap<String, DetectedIpod>,
    current: Vec<DetectedIpod>,
) -> Vec<DeviceEvent>;
```

Retain `scan_for_ipod()` as a compatibility wrapper. Sort scans deterministically. Polling and IOKit signals always rescan the collection and diff maps; removal of A cannot emit removal of B.

- [ ] Replace the swap test with RED cases for initial A+B, removal A only, metadata update A only, and unrelated USB removal.
- [ ] Run `cargo test -p classick daemon::device_watcher`.
- [ ] Implement collection scanning/diffing and rerun it plus `cargo test -p classick ipod::device` GREEN.
- [ ] Commit: `git commit -m "fix(daemon): discover multiple connected iPods"`.

### Task 3: Snapshot and targeted IPC contract

**Files:** Create `crates/classick/src/ipc_device.rs`; modify `crates/classick/src/ipc_daemon.rs`, `crates/classick/src/lib.rs`, `docs/ipc-protocol.md`; split `ui/macos/Sources/Classick/Ipc/WireModels.swift` into `DaemonCommand.swift`, `DaemonEvent.swift`, and `SyncEvent.swift`; modify `ui/macos/Tests/ClassickTests/WireCodecTests.swift`.

```rust
pub struct DeviceInventorySnapshot {
    pub revision: u64,
    pub devices: Vec<DeviceSnapshot>,
}
pub enum DevicePhaseLabel { Disconnected, Unconfigured, Idle, Syncing, Paused, Error }
pub struct DeviceSnapshot {
    pub identity: DeviceIdentitySnapshot, pub configured: bool, pub connected: bool,
    pub mount: Option<String>, pub phase: DevicePhaseLabel, pub session_id: Option<SessionId>,
    pub storage: Option<StorageInfo>, pub synced_count: usize, pub library_count: Option<usize>,
    pub latest_successful_sync: Option<HistoryEntry>, pub latest_attempt: Option<HistoryEntry>,
    pub last_terminal_error: Option<String>, pub selection_revision: u64,
    pub settings_revision: u64, pub subscriptions_revision: u64,
}
```

`DeviceSnapshot` contains identity, configured/connected/mount, phase/session, storage/counts, latest successful sync, latest attempt, retained terminal error, and three device-config revisions. Add `config_revision` plus `acknowledged_request_id` to global `config_update`/`save_config`. Add optional serial/request ID to old command shapes; new Swift always sends them. `SyncEvent` carries optional serial plus session ID. Replies echo serial/request ID.

- [ ] Add exact old/new JSON RED tests, A+B snapshot round-trip, serial on every new mutating command, and echoed correlation fields in Rust and Swift.
- [ ] Run `cargo test -p classick ipc_daemon` and `cd ui/macos && swift test --filter WireCodecTests`.
- [ ] Implement the additive protocol, bump its minor version, document ambiguity rejection, regenerate Xcode project if split files require it, and rerun GREEN.
- [ ] Commit: `git commit -m "feat(ipc): add device inventory snapshots"`.

### Task 4: Isolated session admission and attributed progress

**Files:** Create `crates/classick/src/daemon/session_admission.rs`, `runtime_state.rs`, `command_handler.rs`; modify `daemon/state.rs`, `daemon/sync_orchestrator.rs`, `daemon/runtime.rs`, and all `DaemonDeps` fixtures.

```rust
pub type SessionId = u64;
pub struct SyncSession { pub id: SessionId, pub started_at_unix_secs: u64, pub trigger: SyncTrigger, pub serial: Option<String>, pub drive: Option<String>, pub kind: SessionKind }
impl SessionAdmission {
    pub fn single() -> Self;
    pub fn try_admit_device(&mut self, serial: &str, drive: &Path) -> Result<SyncSession, AdmissionRejection>;
    pub fn finish(&mut self, id: SessionId) -> bool;
}
pub struct EventContext { pub session_id: SessionId, pub serial: Option<String> }
```

Store sessions and control channels keyed by session ID even at capacity one. All orchestrator entry points receive `EventContext`; stale completion can finish only the matching ID.

- [ ] Add RED tests for capacity one, release/admit B, stale A completion, attributed A progress, and a serial-less scan occupying admission.
- [ ] Implement and run `cargo test -p classick daemon::session_admission` plus `cargo test -p classick daemon::sync_orchestrator` GREEN.
- [ ] Commit: `git commit -m "refactor(daemon): isolate sync admission by session"`.

### Task 5: Keyed runtime and exact targeting

**Files:** Modify `crates/classick/src/daemon/runtime.rs`, `runtime_state.rs`, `command_handler.rs`, `daemon/library.rs`; create `daemon/device_snapshot.rs`; create `crates/classick/tests/daemon_multi_device_integration.rs`.

Replace `connected: Option<_>` with a canonical-key map. Build previews/counts/config for an explicit serial. Snapshot after handshake and every device/session/history/config mutation. Rejections cover unknown, disconnected, unconfigured, occupied, stale session, and ambiguous legacy targets. Completion updates only its serial.

- [ ] Add platform-neutral RED integration tests for A+B, disconnect A preserving B, syncing B's drive, unknown B never targeting A, unconfigured B coexistence, rejection correlation, isolated completion, and full fresh-client snapshot.
- [ ] Implement runtime routing and run `cargo test -p classick --test daemon_multi_device_integration` then full `cargo test -p classick` GREEN.
- [ ] Commit: `git commit -m "feat(daemon): target device commands by serial"`.

### Task 6: Swift keyed reducer

**Files:** Create `ui/macos/Sources/Classick/Model/DeviceViewState.swift`, `AppModel+DeviceReducer.swift`, `ui/macos/Tests/ClassickTests/DeviceInventoryReducerTests.swift`; modify `AppModel.swift` and existing reducer tests.

```swift
typealias DeviceSerial = String
enum DevicePhase: Equatable { case disconnected, unconfigured, idle, syncing, paused, error(String) }
struct DeviceViewState: Equatable {
    var identity: DeviceIdentityWire; var configured: Bool; var connected: Bool
    var mountPath: String?; var phase: DevicePhase; var sessionID: UInt64?
    var storage: StorageWire?; var syncedCount: Int; var libraryCount: Int?
    var latestSuccessfulSync: HistoryEntryWire?; var latestAttempt: HistoryEntryWire?
    var lastTerminalError: String?; var config: DeviceConfigState?; var preview: DevicePreview?
    var selectionRevision: UInt64; var settingsRevision: UInt64; var subscriptionsRevision: UInt64
}
var devices: [DeviceSerial: DeviceViewState]
```

Snapshot collections are authoritative. Route progress only by serial/session. Plan 1 does not decide terminal semantics: it stores raw rollups and waits for a later authoritative snapshot; Plan 3 defines completed/cancelled/paused/aborted transitions. Command focus priority is active session, selected destination, sole connected; multiple connected without selection returns nil.

- [ ] Add RED reducer tests for A+B, disconnect A, remembered A plus unconfigured B, A progress isolation, stale-session ignore, non-terminal finish, atomic terminal snapshot, and no-guess focus.
- [ ] Implement; run `cd ui/macos && xcodegen generate`; inspect and include `ui/macos/Classick.xcodeproj/project.pbxproj`; run `swift test --filter DeviceInventoryReducerTests` and `swift test --filter AppModelReducerTests` GREEN.
- [ ] Commit: `git commit -m "refactor(ui): model devices by serial"`.

### Task 7: Retarget current macOS surfaces and gate

**Files:** Modify `ui/macos/Sources/Classick/Views/Sidebar.swift`, `MainWindow.swift`, `DeviceMusicPage.swift`, `DeviceSettingsPage.swift`, `MenuContent.swift`, `DeviceRow.swift`, `SetupWindow.swift`, `HistoryView.swift`, `ui/macos/Sources/Classick/ClassickApp.swift`, `PreviewFixtures.swift`, `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`, `DeviceMusicLogicTests.swift`, `WireCodecTests.swift`.

Sidebar iterates sorted devices and stores expansion by serial. Every action closure accepts serial. Page reads remain pinned to their selected serial when B connects. A multi-connected/no-focus menu cannot emit Sync Now.

- [ ] Add RED tests for sorted/preserved sidebar inventory, targeted action encoding, selected A surviving B, and no unscoped multi-device sync.
- [ ] Implement without performing Plan 5's visual redesign.
- [ ] Run `cargo test -p classick`, `cd ui/macos && swift test`, and the macOS-15-floor Xcode build from the index.
- [ ] Perform read-only live discovery only; do not sync the mounted iPod before Plan 3.
- [ ] Commit: `git commit -m "fix(ui): preserve multiple devices across app surfaces"`.
