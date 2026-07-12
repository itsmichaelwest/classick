# macOS SwiftUI App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a native macOS menu-bar app (`Classick.app`) that owns the `classick` daemon and gives a usable daily-driver iPod-sync experience over the proven v1.1.0 IPC — zero Rust changes.

**Architecture:** `DaemonClient` (actor, owns the Unix socket) → `AppModel` (`@Observable`, reduces `DaemonEvent`s into UI state) → SwiftUI scenes (`MenuBarExtra .menu`, `Settings`, a setup window, a prompt alert). The app spawns/owns `classick --daemon`. Surface-agnostic model so the rich-panel `.window` style is a later view-only swap.

**Tech Stack:** Swift 6.3, SwiftUI, SwiftPM (executable) + `bundle.sh` → `Classick.app`, `UserNotifications`, `SMAppService`. Xcode 26.6 toolchain, macOS 15 deployment floor.

## Global Constraints

- **Deployment target macOS 15.0.** Gate 26-only APIs (Liquid Glass) with `if #available(macOS 26, *)`. Swift 6 language mode, strict concurrency.
- **Zero Rust changes.** The wire contract is `docs/ipc-protocol.md`; the daemon is the store of record for all config. The app never writes the TOML.
- **Socket path:** `NSTemporaryDirectory() + "classick.sock"` (= the confstr `$TMPDIR/classick.sock` SP1 pinned). This is the IPC contract.
- **Not sandboxed** (raw device access via daemon + shared `$TMPDIR` socket). No App Sandbox entitlement.
- **Wire discriminator:** every command/event is a JSON object with a snake_case `"type"` field. Commands are newline-terminated. Flush after every write.
- **Exact wire types (verbatim field names):**
  - `DaemonCommand` (sent): `subscribe_device_events` · `get_status` · `get_config` · `save_config {source:String?, daemon:DaemonSettings?, ipod:IpodIdentity?}` · `forget_ipod` · `trigger_sync {source:"manual"|"scheduled"|"plug_in"}` · `cancel_sync` · `decide_prompt {id:UInt64, choice:Int32}`.
  - `DaemonSettings`: `{enabled:Bool, autostart_with_windows:Bool, first_sync_mode:"review"|"auto_apply", subsequent_sync_mode:"review"|"auto_apply", schedule_minutes:UInt32, notify_on:"all"|"errors_only"|"none"}`. *(`autostart_with_windows` is the launch-at-login flag — reuse the field name as-is; renaming is a Rust change.)*
  - `IpodIdentity`: `{serial:String, model_label:String, name:String?}`.
  - `DaemonEvent` (received): `hello {protocol_version, core_version}` · `status_update {state:"idle"|"syncing", configured:Bool, ipod_connected:Bool, last_sync:HistoryEntry?, next_scheduled_unix_secs:UInt64?, storage:StorageInfo?}` · `config_update {source:String?, daemon:DaemonSettings?, ipod:IpodIdentity?}` · `history_update {entries:[HistoryEntry]}` · `device_connected {serial, model_label, drive, name:String?}` · `device_disconnected {serial}` · `sync_rejected {reason:"already_syncing"|"no_ipod"|"not_configured"|"too_many_failures"}` · `sync_event {line:String}`.
  - `HistoryEntry` (as observed on the wire): `{timestamp:String, duration_secs:UInt64, trigger:String, outcome:String, summary:{add,modify,remove,unchanged,skipped:Int}}`.
  - Inner `sync_event.line` (v1.0.0 `IpcEvent`, snake_case `type`): `hello` · `header {source, ipod, manifest}` · `summary {add, modify, metadata_only, remove, unchanged, total_planned}` · `track_start {current, total, label}` · `track_done` · `log {message}` · `prompt {id, message, options:[String]}` · `form {id, label, initial, hint}` · `error {message, recovery_hints:[String]?}` · `finish {success}`.
- **`storage` is always `None` on macOS** — compute it app-side from the iPod `drive` path via `URLResourceValues` (`.volumeAvailableCapacityKey`/`.volumeTotalCapacityKey`). Never rely on `status_update.storage`.
- **No `review` handling in v1** — daemon-triggered syncs `--apply` (verified in SP1). The `prompt`/`form` events (source-change safeguard, per-track errors) MUST still be handled or a sync stalls.

