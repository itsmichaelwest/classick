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
      deviceID: DeviceID("0000000000000ABC"),
      settings: DeviceSettingsWire(autoSync: false, rockboxCompat: true),
      requestID: UUID(), mutationID: UUID())
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "set_settings")
    XCTAssertEqual(obj["device_id"] as? String, "0000000000000ABC")
    XCTAssertNil(obj["selection"], "a Settings-page edit must never touch selection rules")
    XCTAssertNil(
      obj["subscriptions"], "a Settings-page edit must never touch playlist subscriptions")
    let settings = obj["settings"] as! [String: Any]
    XCTAssertEqual(settings["auto_sync"] as? Bool, false)
    XCTAssertEqual(settings["rockbox_compat"] as? Bool, true)
  }

  func testSaveSettingsCommandRoundTripsBothFlagsIndependently() throws {
    let cmd = DeviceSettingsLogic.saveSettingsCommand(
      deviceID: DeviceID("0000000000000DEF"),
      settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
      requestID: UUID(), mutationID: UUID())
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let settings = obj["settings"] as! [String: Any]
    XCTAssertEqual(settings["auto_sync"] as? Bool, true)
    XCTAssertEqual(settings["rockbox_compat"] as? Bool, false)
  }

  // MARK: - Replace Library disabled predicate (syncing/scanning, or wrong device)

  func testReplaceDisabledWhileSyncing() {
    XCTAssertTrue(
      DeviceSettingsLogic.isReplaceLibraryDisabled(
        phase: .syncing(current: 1, total: 10, label: "x", etaSecs: nil), isConnected: true))
  }

  func testReplaceDisabledWhileScanning() {
    XCTAssertTrue(
      DeviceSettingsLogic.isReplaceLibraryDisabled(
        phase: .scanning(current: 1, total: 10), isConnected: true))
  }

  func testReplaceEnabledWhenIdle() {
    XCTAssertFalse(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .idle, isConnected: true))
  }

  /// Review finding #2: superseded — this used to stay enabled while
  /// disconnected on the theory that the daemon's own `sync_rejected`
  /// would catch a bad Replace. But `replace_library` carries no serial
  /// on the wire: it wipes whichever device IS physically connected right
  /// now, not the device this page represents. If this page's device
  /// isn't the connected one — including "nothing is connected at all" —
  /// Replace Library must be disabled here, not left to the daemon, or a
  /// click can erase a DIFFERENT iPod than the one the user is looking at.
  func testReplaceDisabledWhenDisconnected() {
    XCTAssertTrue(
      DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .noDevice, isConnected: false))
  }

  /// The page's device IS connected and idle -> enabled (baseline case,
  /// restated with the new parameter for clarity alongside the two above).
  func testReplaceEnabledWhenIdleAndConnected() {
    XCTAssertFalse(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .idle, isConnected: true))
  }

  /// A DIFFERENT iPod is connected and idle (nothing syncing/scanning),
  /// but it isn't the device this page represents -> must stay disabled,
  /// since Replace Library would wipe that other, connected device.
  func testReplaceDisabledWhenPageDeviceIsNotTheConnectedDeviceEvenIfIdle() {
    XCTAssertTrue(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: .idle, isConnected: false))
  }

  // MARK: - Disconnected caption (Global Constraints exact string)

  func testCaptionNilWhenConnected() {
    XCTAssertNil(DeviceSettingsLogic.caption(isConnected: true))
  }

  func testCaptionDisconnected() {
    XCTAssertEqual(
      DeviceSettingsLogic.caption(isConnected: false), "Not connected — changes apply on next sync")
  }

  // MARK: - Remove-iPod caption (dynamic device name)

  func testRemoveCaptionInterpolatesDeviceName() {
    XCTAssertEqual(
      DeviceSettingsLogic.removeCaption(deviceName: "Michael's iPod"),
      "Remove Michael's iPod from Classick")
  }

  func testRemoveCaptionFallbackName() {
    XCTAssertEqual(
      DeviceSettingsLogic.removeCaption(deviceName: "This iPod"), "Remove This iPod from Classick")
  }
}
