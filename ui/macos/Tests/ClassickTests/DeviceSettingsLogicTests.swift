import XCTest
@testable import Classick

/// Pure-logic coverage for the device Settings page (Task 6, Figma frame
/// `4:6349`) — no SwiftUI involved, mirroring `DeviceMusicLogicTests`'
/// style. `saveSettingsCommand` is the load-bearing one: every toggle edit
/// on this page must touch ONLY `settings` on the wire — selection/
/// subscription rules are the device Music page's job (Task 5) and must
/// never be disturbed by a Settings-page edit.
final class DeviceSettingsLogicTests: XCTestCase {
    // MARK: - saveSettingsCommand: settings-only save-device-config

    func testSaveSettingsCommandTouchesOnlySettings() throws {
        let cmd = DeviceSettingsLogic.saveSettingsCommand(
            serial: "0xABC", settings: DeviceSettingsWire(autoSync: false, rockboxCompat: true))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "save_device_config")
        XCTAssertEqual(obj["serial"] as? String, "0xABC")
        XCTAssertNil(obj["selection"], "a Settings-page edit must never touch selection rules")
        XCTAssertNil(obj["subscriptions"], "a Settings-page edit must never touch playlist subscriptions")
        let settings = obj["settings"] as! [String: Any]
        XCTAssertEqual(settings["auto_sync"] as? Bool, false)
        XCTAssertEqual(settings["rockbox_compat"] as? Bool, true)
    }

    func testSaveSettingsCommandRoundTripsBothFlagsIndependently() throws {
        let cmd = DeviceSettingsLogic.saveSettingsCommand(
            serial: "0xDEF", settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let settings = obj["settings"] as! [String: Any]
        XCTAssertEqual(settings["auto_sync"] as? Bool, true)
        XCTAssertEqual(settings["rockbox_compat"] as? Bool, false)
    }

    // MARK: - Replace Library disabled predicate (syncing/scanning only)

    func testReplaceDisabledWhileSyncing() {
        XCTAssertTrue(DeviceSettingsLogic.isReplaceLibraryDisabled(
            phase: .syncing(current: 1, total: 10, label: "x", etaSecs: nil)))
    }

    func testReplaceDisabledWhileScanning() {
        XCTAssertTrue(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .scanning(current: 1, total: 10)))
    }

    func testReplaceEnabledWhenIdle() {
        XCTAssertFalse(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .idle))
    }

    func testReplaceEnabledWhenDisconnected() {
        // Disconnected pages stay editable per the Global Constraints; Replace
        // itself still requires a connected device at the daemon layer, but
        // that's the daemon's `sync_rejected` to enforce, not this predicate's
        // — it only guards against racing an in-flight sync/scan.
        XCTAssertFalse(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .noDevice))
    }

    // MARK: - Disconnected caption (Global Constraints exact string)

    func testCaptionNilWhenConnected() {
        XCTAssertNil(DeviceSettingsLogic.caption(isConnected: true))
    }

    func testCaptionDisconnected() {
        XCTAssertEqual(DeviceSettingsLogic.caption(isConnected: false), "Not connected — changes apply on next sync")
    }

    // MARK: - Remove-iPod caption (dynamic device name)

    func testRemoveCaptionInterpolatesDeviceName() {
        XCTAssertEqual(DeviceSettingsLogic.removeCaption(deviceName: "Michael's iPod"), "Remove Michael's iPod from Classick")
    }

    func testRemoveCaptionFallbackName() {
        XCTAssertEqual(DeviceSettingsLogic.removeCaption(deviceName: "This iPod"), "Remove This iPod from Classick")
    }
}
