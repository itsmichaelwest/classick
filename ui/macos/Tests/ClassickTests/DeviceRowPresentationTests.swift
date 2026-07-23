import XCTest

@testable import Classick

final class DeviceRowPresentationTests: XCTestCase {
  func testNoKnownDeviceIsActionlessAndRetainsLibraryContext() {
    let presentation = DeviceRowPresentation.make(device: nil, libraryCount: 91)

    XCTAssertEqual(presentation.serial, nil)
    XCTAssertEqual(presentation.title, "No iPod connected")
    XCTAssertEqual(presentation.subtitle, "Plug in your iPod to sync")
    XCTAssertEqual(presentation.caption, "91 tracks selected")
    XCTAssertEqual(presentation.meter, .unavailable)
    XCTAssertNil(presentation.primaryAction)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testIdleDeviceUsesCapacityProjectionAndSyncAction() {
    var device = makeDevice(phase: .idle)
    device.storage = .init(free: 120_000_000_000, total: 160_000_000_000)
    device.preview = .init(
      serial: device.identity.serial,
      selectedTracks: 91,
      selectedBytes: 3_119_000_000,
      playlistExtraTracks: 0,
      playlistExtraBytes: 0,
      projectedFreeBytes: 110_000_000_000,
      unresolvedSubscriptions: nil,
      acknowledgedRequestID: "preview")
    device.latestSuccessfulSync = history(timestamp: "recently")

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.serial, "A")
    XCTAssertEqual(presentation.title, "Michael's iPod")
    XCTAssertEqual(presentation.subtitle, "Last synced at recently")
    XCTAssertEqual(
      presentation.meter,
      .capacity(
        used: 40_000_000_000,
        total: 160_000_000_000,
        projectedUsed: 50_000_000_000))
    XCTAssertEqual(presentation.primaryAction, .syncNow)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testSyncingDeviceUsesLiveProgressAndStableActions() {
    let label = "A very long artist and album name/07 A track title that must not be discarded.flac"
    var device = makeDevice(phase: .syncing, sessionID: 7)
    device.syncProgress = .init(current: 14, total: 91, label: label, etaSecs: 125)

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.title, "Michael's iPod")
    XCTAssertEqual(presentation.subtitle, "Adding 91 tracks")
    XCTAssertEqual(
      presentation.meter,
      .progress(current: 14, total: 91, label: label, etaSeconds: 125))
    XCTAssertEqual(presentation.primaryAction, .pause)
    XCTAssertEqual(presentation.secondaryAction, .cancel)
  }

