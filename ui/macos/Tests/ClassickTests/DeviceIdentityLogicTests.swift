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
        // but this page is showing 0xB's cached identity from device inventory.
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

    func testAppleOwnedNameWinsOverHardwareDescription() {
        let identity = DeviceIdentityWire(
            serial: "A", modelLabel: "Legacy label", name: "Michael's iPod")
        XCTAssertEqual(
            DeviceIdentityLogic.title(identity: identity, hardware: classicHardware()),
            "Michael's iPod")
    }

    func testBlankAppleNameFallsBackToKnownFamilyAndGeneration() {
        let identity = DeviceIdentityWire(serial: "A", modelLabel: "Legacy label", name: "  ")
        XCTAssertEqual(
            DeviceIdentityLogic.title(identity: identity, hardware: classicHardware()),
            "iPod classic (3rd generation)")
    }

    func testPresentationNeverUsesLegacyStorageLabelOrInferredGenerationAsIdentity() {
        let identity = DeviceIdentityWire(
            serial: "A", modelLabel: "iPod Classic (160 GB)", name: nil)
        let unknown = WireV3Hardware(
            family: nil, generation: nil, modelCode: nil, colour: nil, firmware: nil,
            capacityBytes: .init(
                value: 160_000_000_000, source: "reported", confidence: "certain"))
        XCTAssertEqual(DeviceIdentityLogic.title(identity: identity, hardware: unknown), "iPod")

        let capacityInferred = WireV3Hardware(
            family: .init(value: "classic", source: "decoded", confidence: "certain"),
            generation: .init(value: "1", source: "inferred", confidence: "heuristic"),
            modelCode: nil, colour: nil, firmware: nil, capacityBytes: nil)
        XCTAssertEqual(
            DeviceIdentityLogic.hardwareDescription(capacityInferred), "iPod classic")
    }

    func testVoiceOverLabelStatesKnownHardwareWithoutRelyingOnArtwork() {
        let identity = DeviceIdentityWire(
            serial: "A", modelLabel: "Legacy label", name: "Michael's iPod")
        XCTAssertEqual(
            DeviceIdentityLogic.accessibilityLabel(identity: identity, hardware: classicHardware()),
            "Michael's iPod, iPod classic (3rd generation)")
    }

    func testReadinessGuidanceIsExplicitAndMutationFailsClosed() {
        XCTAssertTrue(DeviceReadinessLogic.isReady("ready"))
        XCTAssertFalse(DeviceReadinessLogic.isReady("future_state"))
        XCTAssertNil(DeviceReadinessLogic.guidance(for: "ready"))
        XCTAssertEqual(
            DeviceReadinessLogic.guidance(for: "needs_apple_initialization")?.title,
            "Finish setup in Finder")
        XCTAssertTrue(
            DeviceReadinessLogic.guidance(for: "needs_apple_initialization")?.message
                .contains("Apple software") == true)
        XCTAssertEqual(
            DeviceReadinessLogic.guidance(for: "invalid_database")?.title,
            "This iPod needs recovery")
        XCTAssertEqual(
            DeviceReadinessLogic.guidance(for: "identity_unavailable")?.title,
            "iPod identity unavailable")
        XCTAssertNotNil(DeviceReadinessLogic.guidance(for: "future_state"))
    }

    private func classicHardware() -> WireV3Hardware {
        WireV3Hardware(
            family: .init(value: "classic", source: "decoded", confidence: "certain"),
            generation: .init(value: "3", source: "decoded", confidence: "certain"),
            modelCode: nil, colour: nil, firmware: nil, capacityBytes: nil)
    }
}
