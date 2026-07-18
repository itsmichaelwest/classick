import XCTest
@testable import Classick

/// The `notify_on` preference used to be ignored on macOS — a banner fired on
/// every sync regardless. These pin the policy that now gates `syncFinished`.
final class NotifierPolicyTests: XCTestCase {
    func testAllNotifiesForSuccessAndFailure() {
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "all", success: true, isScanning: false))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "all", success: false, isScanning: false))
    }

    func testErrorsOnlyNotifiesOnlyOnFailure() {
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: true, isScanning: false))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: false, isScanning: false))
    }

    func testNoneNeverNotifies() {
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "none", success: true, isScanning: false))
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "none", success: false, isScanning: false))
    }

    func testNilOrUnknownDefaultsToAll() {
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: nil, success: true, isScanning: false))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "bogus", success: false, isScanning: false))
    }

    /// A `--scan-library` subprocess streams the same header/summary/finish
    /// wire as a real sync — its `finish` used to fire a bogus
    /// "Sync complete / N added" banner for tracks added to the INDEX, not
    /// the iPod. Scanning suppresses the banner regardless of `notify_on`
    /// or outcome.
    func testScanningSuppressesBannerRegardlessOfPolicy() {
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "all", success: true, isScanning: true))
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "all", success: false, isScanning: true))
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: false, isScanning: true))
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: nil, success: true, isScanning: true))
    }
}