---

## File Structure

Under `ui/macos/`:

- `Package.swift` **(create)** — executable target `Classick`, test target `ClassickTests`, macOS 15 platform.
- `bundle.sh` **(create)** — assemble `Classick.app` (Info.plist w/ `LSUIElement`, executable, embed `classick`), ad-hoc sign.
- `Info.plist` **(create)** — `LSUIElement=true`, bundle id `com.classick.app`, version.
- `Sources/Classick/ClassickApp.swift` **(create)** — `@main App`; `MenuBarExtra` + `Settings` + setup `Window`; owns `AppModel`.
- `Sources/Classick/Ipc/WireModels.swift` **(create)** — `Codable` command/event/nested types + inner `SyncEvent`.
- `Sources/Classick/Ipc/DaemonClient.swift` **(create)** — `actor`: connect, handshake, `send`, event `AsyncStream`, reconnect.
- `Sources/Classick/Model/AppModel.swift` **(create)** — `@Observable @MainActor`; `apply(_:DaemonEvent)` reducer + derived UI state.
- `Sources/Classick/Model/Storage.swift` **(create)** — `storageFor(drive:) -> (free, total)?`.
- `Sources/Classick/Daemon/DaemonProcess.swift` **(create)** — locate + spawn + own `classick --daemon`; attach-if-running.
- `Sources/Classick/Views/MenuContent.swift` **(create)** — the menu rows per state.
- `Sources/Classick/Views/SetupWindow.swift` **(create)** — first-run.
- `Sources/Classick/Views/SettingsView.swift` **(create)** — General + About.
- `Sources/Classick/Views/PromptAlert.swift` **(create)** — modal for `prompt`/`form`.
- `Sources/Classick/Notifications/Notifier.swift` **(create)** — `UserNotifications`.
- `Tests/ClassickTests/WireCodecTests.swift` **(create)**.
- `Tests/ClassickTests/AppModelReducerTests.swift` **(create)**.
- `ui/macos/README.md` **(create)** — build/run.

---

## Task 1: SwiftPM scaffold + bundle → launches as menu-bar agent

**Files:** Create `ui/macos/Package.swift`, `Info.plist`, `bundle.sh`, `Sources/Classick/ClassickApp.swift`.

**Interfaces:**
- Produces: a buildable `Classick` executable and a `bundle.sh` producing `Classick.app`.

- [ ] **Step 1: Package.swift**

```swift
// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Classick",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(name: "Classick", path: "Sources/Classick"),
        .testTarget(name: "ClassickTests", dependencies: ["Classick"], path: "Tests/ClassickTests"),
    ]
)
```

- [ ] **Step 2: Minimal menu-bar app**

`Sources/Classick/ClassickApp.swift`:

```swift
import SwiftUI

@main
struct ClassickApp: App {
    var body: some Scene {
        MenuBarExtra("Classick", systemImage: "ipod") {
            Text("Classick")
            Button("Quit") { NSApplication.shared.terminate(nil) }
        }
        .menuBarExtraStyle(.menu)
    }
}
```

- [ ] **Step 3: Info.plist + bundle.sh**

`Info.plist` sets `LSUIElement=true`, `CFBundleIdentifier=com.classick.app`, `CFBundleExecutable=Classick`, `LSMinimumSystemVersion=15.0`. `bundle.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
swift build -c release
APP="Classick.app"; rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ".build/release/Classick" "$APP/Contents/MacOS/Classick"
# Dev: embed the freshly built daemon binary so the app can spawn it.
cp ../../target/release/classick "$APP/Contents/Resources/classick" 2>/dev/null || \
  echo "warn: target/release/classick not found (run cargo build --release)"
codesign --force --deep --sign - "$APP"   # ad-hoc sign (real signing = SP3)
echo "built $PWD/$APP"
```

- [ ] **Step 4: Build the bundle**

Run: `chmod +x ui/macos/bundle.sh && ui/macos/bundle.sh`
Expected: prints "built …/Classick.app", no errors.

- [ ] **Step 5: Verify it launches as an agent**

