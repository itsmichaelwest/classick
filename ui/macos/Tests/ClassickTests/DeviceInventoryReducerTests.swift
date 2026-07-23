import XCTest

@testable import Classick

@MainActor
final class DeviceInventoryReducerTests: XCTestCase {
  func testProtocol3InventoryUsesCanonicalIdentityAndSeparatesObservations() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")

    model.apply(try protocol3Event(lines[0]))

    let deviceID = DeviceID("000A27002138B0A8")
    XCTAssertEqual(Set(model.devices.keys), [deviceID])
    XCTAssertEqual(model.devices[deviceID]?.mountPath, "/Volumes/Michael West's iPod")
    XCTAssertEqual(model.devices[deviceID]?.hardware.family?.value, "classic")
    XCTAssertEqual(model.unidentifiedDevices.count, 1)
    XCTAssertEqual(model.unidentifiedDevices.values.first?.readiness, "identity_unavailable")
  }

  func testProtocol3ReconnectPreservesConfigWhileReplacingConnectionAttributes() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")

    model.apply(try protocol3Event(lines[0]))
    model.apply(try protocol3Event(lines[4]))
    model.apply(try protocol3Event(lines[1]))

    XCTAssertFalse(model.devices[deviceID]?.connected ?? true)
    XCTAssertNil(model.devices[deviceID]?.mountPath)
    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)
    XCTAssertEqual(
      model.devices[deviceID]?.configDelivery?.settings.delivery.state, "device_committed")
    XCTAssertTrue(model.unidentifiedDevices.isEmpty)
  }

  func testProtocol3ConfigRequiresExactCorrelatedRequest() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let expectedRequestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"

    model.apply(try protocol3Event(lines[0]))
    model.willRequestDeviceConfig(
      serial: deviceID,
      requestID: expectedRequestID,
      intent: .write)

    let wrong = String(decoding: lines[3], as: UTF8.self).replacingOccurrences(
      of: expectedRequestID, with: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f9999")
    model.apply(try protocol3Event(Data(wrong.utf8)))
    XCTAssertNil(model.devices[deviceID]?.config)

    model.apply(try protocol3Event(lines[3]))
    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)
    XCTAssertEqual(
      model.deviceConfigAcknowledgedRequestIDs[deviceID],
      expectedRequestID)
  }

  func testDisconnectedAcceptedConfigReportsWaitingThenCommitted() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let requestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"
    model.apply(try protocol3Event(lines[0]))
    model.willRequestDeviceConfig(serial: deviceID, requestID: requestID, intent: .write)
    model.editDeviceSettings(.init(autoSync: false, rockboxCompat: false), for: deviceID)
    model.markDeviceSettingsSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: requestID,
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8771"))
    model.apply(try protocol3Event(lines[3]))

    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .settings), .waitingForDevice)
    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)

    model.apply(try protocol3Event(lines[4]))

    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .settings), .saved)
    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)
  }

  func testStaleBroadcastCannotOverwriteDisconnectedLocalDraft() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let requestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"
    model.apply(try protocol3Event(lines[0]))
    model.willRequestDeviceConfig(serial: deviceID, requestID: requestID, intent: .write)
    model.apply(try protocol3Event(lines[3]))
    model.apply(try protocol3Event(lines[1]))
    model.editDeviceSelection(
      .init(mode: .include, rules: [.artist(name: "Local")]), for: deviceID)

    model.apply(try protocol3Event(lines[4]))

    XCTAssertEqual(model.editableDeviceConfig(for: deviceID)?.selection.mode, .include)
    XCTAssertEqual(
      model.editableDeviceConfig(for: deviceID)?.selection.rules, [.artist(name: "Local")])
    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .selection), .localDraft)
  }

  func testHostFailureRetainsExactDraftWithoutTurningSyncPhaseIntoError() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let configRequestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"
    model.apply(try protocol3Event(lines[0]))
    model.willRequestDeviceConfig(serial: deviceID, requestID: configRequestID, intent: .write)
    model.apply(try protocol3Event(lines[3]))
    model.editDeviceSettings(.init(autoSync: true, rockboxCompat: false), for: deviceID)
    model.markDeviceSettingsSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8767",
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8774"))

    model.apply(try protocol3Event(lines[5]))

    XCTAssertEqual(model.editableDeviceConfig(for: deviceID)?.settings.autoSync, true)
    XCTAssertEqual(
      model.deviceConfigStatus(for: deviceID, component: .settings),
      .hostAcceptanceFailed("Could not save the setting"))
    XCTAssertEqual(model.devices[deviceID]?.phase, .idle)
  }

  func testDeviceDeliveryFailureRetainsAcceptedHostValue() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let requestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"
    model.apply(try protocol3Event(lines[0]))
    model.willRequestDeviceConfig(serial: deviceID, requestID: requestID, intent: .write)
    model.editDeviceSettings(.init(autoSync: false, rockboxCompat: false), for: deviceID)
    model.markDeviceSettingsSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: requestID,
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8771"))
    model.apply(try protocol3Event(lines[3]))
    let unrelatedFailure = String(decoding: lines[6], as: UTF8.self)
      .replacingOccurrences(of: "subscriptions", with: "settings")
      .replacingOccurrences(of: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8775", with: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8771")

    model.apply(try protocol3Event(Data(unrelatedFailure.utf8)))
    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .settings), .waitingForDevice)

    let correlatedFailure = unrelatedFailure.replacingOccurrences(
      of: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8768", with: requestID)
    model.apply(try protocol3Event(Data(correlatedFailure.utf8)))

    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)
    XCTAssertEqual(
      model.deviceConfigStatus(for: deviceID, component: .settings),
      .deviceDeliveryFailed("iPod is disconnected"))
  }

  func testIndependentComponentWriteRepliesClearOnlyExactMutation() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    model.apply(try protocol3Event(lines[0]))
    model.apply(try protocol3Event(lines[4]))
    model.editDeviceSelection(.init(mode: .include, rules: []), for: deviceID)
    model.editDeviceSubscriptions(.init(playlists: ["x"]), for: deviceID)
    let selectionRequest = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8801"
    let subscriptionsRequest = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8802"
    model.willRequestDeviceConfig(serial: deviceID, requestID: selectionRequest, intent: .write)
    model.willRequestDeviceConfig(serial: deviceID, requestID: subscriptionsRequest, intent: .write)
    model.markDeviceSelectionSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: selectionRequest,
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8770"))
    model.markDeviceSubscriptionsSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: subscriptionsRequest,
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8772"))
    let base = String(decoding: lines[3], as: UTF8.self)
      .replacingOccurrences(of: "\"mode\":\"all\"", with: "\"mode\":\"include\"")
      .replacingOccurrences(of: "\"playlists\":[]", with: "\"playlists\":[\"x\"]")
    let selectionReply = base.replacingOccurrences(
      of: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765", with: selectionRequest)
    let subscriptionsReply = base.replacingOccurrences(
      of: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765", with: subscriptionsRequest)

    model.apply(try protocol3Event(Data(selectionReply.utf8)))
    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .selection), .saved)
    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .subscriptions), .savingOnHost)

    model.apply(try protocol3Event(Data(subscriptionsReply.utf8)))
    XCTAssertEqual(
      model.deviceConfigStatus(for: deviceID, component: .subscriptions),
      .deviceDeliveryFailed("iPod is disconnected"))
  }

  func testTwoDeviceDraftsRemainIndependent() {
    let model = AppModel()
    model.apply(.deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A"), device("B")])))
    for serial: DeviceID in ["A", "B"] {
      model.willRequestDeviceConfig(serial: serial, requestID: serial.rawValue, intent: .read)
      model.apply(
        .deviceConfigUpdate(
          serial: serial.rawValue, selection: .init(mode: .all, rules: []),
          subscriptions: .init(playlists: []),
          settings: .init(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 2, subscriptionsRevision: 3,
          acknowledgedRequestID: serial.rawValue))
    }

    model.editDeviceSettings(.init(autoSync: false, rockboxCompat: false), for: "A")

    XCTAssertEqual(model.editableDeviceConfig(for: "A")?.settings.autoSync, false)
    XCTAssertEqual(model.editableDeviceConfig(for: "B")?.settings.autoSync, true)
  }

  func testDisconnectedDraftSurvivesNavigationAndReconnect() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    model.apply(try protocol3Event(lines[0]))
    model.apply(try protocol3Event(lines[4]))
    model.apply(try protocol3Event(lines[1]))
    model.selectedDestination = .device(serial: deviceID, page: .music)
    model.editDeviceSettings(.init(autoSync: false, rockboxCompat: true), for: deviceID)
    model.markDeviceSettingsSubmitted(
      for: deviceID,
      receipt: .init(
        requestID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8890",
        mutationID: "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8891"))

    model.selectedDestination = .library
    model.selectedDestination = .device(serial: deviceID, page: .settings)
    model.apply(
      .hello(
        WireV3Hello(
          protocolVersion: "3.0.0", role: .daemon,
          softwareVersion: "test", capabilities: WireV3Codec.daemonCapabilities)))
    model.apply(try protocol3Event(lines[0]))

    XCTAssertEqual(model.editableDeviceConfig(for: deviceID)?.settings.autoSync, false)
    XCTAssertEqual(model.editableDeviceConfig(for: deviceID)?.settings.rockboxCompat, true)
    XCTAssertEqual(model.deviceConfigStatus(for: deviceID, component: .settings), .localDraft)
    XCTAssertNotNil(model.pendingDeviceSettings(for: deviceID))
  }

  func testEditableConfigPreservesNonEditableDevicePreview() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    model.apply(try protocol3Event(lines[0]))
    model.apply(try protocol3Event(lines[4]))
    let previewRequestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8990"
    model.willRequestDevicePreview(serial: deviceID, requestID: previewRequestID)
    model.apply(
      .devicePreview(
        .init(
          serial: deviceID.rawValue, selectedTracks: 10, selectedBytes: 20,
          playlistExtraTracks: 1, playlistExtraBytes: 2, projectedFreeBytes: 3,
          unresolvedSubscriptions: ["missing"], acknowledgedRequestID: previewRequestID)))
    model.editDeviceSelection(.init(mode: .include, rules: []), for: deviceID)

    XCTAssertEqual(model.editableDeviceConfig(for: deviceID)?.preview?.selectedTracks, 10)
    XCTAssertEqual(
      model.editableDeviceConfig(for: deviceID)?.preview?.unresolvedSubscriptions, ["missing"])
  }

  func testProtocol3ProgressRoutesByExactDeviceAndSession() throws {
    let model = AppModel()
    model.apply(
      try protocol3Event(
        Data(
          #"{"type":"device_inventory","revision":1,"devices":[{"device_id":"000A27002138B0A8","readiness":"ready","hardware":{},"profile_status":"adopted","connected":true,"mount_path":"/Volumes/A","phase":"syncing","session_id":42,"synced_count":0},{"device_id":"000A27002138B0A9","readiness":"ready","hardware":{},"profile_status":"adopted","connected":true,"mount_path":"/Volumes/B","phase":"syncing","session_id":84,"synced_count":0}],"unidentified":[]}"#
            .utf8)))

    model.apply(
      try protocol3Event(
        Data(
          #"{"type":"track_start","device_id":"000A27002138B0A8","session_id":42,"current":3,"total":10,"label":"A track"}"#
            .utf8)))
    model.apply(
      try protocol3Event(
        Data(
          #"{"type":"track_start","device_id":"000A27002138B0A9","session_id":83,"current":9,"total":10,"label":"Stale B"}"#
            .utf8)))

    XCTAssertEqual(model.devices["000A27002138B0A8"]?.syncProgress?.current, 3)
    XCTAssertNil(model.devices["000A27002138B0A9"]?.syncProgress)
  }

  func testUnknownProtocol3PhasePreservesKnownDevicePhase() throws {
    let model = AppModel()
    let lines = try protocol3FixtureLines("device/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    model.apply(try protocol3Event(lines[0]))

    let future = String(decoding: lines[0], as: UTF8.self)
      .replacingOccurrences(of: "\"revision\":1", with: "\"revision\":2")
      .replacingOccurrences(of: "\"phase\":\"idle\"", with: "\"phase\":\"verifying\"")
    model.apply(try protocol3Event(Data(future.utf8)))

    XCTAssertEqual(model.devices[deviceID]?.phase, .idle)
  }

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

  func testFinalizingEventImmediatelyMarksTheMatchingDevice() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))

    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#,
        serial: "A", sessionID: 42))

    XCTAssertEqual(
      model.devices["A"]?.finalization,
      DeviceFinalization(
        reason: .cancelled, stagedAlbums: 2, stagedTracks: 17))
    XCTAssertFalse(model.canControlSync(to: "A"))
  }

  func testRawFinishAndCancelledDoNotOwnTheTerminalTransition() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#,
        serial: "A", sessionID: 42))

    model.apply(
      .syncEvent(
        line: #"{"type":"cancelled"}"#, serial: "A", sessionID: 42))
    model.apply(
      .syncEvent(
        line: #"{"type":"finish","success":true}"#, serial: "A", sessionID: 42))

    XCTAssertEqual(model.devices["A"]?.phase, .syncing)
    XCTAssertEqual(model.devices["A"]?.sessionID, 42)
    XCTAssertEqual(model.devices["A"]?.finalization?.reason, .cancelled)
  }

  func testCancelledTerminalSnapshotPreservesLatestSuccessfulTimestamp() {
    let model = AppModel()
    let successful = history(
      serial: "A", sessionID: 40, outcome: "ok", timestamp: "2026-07-18T10:00:00Z")
    let cancelled = history(
      serial: "A", sessionID: 42, outcome: "cancelled", timestamp: "2026-07-19T10:00:00Z")
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [
            device(
              "A", phase: .syncing, sessionID: 42, latestSuccessfulSync: successful,
              latestAttempt: successful)
          ])))
    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":1,"staged_tracks":3}"#,
        serial: "A", sessionID: 42))

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [
            device(
              "A", phase: .idle, sessionID: nil, latestSuccessfulSync: successful,
              latestAttempt: cancelled)
          ])))
    model.apply(
      .historyUpdate(entries: [successful, cancelled], acknowledgedRequestID: "terminal"))

    XCTAssertEqual(model.devices["A"]?.latestSuccessfulSync?.timestamp, successful.timestamp)
    XCTAssertEqual(model.devices["A"]?.latestAttempt, cancelled)
    XCTAssertNil(model.devices["A"]?.finalization)
  }

  func testInterruptedFinalizationTransitionsOnlyWithAuthoritativeErrorSnapshot() {
    let model = AppModel()
    let successful = history(
      serial: "A", sessionID: 40, outcome: "ok", timestamp: "2026-07-18T10:00:00Z")
    let interrupted = history(
      serial: "A", sessionID: 42, outcome: "aborted", timestamp: "2026-07-19T10:00:00Z")
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [
            device(
              "A", phase: .syncing, sessionID: 42, latestSuccessfulSync: successful,
              latestAttempt: successful)
          ])))
    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":1,"staged_tracks":3}"#,
        serial: "A", sessionID: 42))

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [
            device(
              "A", phase: .error, sessionID: nil, latestSuccessfulSync: successful,
              latestAttempt: interrupted, lastTerminalError: "finalization_stalled")
          ])))
    model.apply(
      .historyUpdate(entries: [successful, interrupted], acknowledgedRequestID: "terminal"))

    XCTAssertEqual(model.devices["A"]?.phase, .error("finalization_stalled"))
    XCTAssertEqual(model.devices["A"]?.latestSuccessfulSync, successful)
    XCTAssertEqual(model.devices["A"]?.latestAttempt, interrupted)
    XCTAssertNil(model.devices["A"]?.finalization)
  }

  func testFinalizingEventIsIsolatedBySerialAndSession() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [
            device("A", phase: .syncing, sessionID: 42),
            device("B", phase: .syncing, sessionID: 84),
          ])))

    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#,
        serial: "A", sessionID: 42))
    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":9,"staged_tracks":99}"#,
        serial: "B", sessionID: 83))

    XCTAssertEqual(model.devices["A"]?.finalization?.stagedTracks, 17)
    XCTAssertNil(model.devices["B"]?.finalization)
    XCTAssertTrue(model.canControlSync(to: "B"))
  }

  func testDisconnectedFinalizingSessionRemainsFocusedAndNonInteractive() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(
      .syncEvent(
        line:
          #"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#,
        serial: "A", sessionID: 42))

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [
            device(
              "A", connected: false, mount: nil, phase: .disconnected, sessionID: 42)
          ])))

    XCTAssertEqual(model.focusedDeviceSerial, "A")
    XCTAssertEqual(model.devices["A"]?.finalization?.stagedTracks, 17)
    XCTAssertFalse(model.canControlSync(to: "A"))
  }

  func testDisconnectedConfiguredDeviceAllowsSettingsButNotSyncControl() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 1,
          devices: [
            device("A", connected: false, mount: nil, phase: .disconnected)
          ])))

    XCTAssertTrue(model.canSendDeviceCommand(to: "A"))
    XCTAssertFalse(model.canControlSync(to: "A"))
  }

  func testLaterSnapshotAppliesTerminalStateAtomically() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A", phase: .syncing, sessionID: 42)])))
    model.apply(
      .syncEvent(
        line: #"{"type":"finish","success":true}"#, serial: "A", sessionID: 42))
    let successful = history(serial: "A", sessionID: 42, outcome: "ok")

    model.apply(
      .deviceInventorySnapshot(
        snapshot(
          revision: 2,
          devices: [
            device(
              "A", phase: .idle, sessionID: nil, syncedCount: 20, libraryCount: 20,
              latestSuccessfulSync: successful, latestAttempt: successful)
          ])))
    model.apply(.historyUpdate(entries: [successful], acknowledgedRequestID: "terminal"))

    let state = model.devices["A"]
    XCTAssertEqual(state?.phase, .idle)
    XCTAssertNil(state?.sessionID)
    XCTAssertEqual(state?.syncedCount, 20)
    XCTAssertEqual(state?.latestSuccessfulSync, successful)
    XCTAssertEqual(state?.latestAttempt, successful)
  }

  func testChronologicalHistoryUsesTheNewestAttemptAndSuccessfulRun() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 1, devices: [device("A")])))
    let olderSuccess = history(
      serial: "A", sessionID: 40, outcome: "ok", timestamp: "2026-07-18T10:00:00Z")
    let newerFailure = history(
      serial: "A", sessionID: 41, outcome: "error", timestamp: "2026-07-19T10:00:00Z")
    let newestSuccess = history(
      serial: "A", sessionID: 42, outcome: "ok", timestamp: "2026-07-20T10:00:00Z")

    model.apply(
      .historyUpdate(
        entries: [olderSuccess, newerFailure, newestSuccess],
        acknowledgedRequestID: "broadcast"))

    XCTAssertEqual(model.devices["A"]?.latestAttempt, newestSuccess)
    XCTAssertEqual(model.devices["A"]?.latestSuccessfulSync, newestSuccess)
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

  func testPromptAnswerRetainsOriginalDeviceAndSessionAfterNavigation() throws {
    let model = AppModel()
    model.apply(
      try protocol3Event(
        Data(
          #"{"type":"device_inventory","revision":1,"devices":[{"device_id":"000A27002138B0A8","readiness":"ready","hardware":{},"profile_status":"adopted","connected":true,"mount_path":"/Volumes/A","phase":"syncing","session_id":42,"synced_count":0},{"device_id":"000A27002138B0A9","readiness":"ready","hardware":{},"profile_status":"adopted","connected":true,"mount_path":"/Volumes/B","phase":"idle","synced_count":0}],"unidentified":[]}"#
            .utf8)))
    model.apply(
      try protocol3Event(
        Data(
          #"{"type":"prompt","device_id":"000A27002138B0A8","session_id":42,"prompt_id":7,"message":"Choose","options":["Continue","Abort"]}"#
            .utf8)))
    model.selectedDestination = .device(serial: "000A27002138B0A9", page: .music)

    let prompt = try XCTUnwrap(model.pendingPrompt)
    let command = PromptAlert.command(for: prompt, response: .choice(1), requestID: UUID())

    guard case .promptDecision(let route, _, let promptID, let choice) = command else {
      return XCTFail("expected prompt decision")
    }
    XCTAssertEqual(route, WireV3Route(deviceID: "000A27002138B0A8", sessionID: 42))
    XCTAssertEqual(promptID, 7)
    XCTAssertEqual(choice, 1)
  }

  func testMountActionIsFencedByInventoryRevisionAndCurrentMount() {
    let model = AppModel()
    model.apply(.deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A")])))
    let first = model.captureMountAction(for: "A")
    XCTAssertNotNil(first)
    XCTAssertTrue(first.map(model.isCurrentMountAction) ?? false)

    model.apply(
      .deviceInventorySnapshot(
        snapshot(revision: 2, devices: [device("A", connected: true, mount: "/Volumes/New")])) )

    XCTAssertFalse(first.map(model.isCurrentMountAction) ?? true)
    XCTAssertEqual(model.captureMountAction(for: "A")?.mountPath, "/Volumes/New")
  }

  func testSuccessfulMountActionImmediatelyMarksTheDeviceDisconnected() throws {
    let model = AppModel()
    model.apply(.deviceInventorySnapshot(snapshot(revision: 1, devices: [device("A")])))
    let action = try XCTUnwrap(model.captureMountAction(for: "A"))

    XCTAssertTrue(model.completeMountAction(action))

    XCTAssertFalse(model.devices["A"]?.connected ?? true)
    XCTAssertNil(model.devices["A"]?.mountPath)
    XCTAssertEqual(model.devices["A"]?.phase, .disconnected)
    XCTAssertNil(model.captureMountAction(for: "A"))
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

  private func protocol3Event(_ data: Data) throws -> WireV3Event {
    guard
      case .event(let event) = try WireV3Codec.decode(
        data, direction: .daemonToDesktopEvents)
    else { throw Protocol3FixtureError.notEvent }
    return event
  }

  private func protocol3FixtureLines(_ path: String) throws -> [Data] {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .appendingPathComponent("../../../../crates/classick/tests/data/wire-v3")
      .standardizedFileURL
    return try String(decoding: Data(contentsOf: root.appendingPathComponent(path)), as: UTF8.self)
      .split(whereSeparator: \.isNewline)
      .map { Data($0.utf8) }
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
    latestAttempt: HistoryEntry? = nil,
    lastTerminalError: String? = nil
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
      lastTerminalError: lastTerminalError,
      selectionRevision: 1,
      settingsRevision: 2,
      subscriptionsRevision: 3)
  }

  private func history(
    serial: String, sessionID: UInt64, outcome: String,
    timestamp: String = "2026-07-18T12:00:00Z"
  ) -> HistoryEntry {
    HistoryEntry(
      serial: canonicalFixtureDeviceID(serial).rawValue,
      sessionID: sessionID,
      timestamp: timestamp,
      durationSecs: 10,
      trigger: "manual",
      outcome: outcome)
  }

  private func canonicalFixtureDeviceID(_ value: String) -> DeviceID {
    if let canonical = try? DeviceID(value) { return canonical }
    return try! DeviceID(String(repeating: "0", count: 16 - value.count) + value.uppercased())
  }
}

private enum Protocol3FixtureError: Error { case notEvent }
