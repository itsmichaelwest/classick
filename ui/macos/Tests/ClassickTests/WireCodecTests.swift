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

    func testTrackStartDecodesOptionalEta() throws {
        let withEta = #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#
        let noEta = #"{"type":"track_start","current":1,"total":10,"label":"Y"}"#
        let d = JSONDecoder()
        if case let .trackStart(_, _, _, eta) = try d.decode(SyncEvent.self, from: Data(withEta.utf8)) {
            XCTAssertEqual(eta, 42)
        } else { XCTFail("expected trackStart") }
        if case let .trackStart(_, _, _, eta) = try d.decode(SyncEvent.self, from: Data(noEta.utf8)) {
            XCTAssertNil(eta)
        } else { XCTFail("expected trackStart") }
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

    // MARK: - Protocol 1.5.0 (subprocess 1.3.0 finish fields, daemon custom_selection/history/replace_library)

    func testFinishDecodesWithoutNewFieldsStaysAbsentTolerant() throws {
        let json = #"{"type":"finish","success":true}"#
        let event = try JSONDecoder().decode(SyncEvent.self, from: Data(json.utf8))
        guard case let .finish(success, skippedForSpace, artwork, dbRestored) = event else { return XCTFail() }
        XCTAssertTrue(success)
        XCTAssertNil(skippedForSpace)
        XCTAssertNil(artwork)
        XCTAssertFalse(dbRestored, "absent db_restored must default false, not nil-crash")
    }

    func testFinishDecodesSkippedForSpaceArtworkAndDbRestored() throws {
        let json = #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#
        let event = try JSONDecoder().decode(SyncEvent.self, from: Data(json.utf8))
        guard case let .finish(success, skippedForSpace, artwork, dbRestored) = event else { return XCTFail() }
        XCTAssertTrue(success)
        XCTAssertEqual(skippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
        XCTAssertEqual(artwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
        XCTAssertTrue(dbRestored)
    }

    func testIpodIdentityDecodesMissingCustomSelectionAsFalse() throws {
        let json = #"{"serial":"0xABC","model_label":"iPod Classic (3rd gen)"}"#
        let identity = try JSONDecoder().decode(IpodIdentity.self, from: Data(json.utf8))
        XCTAssertFalse(identity.customSelection, "older core/daemon omitting the field must default to shared selection")
    }

    func testIpodIdentityRoundTripsCustomSelectionTrue() throws {
        let identity = IpodIdentity(serial: "0xABC", modelLabel: "iPod Classic (3rd gen)", name: nil, customSelection: true)
        let data = try JSONEncoder().encode(identity)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["custom_selection"] as? Bool, true)
        let decoded = try JSONDecoder().decode(IpodIdentity.self, from: data)
        XCTAssertTrue(decoded.customSelection)
    }

    func testEncodesReplaceLibrary() throws {
        let data = try JSONEncoder().encode(DaemonCommand.replaceLibrary)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "replace_library")
    }

    func testHistoryEntryDecodesSummaryAndDbRestored() throws {
        let json = #"{"timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok","summary":{"add":1,"modify":0,"remove":0,"unchanged":10,"skipped_for_space_tracks":183,"skipped_for_space_bytes":9876543210,"artwork_failed_sources":2},"db_restored":true}"#
        let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
        XCTAssertEqual(entry.summary?.skippedForSpaceTracks, 183)
        XCTAssertEqual(entry.summary?.skippedForSpaceBytes, 9_876_543_210)
        XCTAssertEqual(entry.summary?.artworkFailedSources, 2)
        XCTAssertTrue(entry.dbRestored)
    }

    func testHistoryEntryDecodesWithoutNewFieldsDefaultsCleanly() throws {
        // Pre-1.5.0 history.json entries: no `summary`, no `db_restored`.
        let json = #"{"timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok"}"#
        let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
        XCTAssertNil(entry.summary)
        XCTAssertFalse(entry.dbRestored)
    }

    func testHistoryEntrySummaryMissingNewSubfieldsDefaultsToZero() throws {
        // A `summary` object from an older daemon build (pre-1.5.0) that has
        // the original fields but not the three new ones.
        let json = #"{"timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok","summary":{"add":1,"modify":0,"remove":0,"unchanged":10}}"#
        let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
        XCTAssertEqual(entry.summary?.skippedForSpaceTracks, 0)
        XCTAssertEqual(entry.summary?.skippedForSpaceBytes, 0)
        XCTAssertEqual(entry.summary?.artworkFailedSources, 0)
    }
}