Run: `open ui/macos/Classick.app` then `pgrep -x Classick`.
Expected: an "ipod" icon appears in the menu bar, **no Dock icon**, `pgrep` shows it running. Click it → menu with "Classick" + Quit. Quit via the menu.

- [ ] **Step 6: Commit**

```bash
git add ui/macos/Package.swift ui/macos/Info.plist ui/macos/bundle.sh ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): SwiftPM menu-bar app scaffold + bundle script"
```

---

## Task 2: Wire models (Codable) — TDD against real JSON

**Files:** Create `Sources/Classick/Ipc/WireModels.swift`, `Tests/ClassickTests/WireCodecTests.swift`.

**Interfaces:**
- Produces: `enum DaemonCommand: Encodable`, `enum DaemonEvent: Decodable`, `struct DaemonSettings`, `struct IpodIdentity`, `struct HistoryEntry`, `enum SyncEvent: Decodable` (inner v1.0.0), all matching the Global Constraints wire types.

- [ ] **Step 1: Write failing round-trip tests using authoritative samples**

`Tests/ClassickTests/WireCodecTests.swift` (samples are real daemon output captured in SP1):

```swift
import XCTest
@testable import Classick

final class WireCodecTests: XCTestCase {
    func testDecodesDeviceConnected() throws {
        let json = #"{"type":"device_connected","serial":"0x000A27002138B0A8","model_label":"iPod Classic (3rd gen)","drive":"/Volumes/IPOD","name":"Michael’s iPod"}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .deviceConnected(serial, model, drive, name) = ev else { return XCTFail() }
        XCTAssertEqual(serial, "0x000A27002138B0A8")
        XCTAssertEqual(model, "iPod Classic (3rd gen)")
        XCTAssertEqual(drive, "/Volumes/IPOD")
        XCTAssertEqual(name, "Michael’s iPod")
    }

    func testDecodesStatusUpdateMinimal() throws {
        let json = #"{"type":"status_update","state":"idle","configured":false,"ipod_connected":true}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .statusUpdate(s) = ev else { return XCTFail() }
        XCTAssertEqual(s.state, .idle); XCTAssertTrue(s.ipodConnected); XCTAssertNil(s.storage)
    }

    func testDecodesSyncEventWrappingSummary() throws {
        let inner = #"{\"type\":\"summary\",\"add\":0,\"modify\":0,\"metadata_only\":0,\"remove\":0,\"unchanged\":12,\"total_planned\":0}"#
        let json = "{\"type\":\"sync_event\",\"line\":\"\(inner)\"}"
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .syncEvent(line) = ev else { return XCTFail() }
        let sub = try JSONDecoder().decode(SyncEvent.self, from: Data(line.utf8))
        guard case let .summary(add, _, _, _, unchanged, _) = sub else { return XCTFail() }
        XCTAssertEqual(add, 0); XCTAssertEqual(unchanged, 12)
    }

    func testEncodesSaveConfig() throws {
        let cmd = DaemonCommand.saveConfig(
            source: "/music", daemon: nil,
            ipod: IpodIdentity(serial: "0xABC", modelLabel: "iPod Classic (3rd gen)", name: nil))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "save_config")
        XCTAssertEqual(obj["source"] as? String, "/music")
        XCTAssertEqual((obj["ipod"] as? [String:Any])?["serial"] as? String, "0xABC")
    }

    func testEncodesTriggerSync() throws {
        let data = try JSONEncoder().encode(DaemonCommand.triggerSync(source: .manual))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "trigger_sync")
        XCTAssertEqual(obj["source"] as? String, "manual")
    }
}
```

- [ ] **Step 2: Run — verify it fails to compile (types undefined)**

Run: `cd ui/macos && swift test`
Expected: compile error (no `DaemonEvent`/`DaemonCommand`).

- [ ] **Step 3: Implement WireModels.swift**

Implement all types with hand-written `Codable` using a `type` discriminator. Key shape (abbreviated — implement every case in the Global Constraints list):

