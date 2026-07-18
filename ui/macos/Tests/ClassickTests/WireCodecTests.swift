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

    // MARK: - Protocol 1.6.0: playlists, per-device config, device preview

    func testDecodesPlaylistsUpdate() throws {
        let json = #"{"type":"playlists_update","playlists":[{"slug":"gym","name":"Gym","kind":"manual","tracks":12,"bytes":900},{"slug":"broken","name":"broken","kind":"smart","tracks":0,"bytes":0,"error":"parse failed"}]}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .playlistsUpdate(playlists) = ev else { return XCTFail("expected playlistsUpdate") }
        XCTAssertEqual(playlists.count, 2)
        XCTAssertEqual(playlists[0], PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil))
        XCTAssertEqual(playlists[1].kind, .smart)
        XCTAssertEqual(playlists[1].error, "parse failed")
    }

    func testDecodesPlaylistDetailManual() throws {
        // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- playlist_detail example payloads"
        let json = #"{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":["Artist/Album/01.flac","B/02.flac"]}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .playlistDetail(detail) = ev else { return XCTFail("expected playlistDetail") }
        XCTAssertEqual(detail.slug, "gym")
        XCTAssertEqual(detail.name, "Gym")
        XCTAssertEqual(detail.kind, .manual)
        XCTAssertEqual(detail.tracks, ["Artist/Album/01.flac", "B/02.flac"])
        XCTAssertNil(detail.rules)
        XCTAssertNil(detail.error)
    }

    func testDecodesPlaylistDetailSmart() throws {
        // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- playlist_detail example payloads"
        let json = #"{"type":"playlist_detail","slug":"recent-idm","name":"Recent IDM","kind":"smart","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0}}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .playlistDetail(detail) = ev else { return XCTFail("expected playlistDetail") }
        XCTAssertEqual(detail.kind, .smart)
        XCTAssertNil(detail.tracks)
        XCTAssertEqual(detail.rules?.version, 1)
        XCTAssertEqual(detail.rules?.matching, .all)
        XCTAssertEqual(detail.rules?.rules, [SmartRuleWire(field: .genre, op: .is, value: "IDM")])
        XCTAssertNil(detail.rules?.limit)
        XCTAssertEqual(detail.rules?.order, .alpha)
        XCTAssertEqual(detail.rules?.seed, 0)
    }

    func testDecodesPlaylistDetailErrorOnlySetsSlugAndError() throws {
        // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- playlist_detail example payloads"
        let json = #"{"type":"playlist_detail","slug":"ghost","error":"no such playlist"}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .playlistDetail(detail) = ev else { return XCTFail("expected playlistDetail") }
        XCTAssertEqual(detail.slug, "ghost")
        XCTAssertEqual(detail.error, "no such playlist")
        XCTAssertNil(detail.name)
        XCTAssertNil(detail.kind)
        XCTAssertNil(detail.tracks)
        XCTAssertNil(detail.rules)
    }

    func testDecodesDeviceConfigUpdateFullPayload() throws {
        let json = #"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"include","rules":[{"kind":"artist","name":"Boards of Canada"}]},"subscriptions":{"playlists":["gym","chill"]},"settings":{"auto_sync":true,"rockbox_compat":false}}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .deviceConfigUpdate(serial, selection, subscriptions, settings) = ev else {
            return XCTFail("expected deviceConfigUpdate")
        }
        XCTAssertEqual(serial, "0xABC")
        XCTAssertEqual(selection.mode, .include)
        XCTAssertEqual(selection.rules, [.artist(name: "Boards of Canada")])
        XCTAssertEqual(subscriptions.playlists, ["gym", "chill"])
        XCTAssertTrue(settings.autoSync)
        XCTAssertFalse(settings.rockboxCompat)
    }

    func testDecodesDeviceConfigUpdateWithoutSettingsDefaultsCleanly() throws {
        // Absent-field tolerance: a `settings` object missing entirely must
        // not crash the decode — old/newer daemon skew must never drop the
        // whole device_config_update.
        let json = #"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"all","rules":[]},"subscriptions":{"playlists":[]}}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .deviceConfigUpdate(_, _, _, settings) = ev else { return XCTFail("expected deviceConfigUpdate") }
        XCTAssertTrue(settings.autoSync, "must default true, matching DeviceSettings::default()")
        XCTAssertFalse(settings.rockboxCompat)
    }

    func testDecodesDevicePreviewWithProjection() throws {
        // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- device_preview example payloads"
        let json = #"{"type":"device_preview","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":3,"playlist_extra_bytes":90000000,"projected_free_bytes":1200000000}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .devicePreview(preview) = ev else { return XCTFail("expected devicePreview") }
        XCTAssertEqual(preview.selectedTracks, 412)
        XCTAssertEqual(preview.selectedBytes, 5_123_456_789)
        XCTAssertEqual(preview.playlistExtraTracks, 3)
        XCTAssertEqual(preview.playlistExtraBytes, 90_000_000)
        XCTAssertEqual(preview.projectedFreeBytes, 1_200_000_000)
        XCTAssertNil(preview.unresolvedSubscriptions)
    }

    func testDecodesDevicePreviewWithUnresolvedSubscriptionsAndNullProjection() throws {
        // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- device_preview example payloads"
        let json = #"{"type":"device_preview","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"unresolved_subscriptions":["deleted-favorites"]}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .devicePreview(preview) = ev else { return XCTFail("expected devicePreview") }
        XCTAssertNil(preview.projectedFreeBytes, "null means the previewed device isn't the one currently connected")
        XCTAssertEqual(preview.unresolvedSubscriptions, ["deleted-favorites"])
    }

    func testEncodesListPlaylists() throws {
        let data = try JSONEncoder().encode(DaemonCommand.listPlaylists)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "list_playlists")
    }

    func testEncodesGetPlaylist() throws {
        let data = try JSONEncoder().encode(DaemonCommand.getPlaylist(slug: "gym"))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "get_playlist")
        XCTAssertEqual(obj["slug"] as? String, "gym")
    }

    func testEncodesDeletePlaylist() throws {
        let data = try JSONEncoder().encode(DaemonCommand.deletePlaylist(slug: "gym"))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "delete_playlist")
        XCTAssertEqual(obj["slug"] as? String, "gym")
    }

    func testEncodesSavePlaylistManualCreateOmitsSlug() throws {
        let cmd = DaemonCommand.savePlaylist(.manual(slug: nil, name: "Gym", tracks: ["Artist/Album/01.flac", "B/02.flac"]))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "save_playlist")
        let playlist = obj["playlist"] as! [String: Any]
        XCTAssertEqual(playlist["kind"] as? String, "manual")
        XCTAssertNil(playlist["slug"], "absent slug means create -- must not send a key at all")
        XCTAssertEqual(playlist["name"] as? String, "Gym")
        XCTAssertEqual(playlist["tracks"] as? [String], ["Artist/Album/01.flac", "B/02.flac"])
    }

    func testEncodesSavePlaylistManualEditIncludesSlug() throws {
        let cmd = DaemonCommand.savePlaylist(.manual(slug: "gym", name: "Gym Mix", tracks: []))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let playlist = obj["playlist"] as! [String: Any]
        XCTAssertEqual(playlist["slug"] as? String, "gym")
        XCTAssertEqual(playlist["name"] as? String, "Gym Mix")
        XCTAssertEqual(playlist["tracks"] as? [String], [])
    }

    func testEncodesSavePlaylistSmartWithRules() throws {
        let rules = SmartRulesWire(matching: .all, rules: [SmartRuleWire(field: .genre, op: .is, value: "IDM")])
        let cmd = DaemonCommand.savePlaylist(.smart(slug: "recent-idm", name: "Recent IDM", rules: rules))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let playlist = obj["playlist"] as! [String: Any]
        XCTAssertEqual(playlist["kind"] as? String, "smart")
        XCTAssertEqual(playlist["slug"] as? String, "recent-idm")
        let rulesObj = playlist["rules"] as! [String: Any]
        XCTAssertEqual(rulesObj["version"] as? Int, 1)
        XCTAssertEqual(rulesObj["matching"] as? String, "all")
        XCTAssertTrue(rulesObj["limit"] is NSNull, "limit must serialize as explicit null, not be omitted")
        XCTAssertEqual(rulesObj["order"] as? String, "alpha")
        let ruleRows = rulesObj["rules"] as! [[String: Any]]
        XCTAssertEqual(ruleRows.first?["field"] as? String, "genre")
        XCTAssertEqual(ruleRows.first?["op"] as? String, "is")
        XCTAssertEqual(ruleRows.first?["value"] as? String, "IDM")
    }

    func testEncodesGetDeviceConfig() throws {
        let data = try JSONEncoder().encode(DaemonCommand.getDeviceConfig(serial: "0xABC"))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "get_device_config")
        XCTAssertEqual(obj["serial"] as? String, "0xABC")
    }

    func testEncodesPreviewDevice() throws {
        let data = try JSONEncoder().encode(DaemonCommand.previewDevice(serial: "0xABC"))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "preview_device")
        XCTAssertEqual(obj["serial"] as? String, "0xABC")
    }

    func testEncodesSaveDeviceConfigPartialPayloadOmitsAbsentParts() throws {
        let cmd = DaemonCommand.saveDeviceConfig(
            serial: "0xABC", selection: nil, subscriptions: nil,
            settings: DeviceSettingsWire(autoSync: false, rockboxCompat: true))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "save_device_config")
        XCTAssertEqual(obj["serial"] as? String, "0xABC")
        XCTAssertNil(obj["selection"], "nil selection means don't change -- must be omitted, not null")
        XCTAssertNil(obj["subscriptions"])
        let settings = obj["settings"] as! [String: Any]
        XCTAssertEqual(settings["auto_sync"] as? Bool, false)
        XCTAssertEqual(settings["rockbox_compat"] as? Bool, true)
    }

    func testEncodesSaveDeviceConfigFullPayloadOmitsVersionOnNestedPayloads() throws {
        let cmd = DaemonCommand.saveDeviceConfig(
            serial: "0xABC",
            selection: SelectionState(mode: .include, rules: [.artist(name: "Boards of Canada")]),
            subscriptions: SubscriptionsWire(playlists: ["gym", "chill"]),
            settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false))
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let selection = obj["selection"] as! [String: Any]
        XCTAssertEqual(selection["mode"] as? String, "include")
        XCTAssertNil(selection["version"], "selection wire mirrors mode+rules only")
        let subscriptions = obj["subscriptions"] as! [String: Any]
        XCTAssertEqual(subscriptions["playlists"] as? [String], ["gym", "chill"])
        XCTAssertNil(subscriptions["version"], "subscriptions wire mirrors playlists only, no version")
        let settings = obj["settings"] as! [String: Any]
        XCTAssertNil(settings["version"], "settings wire mirrors auto_sync+rockbox_compat only, no version")
    }

    // MARK: - resolve_tracks / resolved_tracks (protocol 1.7.0 — Add Songs picker)

    func testEncodesResolveTracksReusingSelectionRuleShape() throws {
        let cmd = DaemonCommand.resolveTracks(rules: [
            .artist(name: "Boards of Canada"),
            .album(artist: "Squarepusher", album: "Hard Normal Daddy"),
            .genre(name: "IDM"),
        ])
        let data = try JSONEncoder().encode(cmd)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "resolve_tracks")
        let rules = obj["rules"] as! [[String: Any]]
        XCTAssertEqual(rules.count, 3)
        XCTAssertEqual(rules[0]["kind"] as? String, "artist")
        XCTAssertEqual(rules[0]["name"] as? String, "Boards of Canada")
        XCTAssertEqual(rules[1]["kind"] as? String, "album")
        XCTAssertEqual(rules[1]["artist"] as? String, "Squarepusher")
        XCTAssertEqual(rules[1]["album"] as? String, "Hard Normal Daddy")
        XCTAssertEqual(rules[2]["kind"] as? String, "genre")
        XCTAssertEqual(rules[2]["name"] as? String, "IDM")
    }

    func testEncodesResolveTracksWithEmptyRules() throws {
        let data = try JSONEncoder().encode(DaemonCommand.resolveTracks(rules: []))
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "resolve_tracks")
        let rules = obj["rules"] as! [[String: Any]]
        XCTAssertTrue(rules.isEmpty)
    }

    func testDecodesResolvedTracks() throws {
        let json = #"{"type":"resolved_tracks","tracks":["Artist/Album/01.flac","B/02.flac"]}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .resolvedTracks(tracks) = ev else { return XCTFail() }
        XCTAssertEqual(tracks, ["Artist/Album/01.flac", "B/02.flac"])
    }

    func testDecodesResolvedTracksEmptyIsValidNotAnError() throws {
        let json = #"{"type":"resolved_tracks","tracks":[]}"#
        let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
        guard case let .resolvedTracks(tracks) = ev else { return XCTFail() }
        XCTAssertEqual(tracks, [])
    }
}
