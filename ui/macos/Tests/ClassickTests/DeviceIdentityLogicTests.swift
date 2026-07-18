import XCTest
@testable import Classick

/// Pure-logic coverage for `DeviceIdentityLogic` (review finding #2): with a
/// DIFFERENT iPod connected than the device page being viewed,
/// `AppModel`'s singleton connected-device fields (`device`, `deviceStorage`,
/// `lastSync`, `syncedCount`) belong to that other, connected device — these
/// functions gate every one of them on `isConnected` (page's serial ==
/// the connected device's serial) so a page never borrows another device's
/// numbers.
final class DeviceIdentityLogicTests: XCTestCase {
    private let connected = DeviceState(serial: "0xA", model: "iPod Classic (3rd gen)", name: "Michael's iPod", drive: "/Volumes/IPOD")
    private let paired = IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: "Michael's iPod", customSelection: false)

    // MARK: - deviceName

    func testDeviceNameUsesConnectedDeviceIdentityWhenConnected() {
        XCTAssertEqual(
            DeviceIdentityLogic.deviceName(serial: "0xA", isConnected: true, connectedDevice: connected, pairedIpod: paired),
            "Michael's iPod")
    }

    func testDeviceNameFallsBackToModelWhenConnectedDeviceHasNoName() {
        let unnamed = DeviceState(serial: "0xA", model: "iPod Classic (3rd gen)", name: nil, drive: "/Volumes/IPOD")
        XCTAssertEqual(
            DeviceIdentityLogic.deviceName(serial: "0xA", isConnected: true, connectedDevice: unnamed, pairedIpod: nil),
            "iPod Classic (3rd gen)")
    }

    func testDeviceNameFallsBackToThisIpodWhenConnectedButNoIdentityAtAll() {
        XCTAssertEqual(
            DeviceIdentityLogic.deviceName(serial: "0xA", isConnected: true, connectedDevice: nil, pairedIpod: nil),
            "This iPod")
    }

    /// The trust-critical case: this page is for a DIFFERENT device than the
    /// one physically connected right now. The connected device's own name
    /// must never leak onto this page.
    func testDeviceNameUsesPairedIdentityWhenNotConnectedButSerialMatchesPairedIpod() {
        // Paired iPod 0xB isn't plugged in right now (0xA is, or nothing is),
        // but this page is showing 0xB's cached identity from config_update.
        let pairedB = IpodIdentity(serial: "0xB", modelLabel: "iPod Classic (5th gen)", name: "Old iPod", customSelection: false)
        XCTAssertEqual(
            DeviceIdentityLogic.deviceName(serial: "0xB", isConnected: false, connectedDevice: connected, pairedIpod: pairedB),
            "Old iPod", "must use the PAIRED identity for 0xB, never the connected device's (0xA's) name")
    }

    func testDeviceNameFallsBackToSerialWhenNotConnectedAndPairedIpodDoesNotMatch() {
        XCTAssertEqual(
            DeviceIdentityLogic.deviceName(serial: "0xB", isConnected: false, connectedDevice: connected, pairedIpod: paired),
            "0xB", "paired identity is for 0xA, not this page's 0xB -> bare serial, not the connected device's name")
    }

    // MARK: - capacityText

    func testCapacityTextUsesStorageTextWhenConnected() {
        XCTAssertEqual(DeviceIdentityLogic.capacityText(isConnected: true, storageText: "12 / 64 GB"), "12 / 64 GB")
    }

    func testCapacityTextNilWhenConnectedButStorageNotYetKnown() {
        XCTAssertNil(DeviceIdentityLogic.capacityText(isConnected: true, storageText: nil), "transient race before deviceStorage arrives -> omit, not placeholder")
    }

    func testCapacityTextPlaceholderWhenNotConnectedEvenIfStorageTextHappensToBeSet() {
        XCTAssertEqual(
            DeviceIdentityLogic.capacityText(isConnected: false, storageText: "12 / 64 GB"),
            DeviceIdentityLogic.placeholder,
            "must never show the CONNECTED device's capacity on a different device's page")
    }

    // MARK: - syncedSummaryText

    func testSyncedSummaryTextWithTotalWhenConnected() {
        XCTAssertEqual(DeviceIdentityLogic.syncedSummaryText(isConnected: true, syncedCount: 119, libraryCount: 1500), "119 of 1500")
    }

    func testSyncedSummaryTextWithoutTotalWhenConnected() {
        XCTAssertEqual(DeviceIdentityLogic.syncedSummaryText(isConnected: true, syncedCount: 119, libraryCount: nil), "119")
    }

    func testSyncedSummaryTextPlaceholderWhenNotConnected() {
        XCTAssertEqual(
            DeviceIdentityLogic.syncedSummaryText(isConnected: false, syncedCount: 119, libraryCount: 1500),
            DeviceIdentityLogic.placeholder,
            "must never show the CONNECTED device's synced count on a different device's page")
    }
}