```swift
import Foundation

struct IpodIdentity: Codable, Equatable {
    var serial: String
    var modelLabel: String
    var name: String?
    enum CodingKeys: String, CodingKey { case serial, modelLabel = "model_label", name }
}

struct DaemonSettings: Codable, Equatable {
    var enabled: Bool
    var autostartWithWindows: Bool
    var firstSyncMode: String        // "review" | "auto_apply"
    var subsequentSyncMode: String
    var scheduleMinutes: UInt32
    var notifyOn: String             // "all" | "errors_only" | "none"
    enum CodingKeys: String, CodingKey {
        case enabled, autostartWithWindows = "autostart_with_windows",
             firstSyncMode = "first_sync_mode", subsequentSyncMode = "subsequent_sync_mode",
             scheduleMinutes = "schedule_minutes", notifyOn = "notify_on"
    }
}

struct HistoryEntry: Codable, Equatable {
    var timestamp: String
    var durationSecs: UInt64
    var trigger: String
    var outcome: String
    enum CodingKeys: String, CodingKey {
        case timestamp, durationSecs = "duration_secs", trigger, outcome
    }
    // `summary` decoded leniently; not needed for v1 display beyond outcome.
}

struct StatusInfo: Equatable {
    enum State: String, Codable { case idle, syncing }
    var state: State
    var configured: Bool
    var ipodConnected: Bool
    var lastSync: HistoryEntry?
    var storage: Storage?            // always nil on macOS wire; see Storage.swift
    struct Storage: Codable, Equatable { var free: UInt64; var total: UInt64 }
}

enum DaemonCommand: Encodable {
    case subscribeDeviceEvents, getStatus, getConfig, forgetIpod, cancelSync
    case saveConfig(source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?)
    case triggerSync(source: Trigger)
    case decidePrompt(id: UInt64, choice: Int32)
    enum Trigger: String, Encodable { case manual, scheduled, plugIn = "plug_in" }
    // encode(to:) writes {"type": "...", ...fields} per the Global Constraints.
}

enum DaemonEvent: Decodable {
    case hello(protocolVersion: String, coreVersion: String)
    case statusUpdate(StatusInfo)
    case configUpdate(source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?)
    case deviceConnected(serial: String, modelLabel: String, drive: String, name: String?)
    case deviceDisconnected(serial: String)
    case syncRejected(reason: String)
    case syncEvent(line: String)
    case unknown            // forward-compat: log + ignore
    // init(from:) switches on the "type" string.
}

enum SyncEvent: Decodable {          // inner v1.0.0 line
    case header(source: String, ipod: String, manifest: String)
    case summary(add: Int, modify: Int, metadataOnly: Int, remove: Int, unchanged: Int, totalPlanned: Int)
    case trackStart(current: Int, total: Int, label: String)
    case trackDone
    case log(message: String)
    case prompt(id: UInt64, message: String, options: [String])
    case form(id: UInt64, label: String, initial: String?, hint: String?)
    case error(message: String, recoveryHints: [String]?)
    case finish(success: Bool)
    case other
    // init(from:) switches on "type"; unknowns → .other.
}
```

- [ ] **Step 4: Run tests to green**

Run: `cd ui/macos && swift test --filter WireCodecTests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift
git commit -m "feat(ui-macos): Codable wire models for daemon IPC (round-trip tested)"
```

---

## Task 3: DaemonClient actor (connect, handshake, stream, reconnect)

**Files:** Create `Sources/Classick/Ipc/DaemonClient.swift`.

**Interfaces:**
- Consumes: `DaemonCommand`, `DaemonEvent`.
- Produces: `actor DaemonClient` with `func events() -> AsyncStream<DaemonEvent>`, `func send(_ cmd: DaemonCommand) async`, `func start()` (connect + reconnect loop). Socket path: `NSTemporaryDirectory() + "classick.sock"`.

- [ ] **Step 1: Implement the actor (network I/O — verified by integration + hardware, not unit-tested)**

Use POSIX `socket(AF_UNIX)` + `connect`, or `FileHandle`. Algorithm:
1. `start()`: connect loop with backoff. On connect: read lines; the first must decode to `.hello` — validate `protocolVersion` major == "1"; else emit a fatal error and stop. Then send `subscribeDeviceEvents` + `getStatus`.
2. Read newline-delimited UTF-8; decode each into `DaemonEvent` (unknown `type` → `.unknown`, logged, skipped); yield onto the `AsyncStream`.
3. `send(_:)`: encode + append `\n` + write + flush.
4. On EOF/error: yield a synthetic disconnected marker, backoff, reconnect, re-subscribe.

