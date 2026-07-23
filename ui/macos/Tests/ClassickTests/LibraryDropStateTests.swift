import XCTest

@testable import Classick

final class LibraryDropStateTests: XCTestCase {
  private let nonce = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!

  func testConfiguredDeviceMatrixAcceptsConnectedAndRememberedButRejectsUnconfigured() {
    XCTAssertEqual(
      LibraryDropEligibility.targetForDevice(
        device(serial: "A", configured: true, connected: true)),
      .device(serial: "A", displayName: "Michael's iPod"))
    XCTAssertEqual(
      LibraryDropEligibility.targetForDevice(
        device(serial: "A", configured: true, connected: false)),
      .device(serial: "A", displayName: "Michael's iPod"))
    XCTAssertNil(
      LibraryDropEligibility.targetForDevice(
        device(serial: "B", configured: false, connected: true)))
  }

  func testOnlyExplicitCardAcceptsDeviceDrop() {
    let explicit = DeviceRowPresentation(
      serial: "A", title: "Michael's iPod", subtitle: "Idle", caption: nil,
      meter: .unavailable, primaryAction: nil, secondaryAction: nil)
    let aggregate = DeviceRowPresentation(
      serial: nil, title: "2 iPods available", subtitle: "Select an iPod", caption: nil,
      meter: .unavailable, primaryAction: nil, secondaryAction: nil)

    XCTAssertEqual(
      LibraryDropEligibility.targetForCard(explicit),
      .device(serial: "A", displayName: "Michael's iPod"))
    XCTAssertNil(LibraryDropEligibility.targetForCard(aggregate))
  }

  func testPlaylistMatrixRejectsSmartAndCorrupt() {
    XCTAssertEqual(
      LibraryDropEligibility.targetForPlaylist(playlist("favorites", kind: .manual)),
      .manualPlaylist(slug: "favorites", displayName: "Favorites"))
    XCTAssertNil(LibraryDropEligibility.targetForPlaylist(playlist("recent", kind: .smart)))
    XCTAssertNil(
      LibraryDropEligibility.targetForPlaylist(
        playlist("broken", kind: .manual, error: "invalid")))
  }

  func testAcceptanceRejectsWrongNonceAndCanonicalizesCombinedRules() throws {
    let target = LibraryDropTarget.device(serial: "A", displayName: "A")
    let valid = payload(rules: [.album(artist: "Birdy", album: "Fire Within")])
    let broader = payload(rules: [.artist(name: "Birdy")])
    XCTAssertEqual(
      try LibraryDropAcceptance.rules(from: [valid, broader], expectedNonce: nonce),
      [.artist(name: "Birdy")])
    XCTAssertTrue(acceptLibraryDrop([valid], on: target, expectedNonce: nonce))
    XCTAssertFalse(acceptLibraryDrop([], on: target, expectedNonce: nonce))
    XCTAssertFalse(acceptLibraryDrop([valid], on: target, expectedNonce: UUID()))
  }

  func testAcceptanceRejectsMoreThanSixtyFourCombinedRules() {
    let items = (0..<65).map { index in payload(rules: [.artist(name: "Artist \(index)")]) }
    XCTAssertThrowsError(
      try LibraryDropAcceptance.rules(from: items, expectedNonce: nonce))
  }

  func testFeedbackCopyAndVoiceOverLabelsAreExact() {
    let target = LibraryDropTarget.manualPlaylist(slug: "favorites", displayName: "Favorites")
    XCTAssertEqual(
      LibraryDropFeedback.accessibilityLabel(summary: "Birdy", target: target),
      "Add Birdy to Favorites")
    XCTAssertEqual(DropOutcome.adding(target: target).accessibleMessage, "Adding…")
    XCTAssertEqual(DropOutcome.addedAndSyncing(serial: "A").accessibleMessage, "Added and syncing")
    XCTAssertEqual(
      DropOutcome.addedForNextSync(serial: "A").accessibleMessage, "Added for next sync")
    XCTAssertEqual(
      DropOutcome.alreadyPresent(serial: "A").accessibleMessage, "Already on this iPod")
    XCTAssertEqual(
      DropOutcome.appended(slug: "favorites", count: 2).accessibleMessage, "Appended 2 songs")
  }

