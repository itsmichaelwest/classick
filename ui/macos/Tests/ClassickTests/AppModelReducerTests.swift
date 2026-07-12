import XCTest
@testable import Classick

@MainActor
final class AppModelReducerTests: XCTestCase {
    func testDeviceConnectThenDisconnect() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "Michael’s iPod"))
        XCTAssertEqual(m.device?.name, "Michael’s iPod")
        m.apply(.deviceDisconnected(serial: "0xA"))
        XCTAssertNil(m.device)
        XCTAssertEqual(m.phase, .noDevice)
    }

    func testSyncProgressFromForwardedEvents() {
        let m = AppModel()
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        m.apply(.syncEvent(line: #"{"type":"track_start","current":34,"total":120,"label":"Karma Police"}"#))
        guard case let .syncing(cur, total, label) = m.phase else { return XCTFail() }
        XCTAssertEqual(cur, 34); XCTAssertEqual(total, 120); XCTAssertEqual(label, "Karma Police")
        m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#))
        XCTAssertEqual(m.phase, .idle)
    }

    func testPromptSurfaced() {
        let m = AppModel()
        m.apply(.syncEvent(line: #"{"type":"prompt","id":7,"message":"Source changed","options":["Apply","Cancel"]}"#))
        XCTAssertEqual(m.pendingPrompt?.id, 7)
        XCTAssertEqual(m.pendingPrompt?.options, ["Apply", "Cancel"])
    }

    func testRejectionBecomesError() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "x", drive: "/Volumes/IPOD", name: nil))
        m.apply(.syncRejected(reason: "not_configured"))
        if case .error = m.phase {} else { XCTFail("expected error phase") }
    }

    func testConfigUpdateWithIpodFlipsNotConfiguredToIdle() {
        // Regression: after first-run save_config the daemon emits config_update
        // (not a pushed status_update), so the menu must leave .notConfigured.
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        XCTAssertEqual(m.phase, .notConfigured)
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        XCTAssertEqual(m.phase, .idle)
    }

    func testDeviceSwapToUnpairedShowsNotConfigured() {
        // Regression: "configured" must be checked against the *currently
        // connected* device's serial, not just "some iPod was ever paired" —
        // otherwise swapping in an unpaired iPod after a paired one shows
        // "Sync Now" instead of "Set Up Classick…".
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        XCTAssertEqual(m.phase, .idle)

        m.apply(.deviceDisconnected(serial: "0xA"))
        m.apply(.deviceConnected(serial: "0xB", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        XCTAssertEqual(m.phase, .notConfigured)
    }
}