```swift
import Foundation

actor DaemonClient {
    private var continuation: AsyncStream<DaemonEvent>.Continuation?
    private var handle: FileHandle?

    func events() -> AsyncStream<DaemonEvent> {
        AsyncStream { self.continuation = $0 }
    }
    var socketPath: String { NSTemporaryDirectory() + "classick.sock" }

    func start() async { /* connect+reconnect loop per algorithm above */ }
    func send(_ cmd: DaemonCommand) async { /* encode + "\n" + write */ }
}
```

- [ ] **Step 2: Integration test with a mock Unix socket**

`Tests/ClassickTests/DaemonClientTests.swift`: bind a temp `AF_UNIX` listener, accept one client, send a `hello` line + a `device_connected` line, then close. Assert the client yields `.hello` then `.deviceConnected` and reconnects after close.

Run: `cd ui/macos && swift test --filter DaemonClientTests`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/DaemonClient.swift ui/macos/Tests/ClassickTests/DaemonClientTests.swift
git commit -m "feat(ui-macos): DaemonClient actor over the Unix socket (mock-socket tested)"
```

---

## Task 4: AppModel reducer — TDD

**Files:** Create `Sources/Classick/Model/AppModel.swift`, `Sources/Classick/Model/Storage.swift`, `Tests/ClassickTests/AppModelReducerTests.swift`.

**Interfaces:**
- Consumes: `DaemonEvent`, `SyncEvent`, `storageFor(drive:)`.
- Produces: `@Observable @MainActor final class AppModel` with `func apply(_ ev: DaemonEvent)` and derived state: `device: DeviceState?`, `phase: Phase` (`.noDevice/.notConfigured/.idle/.syncing(current,total,label)/.error(String)`), `lastSync: HistoryEntry?`, `pendingPrompt: PendingPrompt?`, `storageText: String?`.

- [ ] **Step 1: Write failing reducer tests**

```swift
import XCTest
@testable import Classick

