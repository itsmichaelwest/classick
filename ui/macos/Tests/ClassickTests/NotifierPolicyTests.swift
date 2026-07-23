import XCTest

@testable import Classick

/// The `notify_on` preference used to be ignored on macOS — a banner fired on
/// every sync regardless. These pin the policy that now gates `syncFinished`.
@MainActor
final class NotifierPolicyTests: XCTestCase {
  func testAllNotifiesForSuccessAndFailure() {
    XCTAssertTrue(
      Notifier.shouldPostSyncFinished(notifyOn: "all", success: true))
    XCTAssertTrue(
      Notifier.shouldPostSyncFinished(notifyOn: "all", success: false))
  }

  func testErrorsOnlyNotifiesOnlyOnFailure() {
    XCTAssertFalse(
      Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: true))
    XCTAssertTrue(
      Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: false))
  }

  func testNoneNeverNotifies() {
    XCTAssertFalse(
      Notifier.shouldPostSyncFinished(notifyOn: "none", success: true))
    XCTAssertFalse(
      Notifier.shouldPostSyncFinished(notifyOn: "none", success: false))
  }

  func testNilOrUnknownDefaultsToAll() {
    XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: nil, success: true))
    XCTAssertTrue(
      Notifier.shouldPostSyncFinished(notifyOn: "bogus", success: false))
  }

  func testEarlyFinishWaitsForAuthoritativeTerminalSnapshot() {
    var coordinator = SyncNotificationCoordinator()
    let model = AppModel()
    let active = snapshot(revision: 1, phase: .syncing, sessionID: 42)
    model.apply(.deviceInventorySnapshot(active))
    XCTAssertEqual(
      coordinator.consume(inventoryEvent(revision: 1), devices: model.devices), [])

    let summary = WireV3Event.syncEvent(
      line:
        #"{"type":"summary","add":3,"modify":0,"metadata_only":0,"remove":0,"unchanged":7,"total_planned":3}"#,
      serial: "A", sessionID: 42)
    model.apply(summary)
    XCTAssertEqual(coordinator.consume(progressEvent("sync_summary"), devices: model.devices), [])

    let finish = WireV3Event.syncEvent(
      line: #"{"type":"finish","success":true}"#, serial: "A", sessionID: 42)
    model.apply(finish)
    XCTAssertEqual(coordinator.consume(progressEvent("sync_finished"), devices: model.devices), [])
  }

  func testAuthoritativeTerminalSnapshotProducesOneCompletion() {
    var coordinator = SyncNotificationCoordinator()
    let model = AppModel()
    let active = snapshot(revision: 1, phase: .syncing, sessionID: 42)
    model.apply(.deviceInventorySnapshot(active))
    _ = coordinator.consume(inventoryEvent(revision: 1), devices: model.devices)
    let summary = WireV3Event.syncEvent(
      line:
        #"{"type":"summary","add":3,"modify":0,"metadata_only":0,"remove":0,"unchanged":7,"total_planned":3}"#,
      serial: "A", sessionID: 42)
    model.apply(summary)
    _ = coordinator.consume(progressEvent("sync_summary"), devices: model.devices)

    let success = history(sessionID: 42, outcome: "ok")
    let terminal = snapshot(
      revision: 2, phase: .idle, sessionID: nil,
      latestSuccessfulSync: success, latestAttempt: success)
    model.apply(.deviceInventorySnapshot(terminal))
    model.apply(.historyUpdate(entries: [success], acknowledgedRequestID: "terminal"))

    XCTAssertEqual(
      coordinator.consume(inventoryEvent(revision: 2), devices: model.devices),
      [
        SyncFinishedNotification(
          serial: "A", sessionID: 42, displayName: "A's iPod", success: true, added: 3)
      ])
    XCTAssertEqual(
      coordinator.consume(inventoryEvent(revision: 2), devices: model.devices), [],
      "duplicate terminal snapshots must not post twice")
  }

  func testCancellationNeverProducesCompletionNotification() {
    var coordinator = SyncNotificationCoordinator()
    let model = AppModel()
    let active = snapshot(revision: 1, phase: .syncing, sessionID: 42)
    model.apply(.deviceInventorySnapshot(active))
    _ = coordinator.consume(inventoryEvent(revision: 1), devices: model.devices)

    let cancelledEvent = WireV3Event.syncEvent(
      line: #"{"type":"cancelled"}"#, serial: "A", sessionID: 42)
    model.apply(cancelledEvent)
    _ = coordinator.consume(progressEvent("sync_cancelled"), devices: model.devices)
    let cancelled = history(sessionID: 42, outcome: "cancelled")
    let terminal = snapshot(
      revision: 2, phase: .idle, sessionID: nil,
      latestSuccessfulSync: nil, latestAttempt: cancelled)
    model.apply(.deviceInventorySnapshot(terminal))
    model.apply(.historyUpdate(entries: [cancelled], acknowledgedRequestID: "terminal"))

    XCTAssertEqual(
      coordinator.consume(inventoryEvent(revision: 2), devices: model.devices), [])
  }

  func testNotificationTitleUsesDeviceNameWithoutRawIdentifier() {
    let notification = SyncFinishedNotification(
      serial: "000000000000000A", sessionID: 42, displayName: "Michael's iPod",
      success: true, added: 3)

    let title = Notifier.title(for: notification)

    XCTAssertEqual(title, "Sync complete — Michael's iPod")
    XCTAssertFalse(title.contains(notification.serial.rawValue))
  }

  func testConcurrentSessionsProduceIndependentNamedNotifications() {
    var coordinator = SyncNotificationCoordinator()
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 1,
          devices: [
            deviceSnapshot(serial: "A", name: "Alpha iPod", phase: .syncing, sessionID: 42),
            deviceSnapshot(serial: "B", name: "Beta iPod", phase: .syncing, sessionID: 84),
          ])))
    _ = coordinator.consume(inventoryEvent(revision: 1), devices: model.devices)

    let alpha = HistoryEntry(
      serial: "000000000000000A", sessionID: 42, timestamp: "2026-07-19T12:00:00Z",
      durationSecs: 10, trigger: "manual", outcome: "ok")
    let beta = HistoryEntry(
      serial: "000000000000000B", sessionID: 84, timestamp: "2026-07-19T12:00:01Z",
      durationSecs: 11, trigger: "manual", outcome: "error")
    model.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 2,
          devices: [
            deviceSnapshot(
              serial: "A", name: "Alpha iPod", phase: .idle, sessionID: nil,
              latestSuccessfulSync: alpha, latestAttempt: alpha),
            deviceSnapshot(
              serial: "B", name: "Beta iPod", phase: .error, sessionID: nil,
              latestAttempt: beta, lastTerminalError: "Sync failed"),
          ])))
    model.apply(.historyUpdate(entries: [alpha, beta], acknowledgedRequestID: "terminal"))

    let notifications = coordinator.consume(inventoryEvent(revision: 2), devices: model.devices)

    XCTAssertEqual(notifications.map(\.displayName), ["Alpha iPod", "Beta iPod"])
    XCTAssertEqual(notifications.map(\.success), [true, false])
  }

  func testSeriallessScanStreamNeverProducesSyncNotification() {
    var coordinator = SyncNotificationCoordinator()
    let model = AppModel()
    model.apply(
      .statusUpdate(
        .init(
          state: .scanning, configured: true, ipodConnected: false,
          lastSync: nil, storage: nil)))
    let finish = WireV3Event.syncEvent(
      line: #"{"type":"finish","success":true}"#, serial: nil, sessionID: 99)
    model.apply(finish)

    XCTAssertEqual(coordinator.consume(progressEvent("sync_finished", sessionID: 99), devices: model.devices), [])
  }

  private func snapshot(
    revision: UInt64,
    phase: DevicePhaseLabel,
    sessionID: UInt64?,
    latestSuccessfulSync: HistoryEntry? = nil,
    latestAttempt: HistoryEntry? = nil
  ) -> DeviceInventorySnapshot {
    DeviceInventorySnapshot(
      revision: revision,
      devices: [
        deviceSnapshot(
          serial: "A", name: "A's iPod", phase: phase, sessionID: sessionID,
          latestSuccessfulSync: latestSuccessfulSync, latestAttempt: latestAttempt)
      ])
  }

  private func deviceSnapshot(
    serial: String, name: String, phase: DevicePhaseLabel, sessionID: UInt64?,
    latestSuccessfulSync: HistoryEntry? = nil, latestAttempt: HistoryEntry? = nil,
    lastTerminalError: String? = nil
  ) -> DeviceSnapshotWire {
    DeviceSnapshotWire(
      identity: .init(serial: serial, modelLabel: "iPod Classic", name: name),
      configured: true, connected: true, mount: "/Volumes/\(serial)", phase: phase,
      sessionID: sessionID, storage: nil, syncedCount: 10, libraryCount: 10,
      latestSuccessfulSync: latestSuccessfulSync, latestAttempt: latestAttempt,
      lastTerminalError: lastTerminalError, selectionRevision: 1, settingsRevision: 1,
      subscriptionsRevision: 1)
  }

  private func history(sessionID: UInt64, outcome: String) -> HistoryEntry {
    HistoryEntry(
      serial: "000000000000000A", sessionID: sessionID, timestamp: "2026-07-19T12:00:00Z",
      durationSecs: 10, trigger: "manual", outcome: outcome)
  }

  private func inventoryEvent(revision: UInt64) -> WireV3Event {
    decodeV3(
      #"{"type":"device_inventory","revision":\#(revision),"devices":[],"unidentified":[]}"#)
  }

  private func progressEvent(_ type: String, sessionID: UInt64 = 42) -> WireV3Event {
    let fields: String
    switch type {
    case "sync_summary":
      fields = #", "summary":{"add":3,"modify":0,"metadata_only":0,"remove":0,"unchanged":7,"total_planned":3}"#
    case "sync_finished": fields = #", "success":true"#
    default: fields = ""
    }
    return decodeV3(
      #"{"type":"\#(type)","device_id":"000000000000000A","session_id":\#(sessionID)\#(fields)}"#)
  }

  private func decodeV3(_ json: String) -> WireV3Event {
    guard
      case .event(let event) = try! WireV3Codec.decode(
        Data(json.utf8), direction: .daemonToDesktopEvents)
    else { preconditionFailure("expected event") }
    return event
  }
}
