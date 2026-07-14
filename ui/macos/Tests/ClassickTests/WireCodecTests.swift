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

    func testDecodesLibraryUpdate() throws {
        let line = #"{"type":"library_update","source_root":"/music","scanned_at_unix_secs":42,"artists":[{"name":"Aphex Twin","albums":[{"name":"Drukqs","genre":"IDM","tracks":30,"bytes":900}]}],"genres":[{"name":"IDM","tracks":30,"bytes":900}],"total_tracks":30,"total_bytes":900}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .libraryUpdate(info) = event else { return XCTFail("expected libraryUpdate, got \(event)") }
        XCTAssertEqual(info.sourceRoot, "/music")
        XCTAssertEqual(info.scannedAtUnixSecs, 42)
        XCTAssertEqual(info.artists.first?.name, "Aphex Twin")
        XCTAssertEqual(info.artists.first?.albums.first?.tracks, 30)
        XCTAssertEqual(info.genres.first?.name, "IDM")
    }

    func testDecodesLibraryUpdateNeverScanned() throws {
        let line = #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .libraryUpdate(info) = event else { return XCTFail() }
        XCTAssertNil(info.scannedAtUnixSecs, "null timestamp = never scanned")
    }

    func testDecodesSelectionUpdateAndPreview() throws {
        let upd = #"{"type":"selection_update","mode":"include","rules":[{"kind":"artist","name":"BoC"},{"kind":"album","artist":"Aphex Twin","album":"Drukqs"},{"kind":"genre","name":"Ambient"}]}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(upd.utf8))
        guard case let .selectionUpdate(mode, rules) = event else { return XCTFail() }
        XCTAssertEqual(mode, .include)
        XCTAssertEqual(rules, [
            .artist(name: "BoC"),
            .album(artist: "Aphex Twin", album: "Drukqs"),
            .genre(name: "Ambient"),
        ])

        let prev = #"{"type":"selection_preview","selected_tracks":2340,"selected_bytes":14200000000,"adds":120,"removes":214}"#
        let event2 = try JSONDecoder().decode(DaemonEvent.self, from: Data(prev.utf8))
        guard case let .selectionPreview(info) = event2 else { return XCTFail() }
        XCTAssertEqual(info.removes, 214)
    }

    func testEncodesSelectionCommands() throws {
        func encode(_ cmd: DaemonCommand) throws -> String {
            String(decoding: try JSONEncoder().encode(cmd), as: UTF8.self)
        }
        XCTAssertTrue(try encode(.getLibrary).contains(#""type":"get_library""#))
        XCTAssertTrue(try encode(.scanLibrary).contains(#""type":"scan_library""#))
        XCTAssertTrue(try encode(.getSelection).contains(#""type":"get_selection""#))
        let save = try encode(.saveSelection(mode: .include, rules: [.artist(name: "BoC")]))
        XCTAssertTrue(save.contains(#""type":"save_selection""#))
        XCTAssertTrue(save.contains(#""mode":"include""#))
        XCTAssertTrue(save.contains(#""kind":"artist""#))
        let preview = try encode(.previewSelection(mode: .exclude, rules: []))
        XCTAssertTrue(preview.contains(#""type":"preview_selection""#))
    }

    func testStatusUpdateScanningState() throws {
        let line = #"{"type":"status_update","state":"scanning","configured":true,"ipod_connected":false,"synced_count":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .statusUpdate(info) = event else { return XCTFail() }
        XCTAssertEqual(info.state, .scanning)
    }

    func testStatusUpdateUnknownStateDecodesAsIdle() throws {
        // Protocol rule: unknown state values MUST be treated as idle —
        // without this the whole status_update fails to decode and the
        // menu freezes on stale state when a newer daemon speaks.
        let line = #"{"type":"status_update","state":"defragging","configured":true,"ipod_connected":false,"synced_count":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .statusUpdate(info) = event else { return XCTFail("must not throw") }
        XCTAssertEqual(info.state, .idle)
    }
}
