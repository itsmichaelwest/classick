import XCTest
@testable import Classick

/// The `notify_on` preference used to be ignored on macOS — a banner fired on
/// every sync regardless. These pin the policy that now gates `syncFinished`.
final class NotifierPolicyTests: XCTestCase {
    func testAllNotifiesForSuccessAndFailure() {
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "all", success: true))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "all", success: false))
    }

    func testErrorsOnlyNotifiesOnlyOnFailure() {
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: true))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "errors_only", success: false))
    }

    func testNoneNeverNotifies() {
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "none", success: true))
        XCTAssertFalse(Notifier.shouldPostSyncFinished(notifyOn: "none", success: false))
    }

    func testNilOrUnknownDefaultsToAll() {
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: nil, success: true))
        XCTAssertTrue(Notifier.shouldPostSyncFinished(notifyOn: "bogus", success: false))
    }
}
