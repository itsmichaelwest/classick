import XCTest

@testable import Classick

final class SyncOutcomeDisplayTests: XCTestCase {
  /// The four values the daemon can send (`SyncOutcome` in
  /// `crates/classick/src/wire/history.rs`).
  func testEveryWireOutcomeGetsItsOwnLabelAndSymbol() {
    let displays = ["ok", "error", "aborted", "cancelled"].map(SyncOutcomeDisplay.make)

    XCTAssertEqual(displays.map(\.label), ["Synced", "Failed", "Interrupted", "Cancelled"])
    XCTAssertEqual(Set(displays.map(\.systemImage)).count, 4)
  }

  func testSuccessAndFailureAreDistinguishableWithoutReadingTheLabel() {
    XCTAssertNotEqual(
      SyncOutcomeDisplay.make("ok").systemImage,
      SyncOutcomeDisplay.make("error").systemImage)
  }

  /// A newer daemon's outcome must still render as itself — the column is
  /// informational, and dropping an unknown row's outcome would read as a
  /// blank cell.
  func testUnknownOutcomeFallsBackToItsOwnValue() {
    let display = SyncOutcomeDisplay.make("throttled")

    XCTAssertEqual(display.raw, "throttled")
    XCTAssertEqual(display.label, "Throttled")
    XCTAssertEqual(display.systemImage, "questionmark.circle")
  }

  /// Sorting the Outcome column groups by the wire value, not the display
  /// label, so both spellings of cancelled land together.
  func testBothCancelledSpellingsShareALabel() {
    XCTAssertEqual(SyncOutcomeDisplay.make("canceled").label, "Cancelled")
    XCTAssertEqual(SyncOutcomeDisplay.make("cancelled").label, "Cancelled")
  }
}
