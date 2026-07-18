# macOS State and UI Stabilization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove remaining stale editor/terminal state, make playlist deletion truthful, consolidate every device-row phase into the approved geometry, and correct menu-bar icon sizing.

**Architecture:** Daemon writes are acknowledged with revisions/request IDs; generic `AcknowledgedDraft` reconciles local edits. Only Plan 1 authoritative device snapshots drive terminal UI state. `DeviceRowPresentation` and `MenuBarLabelPresentation` are pure tested values rendered by stable SwiftUI shells.

**Tech Stack:** Rust daemon stores, daemon JSON IPC protocol 2.0.0, Swift 6 observable model, SwiftUI/AppKit, deterministic previews and XCTest.

## Global Constraints

- Depends on Plans 1–4. Reuse their registry, request IDs, revisions, finalizing phase, and durable intents.
- Preserve useful dirty DeviceRow/icon work, but split files below roughly 500 lines.
- The Figma-like attached reference controls the stable card geometry; no new interaction permission is required for implementation.
- Visual verification on macOS 27 cannot prove the macOS 15 fallback; use a macOS 15 VM for that branch.

---

### Task 1: Transactional playlist deletion

**Files:** Create `crates/classick/src/daemon/playlist_commands.rs`, `crates/classick/tests/playlist_deletion_integration.rs`; modify `crates/classick/src/daemon/mod.rs`, `runtime.rs`, `ipc_daemon.rs`, `docs/ipc-protocol.md`.

```rust
pub(crate) struct DeletePlaylistOutcome { pub request_id: String, pub deleted: bool, pub changed_revisions: BTreeMap<String, u64> }
pub(crate) fn delete_and_scrub_subscriptions(store: &PlaylistStore, registry: &mut DeviceRegistry, state_root: &Path, slug: &str, request_id: &str) -> Result<DeletePlaylistOutcome>;
```

Use `iPod_Control`-independent host journal `devices/playlist-mutations/<request-id>.json` with staged paths, original hashes, target revisions, and phase. Stage every affected subscription file and playlist deletion, persist the journal, publish renames, then remove it; startup recovery rolls forward or restores by phase/hash. Missing playlist is an acknowledged no-op. `DeletePlaylistOutcome` echoes request ID and resulting per-serial subscription revisions. Broadcast only after success. Keep unresolved diagnostics for corruption/store failures but remove the normal-deletion warning copy.

- [ ] Add RED tests for scrubbing A+B, preserving unrelated/order/unchanged C, missing no-op, injected rollback, no success broadcast, and clean next preview.
- [ ] Implement and run `cargo test -p classick --test playlist_deletion_integration` then `cargo test -p classick` GREEN.
- [ ] Commit: `git commit -m "fix(daemon): scrub device subscriptions on playlist deletion"`.

### Task 2: Canonical daemon acknowledgements

**Files:** Modify `crates/classick/src/daemon/runtime.rs`, `command_handler.rs`, `device_registry.rs`, `crates/classick/src/playlist_store.rs`, `config_file.rs`, `ipc_daemon.rs`, `docs/ipc-protocol.md`, `crates/classick/tests/daemon_runtime_integration.rs`, `ui/macos/Sources/Classick/Ipc/DaemonCommand.swift`, `DaemonEvent.swift`, `ui/macos/Tests/ClassickTests/WireCodecTests.swift`.

Every `save_config`, `save_device_config`, `save_playlist`, `delete_playlist`, `get_playlist`, and `resolve_tracks` carries `request_id`. Successful canonical events echo `acknowledged_request_id` and the relevant monotonic global/device/playlist revision after the atomic store write; failures echo request ID without advancing revision. Add exact JSON and persistence-before-ack RED tests, implement, and run `cargo test -p classick ipc_daemon`, `cargo test -p classick --test daemon_runtime_integration -- --test-threads=1`, and `cd ui/macos && swift test --filter WireCodecTests` GREEN.

- [ ] Commit: `git commit -m "feat(ipc): acknowledge persisted editor intents"`.

### Task 3: Acknowledged editor drafts

**Files:** Create `ui/macos/Sources/Classick/Model/AcknowledgedDraft.swift`, `ui/macos/Tests/ClassickTests/AcknowledgedDraftTests.swift`; modify `Views/DeviceMusicPage.swift`, `DeviceSettingsPage.swift`, `PlaylistPage.swift`, `SmartRulesEditor.swift`, `SettingsView.swift`, `Model/AppModel.swift`, `ClassickApp.swift`, `WireCodecTests.swift`, `AppModelReducerTests.swift`, `DeviceMusicLogicTests.swift`, `ui/macos/Classick.xcodeproj/project.pbxproj`.

```swift
struct AcknowledgedDraft<Value: Equatable>: Equatable {
    private(set) var value: Value
    private(set) var canonicalRevision: UInt64
    private(set) var submitted: [String: SubmittedDraft<Value>]
    private(set) var isDirty: Bool
    mutating func edit(_ value: Value)
    mutating func markSubmitted(requestID: String)
    mutating func reconcile(canonical: Value, revision: UInt64, acknowledgedRequestID: String?)
}
```

Each local edit increments a generation; `SubmittedDraft` records request ID, generation, and value. Use explicit binding setters. Programmatic seed never writes. Ack-A after B submission cannot clean or replace B; ack-B may clean it. Stale revisions cannot roll back. Device, playlist, and global config snapshots carry revision/acknowledged request ID. Remove permanent `userEdited`, `seededFromModel`, and fragile `isSeeding` latches.