@MainActor
final class AppModelReducerTests: XCTestCase {
    func testDeviceConnectThenDisconnect() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "Michael’s iPod"))
        XCTAssertEqual(m.device?.name, "Michael’s iPod")
        m.apply(.deviceDisconnected(serial: "0xA"))
        XCTAssertNil(m.device)
        XCTAssertEqual(m.phase, .noDevice)
    }

    func testSyncProgressFromForwardedEvents() {
        let m = AppModel()
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        m.apply(.syncEvent(line: #"{"type":"track_start","current":34,"total":120,"label":"Karma Police"}"#))
        guard case let .syncing(cur, total, label) = m.phase else { return XCTFail() }
        XCTAssertEqual(cur, 34); XCTAssertEqual(total, 120); XCTAssertEqual(label, "Karma Police")
        m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#))
        XCTAssertEqual(m.phase, .idle)
    }

    func testPromptSurfaced() {
        let m = AppModel()
        m.apply(.syncEvent(line: #"{"type":"prompt","id":7,"message":"Source changed","options":["Apply","Cancel"]}"#))
        XCTAssertEqual(m.pendingPrompt?.id, 7)
        XCTAssertEqual(m.pendingPrompt?.options, ["Apply", "Cancel"])
    }

    func testRejectionBecomesError() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "x", drive: "/Volumes/IPOD", name: nil))
        m.apply(.syncRejected(reason: "not_configured"))
        if case .error = m.phase {} else { XCTFail("expected error phase") }
    }
}
```

- [ ] **Step 2: Implement `Storage.swift`**

```swift
import Foundation
func storageFor(drive: String) -> (free: Int64, total: Int64)? {
    let url = URL(fileURLWithPath: drive)
    guard let v = try? url.resourceValues(forKeys: [.volumeAvailableCapacityKey, .volumeTotalCapacityKey]),
          let free = v.volumeAvailableCapacity, let total = v.volumeTotalCapacity else { return nil }
    return (Int64(free), Int64(total))
}
```

- [ ] **Step 3: Implement `AppModel.apply`**

Reduce each event into state. `deviceConnected` → set `device` + compute `storageText` via `storageFor(drive:)`; `deviceDisconnected` → clear; `statusUpdate` → set `phase`/`lastSync`; `syncEvent` → decode inner `SyncEvent`, map `trackStart`→`.syncing`, `finish`→`.idle`, `prompt`/`form`→`pendingPrompt`, `error`→`.error`; `syncRejected` → `.error(humanReason)`. `phase` derivation: no device→`.noDevice`; device but `!configured`→`.notConfigured`; else `.idle`/`.syncing`.

- [ ] **Step 4: Run tests to green**

Run: `cd ui/macos && swift test --filter AppModelReducerTests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/ ui/macos/Tests/ClassickTests/AppModelReducerTests.swift
git commit -m "feat(ui-macos): AppModel reducer + app-side storage (TDD)"
```

---

## Task 5: DaemonProcess — spawn/own the daemon

**Files:** Create `Sources/Classick/Daemon/DaemonProcess.swift`.

**Interfaces:**
- Produces: `final class DaemonProcess` with `func ensureRunning()` (attach if the socket answers, else spawn `classick --daemon`) and `func stop()` (terminate on quit).

- [ ] **Step 1: Implement**

Locate the binary: `Bundle.main.url(forResource: "classick", withExtension: nil)` (bundled in `Contents/Resources`), else the dev path `../../target/release/classick` relative to the bundle. `ensureRunning()`: try `connect()` to the socket — if it succeeds, another daemon owns it (attach, don't spawn); else `Process` launch `classick --daemon` and retain it. `stop()`: `process.terminate()`.

```swift
import Foundation
final class DaemonProcess {
    private var proc: Process?
    func ensureRunning() { /* attach-if-socket-answers else spawn */ }
    func stop() { proc?.terminate() }
}
```

- [ ] **Step 2: Manual verify**

Wire it into `ClassickApp` (spawn on launch via a `@State` owner + `.onAppear`; stop on terminate). Run the bundle with **no** daemon already running; confirm `pgrep -f "classick --daemon"` appears, and disappears when the app quits.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Daemon/DaemonProcess.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): spawn + own the classick daemon (attach if already running)"
```

---

## Task 6: Menu content wired to AppModel

**Files:** Create `Sources/Classick/Views/MenuContent.swift`; update `ClassickApp.swift`.

**Interfaces:** Consumes `AppModel`; sends `DaemonCommand`s via a closure/`DaemonClient` handle.

- [ ] **Step 1: Implement the state-driven menu**

`MenuContent` renders per `model.phase`:
- `.noDevice`: `Text("No iPod connected")` (disabled).
- `.notConfigured`: `Button("Set Up Classick…")` → opens setup window.
- `.idle`: device name (`Text`), storage line (`model.storageText`), last-sync line, `Divider`, `Button("Sync Now") {…}` `.keyboardShortcut("s")`.
- `.syncing(cur,total,label)`: `Text("Syncing… \(cur) of \(total)")`, `Text(label)`, `Button("Cancel Sync") {…}`.
- `.error(msg)`: `Text(msg)` + `Button("Retry") {…}`.
Always: `Divider`, `Button("Settings…") {…}` `.keyboardShortcut(",")`, `Button("Quit Classick") { NSApp.terminate(nil) }` `.keyboardShortcut("q")`.
Wire the `MenuBarExtra` `systemImage` to a state-derived icon; drive the daemon event stream into `model.apply` via a `.task {}` on an owner view that iterates `daemonClient.events()`.

- [ ] **Step 2: Manual verify against the real daemon + iPod**

