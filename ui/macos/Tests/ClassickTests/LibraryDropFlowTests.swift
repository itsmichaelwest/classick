import XCTest

@testable import Classick

@MainActor
final class LibraryDropFlowTests: XCTestCase {
  func testDropFlowShowsAddingThenOneAuthoritativeOutcome() {
    let model = AppModel()
    seedDevice("A", in: model)
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000101")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "Michael's iPod")
    var announcements: [String] = []
    let announcer = LibraryDropAnnouncementCoordinator { announcements.append($0) }

    model.markLibraryDropAdding(requestID: requestID, target: target)
    XCTAssertTrue(model.isLibraryDropAdding(target: target))
    XCTAssertEqual(DropOutcome.adding(target: target).accessibleMessage, "Adding…")

    let outcome = DeviceSelectionAddedInfo(
      acknowledgedRequestID: requestID.uuidString.lowercased(), serial: "A",
      matchedTracks: 1, missingTracks: 0, selectionChanged: true, selectionRevision: 7,
      selection: .init(mode: .include, rules: [.artist(name: "Birdy")]),
      delivery: .addedAndSyncing)
    model.apply(.deviceSelectionAdded(outcome))
    model.apply(.deviceSelectionAdded(outcome))

    XCTAssertFalse(model.isLibraryDropAdding(target: target))
    XCTAssertEqual(model.dropOutcome, .addedAndSyncing(serial: "A"))
    XCTAssertEqual(model.persistedDropAcknowledgements, [requestID.uuidString.lowercased()])
    announcer.announce(
      requestID: requestID.uuidString.lowercased(), outcome: model.dropOutcome!)
    announcer.announce(
      requestID: requestID.uuidString.lowercased(), outcome: model.dropOutcome!)
    XCTAssertEqual(announcements, ["Added and syncing"])
  }

  func testCrossTargetDropsAreSubmittedInFIFOOrder() async {
    let sender = FlowCommandRecorder()
    let coordinator = LibraryDropSubmissionCoordinator(send: { command in
      await sender.send(command)
    })

    coordinator.submit(
      target: .device(serial: "A", displayName: "A"), rules: [.artist(name: "Birdy")],
      requestID: UUID())
    coordinator.submit(
      target: .manualPlaylist(slug: "favorites", displayName: "Favorites"),
      rules: [.genre(name: "Pop")], requestID: UUID())
    coordinator.submit(
      target: .device(serial: "B", displayName: "B"),
      rules: [.album(artist: "Birdy", album: "Fire Within")], requestID: UUID())

    await sender.waitForCount(3)
    let targets = await sender.targets
    XCTAssertEqual(targets, ["device:A", "playlist:favorites", "device:B"])
  }

  func testDroppedSendProducesLocalRejectionWithoutPersistedAcknowledgement() async {
    let model = AppModel()
    let sender = FlowCommandRecorder(dispositions: [.dropped])
    let coordinator = LibraryDropSubmissionCoordinator(
      send: { command in await sender.send(command) },
      rejectLocally: model.rejectLibraryDropLocally)
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000102")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "A")

    model.markLibraryDropAdding(requestID: requestID, target: target)
    coordinator.submit(target: target, rules: [.artist(name: "Birdy")], requestID: requestID)

    await sender.waitForCount(1)
    await Task.yield()
    XCTAssertFalse(model.isLibraryDropAdding(requestID: requestID))
    XCTAssertEqual(
      model.dropOutcome,
      .rejected(target: target, message: "Couldn’t send this addition to Classick."))
    XCTAssertTrue(model.persistedDropAcknowledgements.isEmpty)
  }

  func testLostAcknowledgementReplaysByteIdenticalAdditiveIntent() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(
      .addSelectionToDevice(
        requestID: UUID(uuidString: "00000000-0000-0000-0000-000000000103")!,
        serial: "A", rules: [.artist(name: "Birdy")]))

    let firstWrite = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: firstWrite.requestID, connectionGeneration: 1)
    XCTAssertNil(outbox.nextIntent(for: 1))
    XCTAssertEqual(outbox.nextIntent(for: 2)?.bytes, firstWrite.bytes)
  }

  private func seedDevice(_ serial: String, in model: AppModel) {
    model.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 1,
          devices: [
            DeviceSnapshotWire(
              identity: .init(serial: serial, modelLabel: "iPod Classic", name: "Michael's iPod"),
              configured: true, connected: true, mount: "/Volumes/IPOD", phase: .idle,
              sessionID: nil, storage: nil, syncedCount: 0, libraryCount: nil,
              latestSuccessfulSync: nil, latestAttempt: nil, lastTerminalError: nil,
              selectionRevision: 0, settingsRevision: 0, subscriptionsRevision: 0)
          ])))
  }
}

private actor FlowCommandRecorder {
  private var commands: [DaemonCommand] = []
  private var dispositions: [SendDisposition]

  init(dispositions: [SendDisposition] = []) {
    self.dispositions = dispositions
  }

  func send(_ command: DaemonCommand) -> SendDisposition {
    commands.append(command)
    return dispositions.isEmpty ? .sent : dispositions.removeFirst()
  }

  func waitForCount(_ count: Int) async {
    while commands.count < count { await Task.yield() }
  }

  var targets: [String] {
    commands.compactMap { command in
      switch command {
      case .addSelectionToDevice(let serial, _, _): "device:\(serial)"
      case .appendSelectionToPlaylist(let slug, _, _): "playlist:\(slug)"
      default: nil
      }
    }
  }
}