- [ ] Add RED truth-table tests including edit-A/submit-A/edit-B/submit-B/ack-A/ack-B, deletion scrub not resurrecting a slug, and global Settings pending-edit preservation.
- [ ] Implement; run `cd ui/macos && xcodegen generate`; inspect/include the pbxproj; run `swift test --filter AcknowledgedDraftTests`, `swift test --filter AppModelReducerTests`, and full `swift test` GREEN.
- [ ] Commit: `git commit -m "refactor(ui): reconcile editor drafts through daemon acknowledgements"`.

### Task 4: Authoritative terminal-state consumers

**Files:** Create `ui/macos/Sources/Classick/Model/SyncNotificationCoordinator.swift`; modify `ui/macos/Sources/Classick/Model/AppModel.swift`, `ClassickApp.swift`, `Views/MenuContent.swift`, `DeviceMusicPage.swift`, `DeviceSettingsPage.swift`, `HistoryView.swift`, `Notifications/Notifier.swift`, `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`, `NotifierPolicyTests.swift`, `ui/macos/Classick.xcodeproj/project.pbxproj`.

Plan 3 owns authoritative terminal transitions. This task updates remaining consumers: null attempt data never clears latest success; errors persist until Details dismissal/retry/disconnect/later success; notification coordinator fires once per terminal session ID; toolbar, Settings, row, menu, and history read the same latest-successful field.

- [ ] Add RED tests for Last-synced preservation, early finish, atomic terminal snapshot, no cancellation completion notification, retained error, later success, scan suppression, and duplicate terminal de-duplication.
- [ ] Implement; run `cd ui/macos && xcodegen generate`; run `swift test --filter AppModelReducerTests`, `swift test --filter NotifierPolicyTests`, and full `swift test` GREEN.
- [ ] Commit: `git commit -m "fix(ui): derive sync completion from authoritative device snapshots"`.

### Task 5: Consolidated `DeviceRowPresentation`

**Files:** Create `ui/macos/Sources/Classick/Model/DeviceRowPresentation.swift`, `ui/macos/Sources/Classick/Views/DeviceIcon.swift`, `ui/macos/Tests/ClassickTests/DeviceRowPresentationTests.swift`; modify `Views/DeviceRow.swift`, `MainWindow.swift`, `PreviewFixtures.swift`, `ui/macos/Tests/ClassickTests/DeviceIconLogicTests.swift`, `ui/macos/Classick.xcodeproj/project.pbxproj`.

```swift
struct DeviceRowPresentation: Equatable {
    enum Meter: Equatable { case capacity(used: UInt64, total: UInt64, projectedUsed: UInt64?); case progress(current: Int,total: Int,label: String?,etaSeconds: UInt64?); case indeterminate(label: String?); case unavailable }
    enum Action: Equatable { case syncNow, pause, cancel, resume, retry, details, setUp }
    var serial: String?
    var title: String
    var subtitle: String
    var caption: String?
    var meter: Meter
    var primaryAction: Action?
    var secondaryAction: Action?
    static func make(device: DeviceViewState?, libraryCount: Int?) -> Self
}
```

Visual reference: `/Users/michael/Library/Application Support/CleanShot/media/media_tGYNlNyqdW/CleanShot 2026-07-18 at 14.57.01@2x.png`. Use 20pt outer inset, 16pt corner radius, 16pt horizontal/10pt vertical inner padding, 40pt artwork, 12pt header-to-meter spacing, 6pt meter height, a large trailing control, and reserved one-line caption height. All phases share title/subtitle, stable actions, and meter/caption slots. Finalizing uses required copy; errors/disconnect retain identity. Persistent-row presentation chooses selected serial, then active serial; with no selection/session it may show an actionless aggregate/remembered card, but command targeting remains nil when multiple devices are connected and never selects an arbitrary first device.

- [ ] Add RED truth-table tests for every phase, long content structure, finalizing copy, disconnected/error identity, and deterministic device selection.
- [ ] Implement the pure presentation and stable shell; keep row below 500 lines; regenerate Xcode project.
- [ ] Run Swift tests/build and inspect light/dark preview matrix at 600, 820, and 860pt with long strings.
- [ ] Commit: `git commit -m "refactor(ui): render every device state through one row presentation"`.

### Task 6: Fixed menu-bar label and final gates

**Files:** Create `ui/macos/Sources/Classick/Views/MenuBarLabel.swift`, `ui/macos/Tests/ClassickTests/MenuBarLabelLogicTests.swift`; modify `ui/macos/Sources/Classick/ClassickApp.swift`, `Views/MenuContent.swift`, `PreviewFixtures.swift`, `ui/macos/Classick.xcodeproj/project.pbxproj`.

```swift
struct MenuBarLabelPresentation: Equatable {
    var systemImage: String
    var accessibilityLabel: String
    static func make(phase: DevicePhase?) -> Self
}
```

Use custom-label `MenuBarExtra`; monochrome medium-weight glyph in fixed 18×18 optical frame, accessibility label “Classick,” stable footprint across phases.

- [ ] Add RED phase mapping/footprint/accessibility tests and previews for idle, syncing, finalizing, paused, scanning, error.
- [ ] Implement and run the full index gate: Rust, Swift, Xcode macOS-15 floor, bundle.
- [ ] Verify device row/menu icon optically on macOS 27 and fallback in macOS 15 VM. Use previews/event injection where interaction permissions are unavailable.
- [ ] Only now perform the mounted-iPod sync/cancel/artwork/eject/playback gate from the index; keep the music share read-only.
- [ ] Commit: `git commit -m "fix(ui): align device row and menu bar status presentation"`.