  func testSyncingWithoutTrackProgressIsIndeterminate() {
    let device = makeDevice(phase: .syncing, sessionID: 7)

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.subtitle, "Preparing sync…")
    XCTAssertEqual(presentation.meter, .indeterminate(label: "Preparing sync…"))
    XCTAssertEqual(presentation.primaryAction, .pause)
    XCTAssertEqual(presentation.secondaryAction, .cancel)
  }

  func testFinalizingUsesRequiredCopyAndHasNoActions() {
    var device = makeDevice(phase: .syncing, sessionID: 7)
    device.finalization = .init(reason: .cancelled, stagedAlbums: 2, stagedTracks: 17)

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.title, "Michael's iPod")
    XCTAssertEqual(presentation.subtitle, "Finishing sync…")
    XCTAssertEqual(presentation.caption, "Keep the iPod connected")
    XCTAssertEqual(presentation.meter, .indeterminate(label: "Saving completed albums"))
    XCTAssertNil(presentation.primaryAction)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testPausedDeviceUsesKnownProgressAndResumeAction() {
    var device = makeDevice(phase: .paused, sessionID: 7)
    device.syncedCount = 14
    device.libraryCount = 91

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.subtitle, "Sync paused")
    XCTAssertEqual(
      presentation.meter,
      .progress(current: 14, total: 91, label: "14 of 91 synced", etaSeconds: nil))
    XCTAssertEqual(presentation.primaryAction, .resume)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testScanningKeepsIdentityInTheSharedShell() {
    let device = makeDevice(phase: .idle)

    let presentation = DeviceRowPresentation.make(
      device: device,
      libraryCount: 91,
      globalPhase: .scanning(current: 12, total: 91))

    XCTAssertEqual(presentation.title, "Michael's iPod")
    XCTAssertEqual(presentation.subtitle, "Updating library…")
    XCTAssertEqual(
      presentation.meter,
      .progress(current: 12, total: 91, label: "Scanning library", etaSeconds: nil))
    XCTAssertNil(presentation.primaryAction)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testDisconnectedAndErrorStatesRetainDeviceIdentity() {
    let disconnected = DeviceRowPresentation.make(
      device: makeDevice(phase: .disconnected, connected: false), libraryCount: 91)
    let message = "The iPod database could not be published after artwork verification."
    let failed = DeviceRowPresentation.make(
      device: makeDevice(phase: .error(message)), libraryCount: 91)

    XCTAssertEqual(disconnected.title, "Michael's iPod")
    XCTAssertEqual(disconnected.subtitle, "Not connected")
    XCTAssertEqual(disconnected.caption, "Plug it in to sync")
    XCTAssertEqual(disconnected.meter, .unavailable)
    XCTAssertNil(disconnected.primaryAction)

    XCTAssertEqual(failed.title, "Michael's iPod")
    XCTAssertEqual(failed.subtitle, "Sync failed")
    XCTAssertEqual(failed.caption, message)
    XCTAssertEqual(failed.meter, .unavailable)
    XCTAssertEqual(failed.primaryAction, .retry)
    XCTAssertEqual(failed.secondaryAction, .details)
  }

  func testUnconfiguredDeviceRetainsIdentityAndOffersSetup() {
    let device = makeDevice(phase: .unconfigured, configured: false)

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.serial, "A")
    XCTAssertEqual(presentation.title, "Michael's iPod")
    XCTAssertEqual(presentation.subtitle, "iPod not set up")
    XCTAssertEqual(presentation.meter, .unavailable)
    XCTAssertEqual(presentation.primaryAction, .setUp)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testAppleInitializationReadinessReplacesClassickSetupAction() {
    let device = makeDevice(
      phase: .unconfigured, configured: false, readiness: "needs_apple_initialization")

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.subtitle, "Finish setup in Finder")
    XCTAssertTrue(presentation.caption?.contains("Apple software") == true)
    XCTAssertNil(presentation.primaryAction)
    XCTAssertNil(presentation.secondaryAction)
  }

  func testInvalidDatabaseAndUnknownReadinessOfferNoMutation() {
    for readiness in ["invalid_database", "identity_unavailable", "future_state"] {
      let presentation = DeviceRowPresentation.make(
        device: makeDevice(phase: .idle, readiness: readiness), libraryCount: 91)
      XCTAssertEqual(presentation.meter, .unavailable)
      XCTAssertNil(presentation.primaryAction, readiness)
      XCTAssertNil(presentation.secondaryAction, readiness)
    }
  }

  func testLongIdentityAndErrorCopyRemainIntactInPresentation() {
    let longName = "Michael's extraordinarily long engraved silver iPod Classic used for road trips"
    let longError =
      "Classick could not verify the complete artwork publication for the selected device."
    let device = makeDevice(name: longName, phase: .error(longError))

    let presentation = DeviceRowPresentation.make(device: device, libraryCount: 91)

    XCTAssertEqual(presentation.title, longName)
    XCTAssertEqual(presentation.caption, longError)
    XCTAssertEqual(DeviceRowLayout.titleLineLimit, 1)
    XCTAssertEqual(DeviceRowLayout.subtitleLineLimit, 1)
    XCTAssertEqual(DeviceRowLayout.captionLineLimit, 1)
  }

  func testStableShellUsesApprovedGeometry() {
    XCTAssertEqual(DeviceRowLayout.outerInset, 20)
    XCTAssertEqual(DeviceRowLayout.cornerRadius, 16)
    XCTAssertEqual(DeviceRowLayout.horizontalPadding, 16)
    XCTAssertEqual(DeviceRowLayout.verticalPadding, 10)
    XCTAssertEqual(DeviceRowLayout.artworkSize, 40)
    XCTAssertEqual(DeviceRowLayout.headerToMeterSpacing, 12)
    XCTAssertEqual(DeviceRowLayout.meterHeight, 6)
  }

  func testSelectedDeviceWinsBeforeActiveSession() {
    let selected = makeDevice(serial: "A", name: "Selected iPod", phase: .idle)
    let active = makeDevice(
      serial: "B", name: "Active iPod", phase: .syncing, sessionID: 7)

    let presentation = DeviceRowPresentation.make(
      devices: ["B": active, "A": selected],
      selectedSerial: "A",
      globalPhase: .idle,
      libraryCount: 91)

    XCTAssertEqual(presentation.serial, "A")
    XCTAssertEqual(presentation.title, "Selected iPod")
  }

  func testSoleActiveSessionIsChosenWithoutASelection() {
    let remembered = makeDevice(serial: "A", name: "Remembered iPod", phase: .disconnected)
    let active = makeDevice(
      serial: "B", name: "Active iPod", phase: .syncing, sessionID: 7)

    let presentation = DeviceRowPresentation.make(
      devices: ["A": remembered, "B": active],
      selectedSerial: nil,
      globalPhase: .idle,
      libraryCount: 91)

    XCTAssertEqual(presentation.serial, "B")
    XCTAssertEqual(presentation.title, "Active iPod")
  }

  func testSoleInventoryDeviceIsATargetWithoutGuessing() {
    let onlyDevice = makeDevice(serial: "A", name: "Only iPod", phase: .idle)

    let presentation = DeviceRowPresentation.make(
      devices: ["A": onlyDevice],
      selectedSerial: nil,
      globalPhase: .idle,
      libraryCount: 91)

    XCTAssertEqual(presentation.serial, "A")
    XCTAssertEqual(presentation.primaryAction, .syncNow)
  }

  func testAmbiguousInventoryNeverPicksAnArbitraryFirstTarget() {
    let first = makeDevice(serial: "A", name: "First iPod", phase: .idle)
    let second = makeDevice(serial: "B", name: "Second iPod", phase: .idle)

    let presentation = DeviceRowPresentation.make(
      devices: ["B": second, "A": first],
      selectedSerial: nil,
      globalPhase: .idle,
      libraryCount: 91)

    XCTAssertNil(presentation.serial)
    XCTAssertEqual(presentation.title, "2 iPods available")
    XCTAssertEqual(presentation.subtitle, "Select an iPod to manage it")
    XCTAssertNil(presentation.primaryAction)
    XCTAssertNil(presentation.secondaryAction)
  }

  private func makeDevice(
    serial: String = "A",
    name: String? = "Michael's iPod",
    phase: DevicePhase,
    configured: Bool = true,
    connected: Bool = true,
    sessionID: UInt64? = nil,
    readiness: String = "ready"
  ) -> DeviceViewState {
    DeviceViewState(
      deviceID: try! DeviceID(
        String(repeating: "0", count: 16 - serial.count) + serial.uppercased()),
      identity: .init(serial: serial, modelLabel: "iPod Classic (160 GB)", name: name),
      readiness: readiness,
      configured: configured,
      connected: connected,
      mountPath: connected ? "/Volumes/\(serial)" : nil,
      phase: phase,
      sessionID: sessionID,
      storage: nil,
      syncedCount: 0,
      libraryCount: nil,
      latestSuccessfulSync: nil,
      latestAttempt: nil,
      lastTerminalError: nil,
      config: nil,
      preview: nil,
      selectionRevision: 0,
      settingsRevision: 0,
      subscriptionsRevision: 0,
      syncProgress: nil,
      finalization: nil,
      lastRun: nil)
  }

  private func history(timestamp: String) -> HistoryEntry {
    HistoryEntry(
      serial: "A",
      timestamp: timestamp,
      durationSecs: 1,
      trigger: "manual",
      outcome: "completed")
  }
}