Start the real daemon (`target/release/classick --daemon`), plug in the iPod, run `Classick.app`. Expected: menu shows the device name + storage (e.g. "108 / 160 GB") + last sync; unplug → "No iPod connected"; replug → device returns. (This exercises DaemonClient + AppModel + storage end-to-end.)

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): state-driven menu bound to live daemon events"
```

---

## Task 7: First-run setup window

**Files:** Create `Sources/Classick/Views/SetupWindow.swift`; update `ClassickApp.swift` (add a `Window` scene, `openWindow`).

**Interfaces:** Consumes `AppModel` (detected device); sends `saveConfig`.

- [ ] **Step 1: Implement**

A single window: a `.fileImporter` button to pick the music folder (store the POSIX path), a row showing the detected iPod (`model.device?.name ?? "Plug in your iPod"`), a `Toggle("Sync automatically when plugged in")` (default on), a note "Quit Music.app before syncing", and a **Done** button that sends `saveConfig(source: pickedPath, daemon: DaemonSettings(enabled: autoSync, autostartWithWindows: false, firstSyncMode: "auto_apply", subsequentSyncMode: "auto_apply", scheduleMinutes: 0, notifyOn: "all"), ipod: model.device.map { IpodIdentity(serial: $0.serial, modelLabel: $0.model, name: $0.name) })` and closes. *(sync modes set to `auto_apply` so no `review` event is relayed — v1 has no review UI.)*

- [ ] **Step 2: Manual verify**

From the `.notConfigured` menu → "Set Up Classick…" → pick a folder → Done. Confirm the daemon persists it: `cat ~/Library/Application\ Support/classick/config.toml` shows the source + ipod serial, and the menu flips to `.idle`.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Views/SetupWindow.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): single-window first-run setup (folder + iPod + auto-sync)"
```

---

## Task 8: Settings window

**Files:** Create `Sources/Classick/Views/SettingsView.swift`; update `ClassickApp.swift` (add `Settings` scene).

**Interfaces:** Consumes `AppModel` (current config from `config_update`); sends `saveConfig` (debounced) + `forgetIpod`.

- [ ] **Step 1: Implement General + About**

`Settings { TabView { GeneralTab; AboutTab } }`. General: re-pick music folder, `Toggle("Sync automatically on plug-in")` (↔ `DaemonSettings.enabled`), schedule `Picker` (Off/every N hours ↔ `scheduleMinutes`), `Toggle("Launch at login")` (`SMAppService.mainApp.register()`/`.unregister()` AND `autostartWithWindows`), `Button("Remove this iPod")` → `forgetIpod`. Persist edits via a 400ms-debounced `saveConfig`. About: version, LGPL note, GitHub link.

- [ ] **Step 2: Manual verify**

Open Settings (⌘,), toggle auto-sync + change schedule; confirm `config.toml` updates (debounced). Toggle Launch-at-login; confirm `SMAppService.mainApp.status`.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Views/SettingsView.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): Settings window (General + About), debounced save_config"
```

---

## Task 9: Notifications

**Files:** Create `Sources/Classick/Notifications/Notifier.swift`; update AppModel to call it on `finish`.

**Interfaces:** `Notifier.requestAuth()`, `Notifier.syncFinished(success:Bool, added:Int)`.

- [ ] **Step 1: Implement**

`UNUserNotificationCenter`: request `.alert`/`.sound` auth on first launch. On the reducer seeing inner `finish{success}` (and honoring the notify-level setting), post "Sync complete — N added" or "Sync failed". `error` events → a failure notification.

- [ ] **Step 2: Manual verify**

Trigger a sync (Task 11 wiring) that adds ≥1 track; confirm a notification banner appears. (Requires the ad-hoc-signed bundle — see Risk 1; if it doesn't register, switch bundle.sh to a stable signing identity.)

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Notifications/Notifier.swift ui/macos/Sources/Classick/Model/AppModel.swift
git commit -m "feat(ui-macos): sync-complete notifications via UserNotifications"
```

---

## Task 10: Prompt alert

**Files:** Create `Sources/Classick/Views/PromptAlert.swift`; update `ClassickApp.swift`.

**Interfaces:** Consumes `AppModel.pendingPrompt`; sends `decidePrompt(id:choice:)`.

- [ ] **Step 1: Implement**

