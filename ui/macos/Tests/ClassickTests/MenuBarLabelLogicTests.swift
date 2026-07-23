import XCTest

@testable import Classick

final class MenuBarLabelLogicTests: XCTestCase {
  func testDevicePhasesMapToTheirStatusGlyphs() {
    XCTAssertEqual(MenuBarLabelPresentation.make(phase: nil).systemImage, "ipod")
    XCTAssertEqual(MenuBarLabelPresentation.make(phase: .disconnected).systemImage, "ipod")
    XCTAssertEqual(MenuBarLabelPresentation.make(phase: .unconfigured).systemImage, "ipod")
    XCTAssertEqual(MenuBarLabelPresentation.make(phase: .idle).systemImage, "ipod")
    XCTAssertEqual(
      MenuBarLabelPresentation.make(phase: .syncing).systemImage,
      "arrow.triangle.2.circlepath")
    XCTAssertEqual(MenuBarLabelPresentation.make(phase: .paused).systemImage, "pause.circle")
    XCTAssertEqual(
      MenuBarLabelPresentation.make(phase: .error("Disk full")).systemImage,
      "exclamationmark.triangle")
  }

  func testScanningAndFinalizingOverrideTheGeneralPhase() {
    let syncing = makeDevice(phase: .syncing, finalizing: false)
    let finalizing = makeDevice(phase: .syncing, finalizing: true)

    XCTAssertEqual(
      MenuBarLabelPresentation.make(globalPhase: .scanning(current: 4, total: 12), device: syncing)
        .systemImage,
      "magnifyingglass")
    XCTAssertEqual(
      MenuBarLabelPresentation.make(
        globalPhase: .syncing(current: 4, total: 12, label: "", etaSecs: nil),
        device: finalizing
      )
      .systemImage,
      "arrow.triangle.2.circlepath")
  }

  func testEveryPresentationUsesTheClassickAccessibilityLabel() {
    let presentations = [
      MenuBarLabelPresentation.make(phase: .idle),
      MenuBarLabelPresentation.make(phase: .syncing),
      MenuBarLabelPresentation.make(phase: .paused),
      MenuBarLabelPresentation.make(phase: .error("Failed")),
      MenuBarLabelPresentation.make(globalPhase: .scanning(current: 1, total: 2), device: nil),
      MenuBarLabelPresentation.make(
        globalPhase: .syncing(current: 1, total: 2, label: "", etaSecs: nil),
        device: makeDevice(phase: .syncing, finalizing: true)),
    ]

    XCTAssertTrue(presentations.allSatisfy { $0.accessibilityLabel == "Classick" })
  }

  func testLabelLayoutHasOneFixedOpticalFootprint() {
    XCTAssertEqual(MenuBarLabelLayout.opticalFrameWidth, 18)
    XCTAssertEqual(MenuBarLabelLayout.opticalFrameHeight, 18)
    XCTAssertEqual(MenuBarLabelLayout.symbolPointSize, 14)
    XCTAssertEqual(MenuBarLabelLayout.symbolWeight, .medium)
  }

  private func makeDevice(phase: DevicePhase, finalizing: Bool) -> DeviceViewState {
    DeviceViewState(
      deviceID: "A",
      identity: .init(serial: "A", modelLabel: "iPod Classic", name: "Michael's iPod"),
      configured: true,
      connected: true,
      mountPath: "/Volumes/iPod",
      phase: phase,
      sessionID: 1,
      storage: nil,
      syncedCount: 0,
      libraryCount: 12,
      latestSuccessfulSync: nil,
      latestAttempt: nil,
      lastTerminalError: nil,
      config: nil,
      preview: nil,
      selectionRevision: 0,
      settingsRevision: 0,
      subscriptionsRevision: 0,
      syncProgress: nil,
      finalization: finalizing
        ? .init(reason: .cancelled, stagedAlbums: 1, stagedTracks: 4)
        : nil,
      lastRun: nil)
  }
}