  @MainActor
  func testRapidCrossTargetSubmissionsPreserveUICallbackOrder() async {
    let sender = RecordingAsyncSender()
    let coordinator = LibraryDropSubmissionCoordinator(send: sender.send)
    coordinator.submit(
      target: .device(serial: "A", displayName: "A"), rules: [.artist(name: "Birdy")],
      requestID: UUID())
    coordinator.submit(
      target: .manualPlaylist(slug: "favorites", displayName: "Favorites"),
      rules: [.genre(name: "Pop")], requestID: UUID())
    coordinator.submit(
      target: .device(serial: "B", displayName: "B"),
      rules: [.album(artist: "B", album: "Two")], requestID: UUID())

    await sender.waitForCount(3)
    let targets = await sender.targets
    XCTAssertEqual(
      targets,
      ["device:000000000000000A", "playlist:favorites", "device:000000000000000B"])
  }

  @MainActor
  func testDroppedDispositionClearsOnlyMatchingAddingState() async {
    let model = AppModel()
    let sender = RecordingAsyncSender(dispositions: [.dropped])
    let coordinator = LibraryDropSubmissionCoordinator(
      send: sender.send, rejectLocally: model.rejectLibraryDropLocally)
    let id = UUID()
    let target = LibraryDropTarget.device(serial: "A", displayName: "A")
    model.markLibraryDropAdding(requestID: id, target: target)
    coordinator.submit(target: target, rules: [.artist(name: "Birdy")], requestID: id)

    await sender.waitForCount(1)
    await Task.yield()
    XCTAssertFalse(model.isLibraryDropAdding(requestID: id))
    XCTAssertEqual(
      model.dropOutcome,
      .rejected(target: target, message: "Couldn’t send this addition to Classick."))
    XCTAssertEqual(model.persistedDropAcknowledgements, [])
  }

  @MainActor
  func testAnnouncementCoordinatorDeduplicatesAcknowledgedRequestID() {
    var messages: [String] = []
    let coordinator = LibraryDropAnnouncementCoordinator { messages.append($0) }
    coordinator.announce(
      requestID: "request-a", outcome: .addedAndSyncing(serial: "A"))
    coordinator.announce(
      requestID: "request-a", outcome: .addedAndSyncing(serial: "A"))
    XCTAssertEqual(messages, ["Added and syncing"])
  }

  private func payload(rules: [SelectionRule]) -> LibraryDragPayload {
    LibraryDragPayload(version: 1, launchNonce: nonce, rules: rules, summary: "Selection")
  }

  private func playlist(
    _ slug: String, kind: PlaylistKind, error: String? = nil
  ) -> PlaylistSummary {
    PlaylistSummary(
      slug: slug, name: slug.capitalized, kind: kind, tracks: 0, bytes: 0, error: error)
  }

  private func device(serial: String, configured: Bool, connected: Bool) -> DeviceViewState {
    DeviceViewState(
      deviceID: try! DeviceID(
        String(repeating: "0", count: 16 - serial.count) + serial.uppercased()),
      identity: DeviceIdentityWire(serial: serial, modelLabel: "iPod", name: "Michael's iPod"),
      configured: configured, connected: connected, mountPath: connected ? "/Volumes/IPOD" : nil,
      phase: connected ? .idle : .disconnected, sessionID: nil, storage: nil, syncedCount: 0,
      libraryCount: nil, latestSuccessfulSync: nil, latestAttempt: nil, lastTerminalError: nil,
      config: configured ? .defaultState : nil, preview: nil, selectionRevision: 0,
      settingsRevision: 0, subscriptionsRevision: 0, syncProgress: nil, finalization: nil,
      lastRun: nil)
  }
}

private actor RecordingAsyncSender {
  private var commands: [WireV3Command] = []
  private var dispositions: [SendDisposition]
  private var firstContinuation: CheckedContinuation<Void, Never>?
  private let suspendFirstSend: Bool

  init(suspendFirstSend: Bool = false, dispositions: [SendDisposition] = []) {
    self.suspendFirstSend = suspendFirstSend
    self.dispositions = dispositions
  }

  func send(_ command: WireV3Command) async -> SendDisposition {
    if suspendFirstSend && commands.isEmpty {
      await withCheckedContinuation { firstContinuation = $0 }
    }
    commands.append(command)
    return dispositions.isEmpty ? .sent : dispositions.removeFirst()
  }

  func resumeFirstSend() {
    firstContinuation?.resume()
    firstContinuation = nil
  }

  func waitForCount(_ count: Int) async {
    while commands.count < count { await Task.yield() }
  }

  var targets: [String] {
    commands.compactMap { command in
      switch command {
      case .addSelectionToDevice(let deviceID, _, _, _): "device:\(deviceID)"
      case .appendSelectionToPlaylist(_, let slug, _): "playlist:\(slug)"
      default: nil
      }
    }
  }
}
