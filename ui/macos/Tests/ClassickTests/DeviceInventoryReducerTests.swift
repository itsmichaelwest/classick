import XCTest

@testable import Classick

@MainActor
final class DeviceInventoryReducerTests: XCTestCase {
  func testInventorySnapshotKeepsTwoDevicesKeyedBySerial() {
    let model = AppModel()

    model.apply(
      .deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A"), device("B")])))

    XCTAssertEqual(Set(model.devices.keys), ["A", "B"])
    XCTAssertEqual(model.devices["A"]?.identity.name, "A's iPod")
    XCTAssertEqual(model.devices["B"]?.identity.name, "B's iPod")
  }

  func testDisconnectingOneDeviceDoesNotReplaceTheOther() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A"), device("B")])))

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [device("A", connected: false, mount: nil, phase: .disconnected), device("B")])))

    XCTAssertEqual(model.devices["A"]?.phase, .disconnected)
    XCTAssertFalse(model.devices["A"]?.connected ?? true)
    XCTAssertTrue(model.devices["B"]?.connected ?? false)
  }

  func testRememberedConfiguredDeviceCoexistsWithConnectedUnconfiguredDevice() {
    let model = AppModel()

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [
            device("A", configured: true, connected: false, mount: nil, phase: .disconnected),
            device("B", configured: false, phase: .unconfigured),
          ])))

    XCTAssertTrue(model.devices["A"]?.configured ?? false)
    XCTAssertFalse(model.devices["A"]?.connected ?? true)
    XCTAssertFalse(model.devices["B"]?.configured ?? true)
    XCTAssertTrue(model.devices["B"]?.connected ?? false)
    XCTAssertEqual(model.devices["B"]?.phase, .unconfigured)
  }

  func testProgressIsIsolatedToMatchingSerialAndSession() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [device("A", phase: .syncing, sessionID: 41), device("B")])))

    model.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":7,"total":20,"label":"Seven"}"#,
        serial: "A", sessionID: 41))

    XCTAssertEqual(model.devices["A"]?.phase, .syncing)
    XCTAssertEqual(model.devices["A"]?.syncProgress?.current, 7)
    XCTAssertEqual(model.devices["A"]?.syncProgress?.total, 20)
    XCTAssertEqual(model.devices["A"]?.syncProgress?.label, "Seven")
    XCTAssertNil(model.devices["B"]?.syncProgress)
    XCTAssertEqual(model.devices["B"]?.phase, .idle)
  }

  func testStaleSessionProgressIsIgnored() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))

    model.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":99,"total":100,"label":"Stale"}"#,
        serial: "A", sessionID: 41))

    XCTAssertNil(model.devices["A"]?.syncProgress)
    XCTAssertEqual(model.devices["A"]?.sessionID, 42)
  }

  func testFinishStoresRollupsWithoutMakingTheDeviceTerminal() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))

    model.apply(
      .syncEvent(
        line:
          #"{"type":"finish","success":true,"skipped_for_space":{"albums":1,"tracks":2,"bytes":3},"artwork":{"embedded":4,"eligible":5,"failed_sources":1},"db_restored":true}"#,
        serial: "A", sessionID: 42))

    XCTAssertEqual(model.devices["A"]?.phase, .syncing)
    XCTAssertEqual(model.devices["A"]?.sessionID, 42)
    XCTAssertEqual(model.devices["A"]?.lastRun?.skippedForSpace?.tracks, 2)
    XCTAssertEqual(model.devices["A"]?.lastRun?.artwork?.failedSources, 1)
    XCTAssertEqual(model.devices["A"]?.lastRun?.dbRestored, true)
  }

  func testLaterSnapshotAppliesTerminalStateAtomically() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(
      .syncEvent(
        line: #"{"type":"finish","success":true}"#, serial: "A", sessionID: 42))
    let successful = history(serial: "A", sessionID: 42, outcome: "completed")

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [
            device(
              "A", phase: .idle, sessionID: nil, syncedCount: 20, libraryCount: 20,
              latestSuccessfulSync: successful, latestAttempt: successful)
          ])))

    let state = model.devices["A"]
    XCTAssertEqual(state?.phase, .idle)
    XCTAssertNil(state?.sessionID)
    XCTAssertEqual(state?.syncedCount, 20)
    XCTAssertEqual(state?.latestSuccessfulSync, successful)
    XCTAssertEqual(state?.latestAttempt, successful)
  }

  func testFocusPriorityIsActiveSessionThenSelectionThenSoleConnected() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A"), device("B", phase: .syncing, sessionID: 7)])))
    model.selectedDestination = .device(serial: "A", page: .music)
    XCTAssertEqual(model.focusedDeviceSerial, "B")

    model.apply(
      .deviceInventorySnapshot(snapshot(revision: 2, devices: [device("A"), device("B")])))
    XCTAssertEqual(model.focusedDeviceSerial, "A")

    model.selectedDestination = .library
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 3,
          devices: [device("A"), device("B", connected: false, mount: nil, phase: .disconnected)])))
    XCTAssertEqual(model.focusedDeviceSerial, "A")
  }

  func testFocusDoesNotGuessWhenMultipleDevicesAreConnected() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A"), device("B")])))

    XCTAssertNil(model.focusedDeviceSerial)
  }

  func testOlderOrDuplicateInventoryRevisionCannotRollBackSessionState() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 8, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":7,"total":20,"label":"Current"}"#,
        serial: "A", sessionID: 42))

    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 8, devices: [device("A", phase: .syncing, sessionID: 41)])))
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 7, devices: [device("A", phase: .syncing, sessionID: 41)])))
    model.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":19,"total":20,"label":"Stale"}"#,
        serial: "A", sessionID: 41))

    XCTAssertEqual(model.devices["A"]?.sessionID, 42)
    XCTAssertEqual(model.devices["A"]?.syncProgress?.current, 7)
    XCTAssertEqual(model.devices["A"]?.syncProgress?.label, "Current")
  }

  func testHelloStartsNewInventoryRevisionEpoch() {
    let model = AppModel()
    model.apply(.deviceInventorySnapshot(snapshot(revision: 8, devices: [device("A")])))

    model.apply(.hello(protocolVersion: "2.0.0", coreVersion: "2.0.0"))
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [device("A", connected: false, mount: nil, phase: .disconnected)])))

    XCTAssertEqual(model.devices["A"]?.phase, .disconnected)
    XCTAssertFalse(model.devices["A"]?.connected ?? true)
  }

  func testReconnectDropsDeviceSyncEventsUntilFreshInventoryArrives() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 8, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(.hello(protocolVersion: "2.0.0", coreVersion: "2.0.0"))

    model.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":9,"total":20,"label":"Too early"}"#,
        serial: "A", sessionID: 42))

    XCTAssertNil(model.devices["A"]?.syncProgress)
  }

  private func snapshot(revision: UInt64, devices: [DeviceSnapshotWire]) -> DeviceInventorySnapshot
  {
    DeviceInventorySnapshot(revision: revision, devices: devices)
  }

  private func device(
    _ serial: String,
    configured: Bool = true,
    connected: Bool = true,
    mount: String? = nil,
    phase: DevicePhaseLabel = .idle,
    sessionID: UInt64? = nil,
    syncedCount: Int = 0,
    libraryCount: Int? = 20,
    latestSuccessfulSync: HistoryEntry? = nil,
    latestAttempt: HistoryEntry? = nil
  ) -> DeviceSnapshotWire {
    DeviceSnapshotWire(
      identity: DeviceIdentityWire(
        serial: serial, modelLabel: "iPod Classic", name: "\(serial)'s iPod"),
      configured: configured,
      connected: connected,
      mount: mount ?? (connected ? "/Volumes/\(serial)" : nil),
      phase: phase,
      sessionID: sessionID,
      storage: connected ? .init(free: 75, total: 100) : nil,
      syncedCount: syncedCount,
      libraryCount: libraryCount,
      latestSuccessfulSync: latestSuccessfulSync,
      latestAttempt: latestAttempt,
      lastTerminalError: nil,
      selectionRevision: 1,
      settingsRevision: 2,
      subscriptionsRevision: 3)
  }

  private func history(serial: String, sessionID: UInt64, outcome: String) -> HistoryEntry {
    HistoryEntry(
      serial: serial,
      sessionID: sessionID,
      timestamp: "2026-07-18T12:00:00Z",
      durationSecs: 10,
      trigger: "manual",
      outcome: outcome)
  }
}
