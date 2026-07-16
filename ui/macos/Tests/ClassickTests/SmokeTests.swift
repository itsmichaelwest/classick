import AppKit
import XCTest
@testable import Classick

// Placeholder so the test target builds before Task 2 adds real tests.
final class SmokeTests: XCTestCase {
    func testItCompiles() {
        XCTAssertTrue(true)
    }

    @MainActor
    func testAppDoesNotQuitWhenLastWindowCloses() {
        let delegate = AppDelegate()
        // Hybrid app: closing the main window must leave the app (and its daemon)
        // running in the Dock + menu bar.
        XCTAssertFalse(delegate.applicationShouldTerminateAfterLastWindowClosed(NSApplication.shared))
    }
}
