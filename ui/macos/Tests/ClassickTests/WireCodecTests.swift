import XCTest
@testable import Classick

final class WireCodecTests: XCTestCase {
    func testDecodesDeviceConnected() throws {
        let json = #"{"type":"device_connected","serial":"0x000A27002138B0A8","model_label":"iPod Classic (3rd gen)","drive":"/Volumes/IPOD","name":"Michael’s iPod"}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .deviceConnected(serial, model, drive, name) = ev else { return XCTFail() }
        XCTAssertEqual(serial, "0x000A27002138B0A8")
        XCTAssertEqual(model, "iPod Classic (3rd gen)")
        XCTAssertEqual(drive, "/Volumes/IPOD")
        XCTAssertEqual(name, "Michael’s iPod")
    }

    func testDecodesStatusUpdateMinimal() throws {
        let json = #"{"type":"status_update","state":"idle","configured":false,"ipod_connected":true}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .statusUpdate(s) = ev else { return XCTFail() }
        XCTAssertEqual(s.state, .idle); XCTAssertTrue(s.ipodConnected); XCTAssertNil(s.storage)
    }

    func testDecodesSyncEventWrappingSummary() throws {
        let inner = #"{\"type\":\"summary\",\"add\":0,\"modify\":0,\"metadata_only\":0,\"remove\":0,\"unchanged\":12,\"total_planned\":0}"#
        let json = "{\"type\":\"sync_event\",\"line\":\"\(inner)\"}"
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .syncEvent(line) = ev else { return XCTFail() }
        let sub = try JSONDecoder().decode(SyncEvent.self, from: Data(line.utf8))
        guard case let .summary(add, _, _, _, unchanged, _) = sub else { return XCTFail() }
        XCTAssertEqual(add, 0); XCTAssertEqual(unchanged, 12)
    }

    func testEncodesSaveConfig() throws {
        let cmd = DaemonCommand.saveConfig(
            source: "/music", daemon: nil,
            ipod: IpodIdentity(serial: "0xABC", modelLabel: "iPod Classic (3rd gen)", name: nil))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "save_config")
        XCTAssertEqual(obj["source"] as? String, "/music")
        XCTAssertEqual((obj["ipod"] as? [String:Any])?["serial"] as? String, "0xABC")
    }

    func testEncodesTriggerSync() throws {
        let data = try JSONEncoder().encode(DaemonCommand.triggerSync(source: .manual))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "trigger_sync")
        XCTAssertEqual(obj["source"] as? String, "manual")
    }

    func testBackfillRockboxEncodes() throws {
        let data = try JSONEncoder().encode(DaemonCommand.backfillRockbox)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "backfill_rockbox")
    }

    func testDaemonSettingsRoundTripsRockboxCompat() throws {
        let settings = DaemonSettings(
            enabled: true,
            autostartWithWindows: false,
            firstSyncMode: "auto_apply",
            subsequentSyncMode: "auto_apply",
            scheduleMinutes: 0,
            notifyOn: "all",
            rockboxCompat: true)
        let data = try JSONEncoder().encode(settings)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["rockbox_compat"] as? Bool, true)
        let decoded = try JSONDecoder().decode(DaemonSettings.self, from: data)
        XCTAssertEqual(decoded, settings)
    }

    func testDaemonSettingsDecodesMissingRockboxCompatAsFalse() throws {
        let json = #"{"enabled":true,"autostart_with_windows":false,"first_sync_mode":"auto_apply","subsequent_sync_mode":"auto_apply","schedule_minutes":0,"notify_on":"all"}"#
        let decoded = try JSONDecoder().decode(DaemonSettings.self, from: Data(json.utf8))
        XCTAssertFalse(decoded.rockboxCompat)
    }
}