When `model.pendingPrompt != nil`, present a window/alert (activate the app so it's frontmost) with the message and a button per option; the tapped index sends `decidePrompt(id: prompt.id, choice: index)` and clears `pendingPrompt`. Handle `form` similarly with a `TextField` (choice replaced by `form_decision` if that path is needed — for v1, options-only prompts suffice; a `form` shows the text input and replies via the form path).

- [ ] **Step 2: Manual verify**

Force a prompt: configure source A, sync, then change the source in Settings and sync again → the daemon relays the source-change safeguard `prompt` → confirm the alert appears and answering it resumes/aborts the sync.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Views/PromptAlert.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): modal alert for daemon-relayed sync prompts"
```

---

## Task 11: Sync Now / Cancel + live progress

**Files:** Update `MenuContent.swift`, `AppModel.swift`.

- [ ] **Step 1: Wire actions**

**Sync Now** → `send(.triggerSync(source: .manual))`. **Cancel Sync** → `send(.cancelSync)`. Ensure `track_start`/`track_done` update the menu's syncing line live (verify the row re-renders from `@Observable` while the menu is open — Risk 2).

- [ ] **Step 2: Manual verify — real sync**

With a configured source containing an album NOT on the iPod (use the `ipod-albums` tool from SP1 to pick), click **Sync Now**. Expected: menu shows "Syncing… N of M · <track>" live, a completion notification fires, and the tracks land on the iPod. Then **Cancel** mid-sync on a larger album → sync stops.

- [ ] **Step 3: Commit**

```bash
git add ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/Model/AppModel.swift
git commit -m "feat(ui-macos): Sync Now / Cancel actions with live menu progress"
```

---

## Task 12: End-to-end hardware gate (manual)

**Files:** none (verification).

- [ ] **Step 1: Fresh-install flow**

Delete `~/Library/Application Support/classick/config.toml`, launch `Classick.app` (no daemon running). Expected: app spawns the daemon, menu shows "Set Up Classick…"; complete setup; menu flips to idle showing the plugged-in iPod + storage.

- [ ] **Step 2: Full daily-driver loop**

Plug/unplug the iPod (menu updates live) → **Sync Now** a new album (live progress + notification) → open Settings, change auto-sync + schedule (persists) → quit the app (daemon stops, no orphan `classick`).

- [ ] **Step 3: Record + commit**

Add a `LEARNINGS.md` bullet for any macOS-UI gotcha found. Commit.

**SP2 DONE when the fresh-install flow and the daily-driver loop both work on the real iPod.**

---

## Task 13: README

**Files:** Create `ui/macos/README.md`.

- [ ] **Step 1: Write it**

Mirror `ui/windows/README.md`: what the app is, build (`cargo build --release` then `ui/macos/bundle.sh`), run (`open ui/macos/Classick.app`), test (`cd ui/macos && swift test`), architecture (DaemonClient/AppModel/scenes), the macOS-15 floor + Liquid-Glass-on-26 note, and the not-sandboxed rationale.

- [ ] **Step 2: Commit**

```bash
git add ui/macos/README.md
git commit -m "docs(ui-macos): README (build/run/test/architecture)"
```

---

## Self-Review

**Spec coverage:** DaemonClient/AppModel/daemon-ownership → Tasks 3/4/5; native menu + states → Task 6; rich-panel future = documented, not a task (correct); first-run window → Task 7; Settings (General/About, history deferred) → Task 8; auto-sync default on → Task 7/8; notifications → Task 9; in-sync prompts → Task 10; Sync/Cancel + progress → Task 11; unit tests (codec + reducer) → Tasks 2/4; hardware gate → Task 12; README → Task 13. macOS-15 floor + not-sandboxed in Global Constraints. ✅

**Placeholder note:** network I/O (`DaemonClient`) and SwiftUI views are specified as interface + algorithm + real code skeletons with drive-and-observe verification, deliberately — UI/socket code is verified by the mock-socket test (Task 3) and the hardware gates (6/11/12), not unit tests. Every pure/testable unit (wire codec, reducer, storage) has complete TDD code. Wire field names are copied verbatim from the Rust source into Global Constraints.

**Type consistency:** `DaemonCommand`/`DaemonEvent`/`DaemonSettings`/`IpodIdentity`/`HistoryEntry`/`StatusInfo`/`SyncEvent` names and their snake_case wire keys are consistent across Tasks 2–11 and match the Global Constraints block.
