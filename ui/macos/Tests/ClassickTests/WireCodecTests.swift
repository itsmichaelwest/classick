import XCTest

@testable import Classick

final class WireCodecTests: XCTestCase {
  func testDecodesDeviceConnected() throws {
    let json =
      #"{"type":"device_connected","serial":"0x000A27002138B0A8","model_label":"iPod Classic (3rd gen)","drive":"/Volumes/IPOD","name":"Michael’s iPod"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .deviceConnected(let serial, let model, let drive, let name) = ev else {
      return XCTFail()
    }
    XCTAssertEqual(serial, "0x000A27002138B0A8")
    XCTAssertEqual(model, "iPod Classic (3rd gen)")
    XCTAssertEqual(drive, "/Volumes/IPOD")
    XCTAssertEqual(name, "Michael’s iPod")
  }

  func testDecodesStatusUpdateMinimal() throws {
    let json =
      #"{"type":"status_update","state":"idle","configured":false,"ipod_connected":true,"synced_count":0}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .statusUpdate(let s) = ev else { return XCTFail() }
    XCTAssertEqual(s.state, .idle)
    XCTAssertTrue(s.ipodConnected)
    XCTAssertNil(s.storage)
  }

  func testDecodesStatusUpdateAcknowledgedRequestID() throws {
    let json =
      #"{"type":"status_update","state":"idle","configured":false,"ipod_connected":false,"synced_count":0,"acknowledged_request_id":"request-status"}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .statusUpdate(let info) = event else { return XCTFail() }
    XCTAssertEqual(info.acknowledgedRequestID, "request-status")
  }

  /// Verbatim `status_update` captured from a LIVE daemon (2026-07-18)
  /// with an iPod connected. Pins the `storage` wire keys
  /// (`free_bytes`/`total_bytes` — Rust `StorageInfo`): Swift once
  /// decoded bare `free`/`total`, so every status_update with a
  /// connected device failed to decode and was silently dropped — the
  /// app's phase sat on "iPod not set up" while paired and plugged in.
  func testDecodesStatusUpdateWithStorageFromLiveWireCapture() throws {
    let json =
      #"{"type":"status_update","state":"idle","configured":true,"ipod_connected":true,"last_sync":{"serial":"RAW-A","timestamp":"2026-07-18T11:27:01Z","duration_secs":3,"trigger":"manual","outcome":"ok","summary":{"add":0,"modify":0,"remove":0,"unchanged":51,"skipped":0,"metadata_only":0,"skipped_for_space_tracks":0,"skipped_for_space_bytes":0,"artwork_failed_sources":0}},"storage":{"total_bytes":159761891328,"free_bytes":158289281024},"synced_count":0,"library_count":33}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .statusUpdate(let s) = ev else { return XCTFail() }
    XCTAssertEqual(s.storage?.total, 159_761_891_328)
    XCTAssertEqual(s.storage?.free, 158_289_281_024)
    XCTAssertEqual(s.libraryCount, 33)
    XCTAssertEqual(s.lastSync?.outcome, "ok")
  }

  func testDecodesSyncEventWrappingSummary() throws {
    let inner =
      #"{\"type\":\"summary\",\"add\":0,\"modify\":0,\"metadata_only\":0,\"remove\":0,\"unchanged\":12,\"total_planned\":0}"#
    let json =
      "{\"type\":\"sync_event\",\"line\":\"\(inner)\",\"serial\":\"RAW-A\",\"session_id\":42}"
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .syncEvent(let line, _, _) = ev else { return XCTFail() }
    let sub = try JSONDecoder().decode(SyncEvent.self, from: Data(line.utf8))
    guard case .summary(let add, _, _, _, let unchanged, _) = sub else { return XCTFail() }
    XCTAssertEqual(add, 0)
    XCTAssertEqual(unchanged, 12)
  }

  func testTrackStartDecodesOptionalEta() throws {
    let withEta = #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#
    let noEta = #"{"type":"track_start","current":1,"total":10,"label":"Y"}"#
    let d = JSONDecoder()
    if case .trackStart(_, _, _, let eta) = try d.decode(SyncEvent.self, from: Data(withEta.utf8)) {
      XCTAssertEqual(eta, 42)
    } else {
      XCTFail("expected trackStart")
    }
    if case .trackStart(_, _, _, let eta) = try d.decode(SyncEvent.self, from: Data(noEta.utf8)) {
      XCTAssertNil(eta)
    } else {
      XCTFail("expected trackStart")
    }
  }

  func testDecodesFinalizingAndCancelledSyncEvents() throws {
    let decoder = JSONDecoder()
    let finalizing = try decoder.decode(
      SyncEvent.self,
      from: Data(
        #"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#.utf8))
    guard
      case .finalizing(
        reason: .cancelled, stagedAlbums: let stagedAlbums, stagedTracks: let stagedTracks) =
        finalizing
    else {
      return XCTFail("expected finalizing")
    }
    XCTAssertEqual(stagedAlbums, 2)
    XCTAssertEqual(stagedTracks, 17)

    let cancelled = try decoder.decode(
      SyncEvent.self, from: Data(#"{"type":"cancelled"}"#.utf8))
    guard case .cancelled = cancelled else {
      return XCTFail("expected cancelled")
    }
  }

  func testEncodesSaveConfig() throws {
    let cmd = DaemonCommand.saveConfig(
      source: "/music", daemon: nil,
      ipod: IpodIdentity(serial: "0xABC", modelLabel: "iPod Classic (3rd gen)", name: nil),
      requestID: "request-a")
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "save_config")
    XCTAssertEqual(obj["source"] as? String, "/music")
    XCTAssertEqual((obj["ipod"] as? [String: Any])?["serial"] as? String, "0xABC")
  }

  func testEncodesTriggerSync() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.triggerSync(
        source: .manual, serial: "0xABC", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "trigger_sync")
    XCTAssertEqual(obj["source"] as? String, "manual")
  }

  func testBackfillRockboxEncodes() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.backfillRockbox(
        serial: "0xABC", requestID: "request-a"))
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

  func testDaemonSettingsRejectsMissingRockboxCompat() throws {
    let json =
      #"{"enabled":true,"autostart_with_windows":false,"first_sync_mode":"auto_apply","subsequent_sync_mode":"auto_apply","schedule_minutes":0,"notify_on":"all"}"#
    XCTAssertThrowsError(try JSONDecoder().decode(DaemonSettings.self, from: Data(json.utf8)))
  }

  func testDecodesLibraryUpdate() throws {
    let line =
      #"{"type":"library_update","source_root":"/music","scanned_at_unix_secs":42,"artists":[{"name":"Aphex Twin","albums":[{"name":"Drukqs","genre":"IDM","tracks":30,"bytes":900}]}],"genres":[{"name":"IDM","tracks":30,"bytes":900}],"total_tracks":30,"total_bytes":900}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
    guard case .libraryUpdate(let info) = event else {
      return XCTFail("expected libraryUpdate, got \(event)")
    }
    XCTAssertEqual(info.sourceRoot, "/music")
    XCTAssertEqual(info.scannedAtUnixSecs, 42)
    XCTAssertEqual(info.artists.first?.name, "Aphex Twin")
    XCTAssertEqual(info.artists.first?.albums.first?.tracks, 30)
    XCTAssertEqual(info.genres.first?.name, "IDM")
  }

  func testDecodesLibraryUpdateNeverScanned() throws {
    let line =
      #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
    guard case .libraryUpdate(let info) = event else { return XCTFail() }
    XCTAssertNil(info.scannedAtUnixSecs, "null timestamp = never scanned")
  }

  func testDecodesSelectionUpdateAndPreview() throws {
    let upd =
      #"{"type":"selection_update","mode":"include","rules":[{"kind":"artist","name":"BoC"},{"kind":"album","artist":"Aphex Twin","album":"Drukqs"},{"kind":"genre","name":"Ambient"}]}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(upd.utf8))
    guard case .selectionUpdate(let mode, let rules, _, _) = event else { return XCTFail() }
    XCTAssertEqual(mode, .include)
    XCTAssertEqual(
      rules,
      [
        .artist(name: "BoC"),
        .album(artist: "Aphex Twin", album: "Drukqs"),
        .genre(name: "Ambient"),
      ])

    let prev =
      #"{"type":"selection_preview","selected_tracks":2340,"selected_bytes":14200000000,"adds":120,"removes":214,"serial":"RAW-A","acknowledged_request_id":"request-a"}"#
    let event2 = try JSONDecoder().decode(DaemonEvent.self, from: Data(prev.utf8))
    guard case .selectionPreview(let info) = event2 else { return XCTFail() }
    XCTAssertEqual(info.removes, 214)
  }

  func testEncodesSelectionCommands() throws {
    func encode(_ cmd: DaemonCommand) throws -> String {
      String(decoding: try JSONEncoder().encode(cmd), as: UTF8.self)
    }
    XCTAssertTrue(
      try encode(.getLibrary(requestID: "request-library")).contains(#""type":"get_library""#))
    XCTAssertTrue(
      try encode(.scanLibrary(requestID: "request-scan")).contains(#""type":"scan_library""#))
    let preview = try encode(
      .previewSelection(
        mode: .exclude,
        rules: [],
        serial: "RAW-A",
        requestID: "request-preview"))
    XCTAssertTrue(preview.contains(#""type":"preview_selection""#))
  }

  func testStatusUpdateScanningState() throws {
    let line =
      #"{"type":"status_update","state":"scanning","configured":true,"ipod_connected":false,"synced_count":0}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
    guard case .statusUpdate(let info) = event else { return XCTFail() }
    XCTAssertEqual(info.state, .scanning)
  }

  func testStatusUpdateUnknownStateDecodesAsIdle() throws {
    // Protocol rule: unknown state values MUST be treated as idle —
    // without this the whole status_update fails to decode and the
    // menu freezes on stale state when a newer daemon speaks.
    let line =
      #"{"type":"status_update","state":"defragging","configured":true,"ipod_connected":false,"synced_count":0}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
    guard case .statusUpdate(let info) = event else { return XCTFail("must not throw") }
    XCTAssertEqual(info.state, .idle)
  }

  // MARK: - Protocol 1.5.0 (subprocess 1.3.0 finish fields, daemon custom_selection/history/replace_library)

  func testFinishDecodesWithoutNewFieldsStaysAbsentTolerant() throws {
    let json = #"{"type":"finish","success":true}"#
    let event = try JSONDecoder().decode(SyncEvent.self, from: Data(json.utf8))
    guard case .finish(let success, let skippedForSpace, let artwork, let dbRestored) = event else {
      return XCTFail()
    }
    XCTAssertTrue(success)
    XCTAssertNil(skippedForSpace)
    XCTAssertNil(artwork)
    XCTAssertFalse(dbRestored, "absent db_restored must default false, not nil-crash")
  }

  func testFinishDecodesSkippedForSpaceArtworkAndDbRestored() throws {
    let json =
      #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#
    let event = try JSONDecoder().decode(SyncEvent.self, from: Data(json.utf8))
    guard case .finish(let success, let skippedForSpace, let artwork, let dbRestored) = event else {
      return XCTFail()
    }
    XCTAssertTrue(success)
    XCTAssertEqual(skippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
    XCTAssertEqual(artwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
    XCTAssertTrue(dbRestored)
  }

  func testIpodIdentityRejectsMissingCustomSelection() throws {
    let json = #"{"serial":"0xABC","model_label":"iPod Classic (3rd gen)"}"#
    XCTAssertThrowsError(try JSONDecoder().decode(IpodIdentity.self, from: Data(json.utf8)))
  }

  func testIpodIdentityRoundTripsCustomSelectionTrue() throws {
    let identity = IpodIdentity(
      serial: "0xABC", modelLabel: "iPod Classic (3rd gen)", name: nil, customSelection: true)
    let data = try JSONEncoder().encode(identity)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["custom_selection"] as? Bool, true)
    let decoded = try JSONDecoder().decode(IpodIdentity.self, from: data)
    XCTAssertTrue(decoded.customSelection)
  }

  func testEncodesReplaceLibrary() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.replaceLibrary(
        serial: "0xABC", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "replace_library")
  }

  func testHistoryEntryDecodesSummaryAndDbRestored() throws {
    let json =
      #"{"serial":"RAW-A","timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok","summary":{"add":1,"modify":0,"remove":0,"unchanged":10,"skipped_for_space_tracks":183,"skipped_for_space_bytes":9876543210,"artwork_failed_sources":2},"db_restored":true}"#
    let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
    XCTAssertEqual(entry.summary?.skippedForSpaceTracks, 183)
    XCTAssertEqual(entry.summary?.skippedForSpaceBytes, 9_876_543_210)
    XCTAssertEqual(entry.summary?.artworkFailedSources, 2)
    XCTAssertTrue(entry.dbRestored)
  }

  func testHistoryEntryDecodesWithoutNewFieldsDefaultsCleanly() throws {
    // Pre-1.5.0 history.json entries: no `summary`, no `db_restored`.
    let json =
      #"{"serial":"RAW-A","timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok"}"#
    let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
    XCTAssertNil(entry.summary)
    XCTAssertFalse(entry.dbRestored)
  }

  func testHistoryEntrySummaryMissingNewSubfieldsDefaultsToZero() throws {
    // A `summary` object from an older daemon build (pre-1.5.0) that has
    // the original fields but not the three new ones.
    let json =
      #"{"serial":"RAW-A","timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok","summary":{"add":1,"modify":0,"remove":0,"unchanged":10}}"#
    let entry = try JSONDecoder().decode(HistoryEntry.self, from: Data(json.utf8))
    XCTAssertEqual(entry.summary?.skippedForSpaceTracks, 0)
    XCTAssertEqual(entry.summary?.skippedForSpaceBytes, 0)
    XCTAssertEqual(entry.summary?.artworkFailedSources, 0)
  }

  // MARK: - Protocol 1.6.0: playlists, per-device config, device preview

  func testDecodesPlaylistsUpdate() throws {
    let json =
      #"{"type":"playlists_update","playlists":[{"slug":"gym","name":"Gym","kind":"manual","tracks":12,"bytes":900},{"slug":"broken","name":"broken","kind":"smart","tracks":0,"bytes":0,"error":"parse failed"}]}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .playlistsUpdate(let playlists, _) = ev else {
      return XCTFail("expected playlistsUpdate")
    }
    XCTAssertEqual(playlists.count, 2)
    XCTAssertEqual(
      playlists[0],
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil))
    XCTAssertEqual(playlists[1].kind, .smart)
    XCTAssertEqual(playlists[1].error, "parse failed")
  }

  func testDecodesPlaylistDetailManual() throws {
    // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- playlist_detail example payloads"
    let json =
      #"{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":["Artist/Album/01.flac","B/02.flac"],"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .playlistDetail(let detail) = ev else { return XCTFail("expected playlistDetail") }
    XCTAssertEqual(detail.slug, "gym")
    XCTAssertEqual(detail.name, "Gym")
    XCTAssertEqual(detail.kind, .manual)
    XCTAssertEqual(detail.tracks, ["Artist/Album/01.flac", "B/02.flac"])
    XCTAssertNil(detail.rules)
    XCTAssertNil(detail.error)
  }

  func testDecodesPlaylistDetailSmart() throws {
    // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- playlist_detail example payloads"
    let json =
      #"{"type":"playlist_detail","slug":"recent-idm","name":"Recent IDM","kind":"smart","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0},"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .playlistDetail(let detail) = ev else { return XCTFail("expected playlistDetail") }
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
    let json =
      #"{"type":"playlist_detail","slug":"ghost","error":"no such playlist","acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .playlistDetail(let detail) = ev else { return XCTFail("expected playlistDetail") }
    XCTAssertEqual(detail.slug, "ghost")
    XCTAssertEqual(detail.error, "no such playlist")
    XCTAssertNil(detail.name)
    XCTAssertNil(detail.kind)
    XCTAssertNil(detail.tracks)
    XCTAssertNil(detail.rules)
  }

  func testDecodesDeviceConfigUpdateFullPayload() throws {
    let json =
      #"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"include","rules":[{"kind":"artist","name":"Boards of Canada"}]},"subscriptions":{"playlists":["gym","chill"]},"settings":{"auto_sync":true,"rockbox_compat":false},"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard
      case .deviceConfigUpdate(let serial, let selection, let subscriptions, let settings, _) = ev
    else {
      return XCTFail("expected deviceConfigUpdate")
    }
    XCTAssertEqual(serial, "0xABC")
    XCTAssertEqual(selection.mode, .include)
    XCTAssertEqual(selection.rules, [.artist(name: "Boards of Canada")])
    XCTAssertEqual(subscriptions.playlists, ["gym", "chill"])
    XCTAssertTrue(settings.autoSync)
    XCTAssertFalse(settings.rockboxCompat)
  }

  func testDeviceConfigUpdateRejectsMissingRequiredPayloadFields() {
    let fixtures = [
      #"{"type":"device_config_update","serial":"0xABC","subscriptions":{"playlists":[]},"settings":{"auto_sync":true,"rockbox_compat":false},"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"all","rules":[]},"settings":{"auto_sync":true,"rockbox_compat":false},"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"all","rules":[]},"subscriptions":{"playlists":[]},"acknowledged_request_id":"request-a"}"#,
    ]

    for fixture in fixtures {
      XCTAssertThrowsError(
        try JSONDecoder().decode(DaemonEvent.self, from: Data(fixture.utf8)),
        "expected incomplete device_config_update to be rejected: \(fixture)")
    }
  }

  func testV2EventsRejectMissingRequiredAggregateFields() {
    let fixtures = [
      #"{"type":"status_update","state":"idle","configured":true,"ipod_connected":false}"#,
      #"{"type":"library_update","scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#,
      #"{"type":"library_update","source_root":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#,
      #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"genres":[],"total_tracks":0,"total_bytes":0}"#,
      #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"total_tracks":0,"total_bytes":0}"#,
      #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_bytes":0}"#,
      #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0}"#,
      #"{"type":"selection_update","rules":[]}"#,
      #"{"type":"selection_update","mode":"all"}"#,
      #"{"type":"selection_preview","selected_bytes":0,"adds":0,"removes":0,"serial":"RAW-A","acknowledged_request_id":"request-a"}"#,
      #"{"type":"selection_preview","selected_tracks":0,"adds":0,"removes":0,"serial":"RAW-A","acknowledged_request_id":"request-a"}"#,
      #"{"type":"selection_preview","selected_tracks":0,"selected_bytes":0,"removes":0,"serial":"RAW-A","acknowledged_request_id":"request-a"}"#,
      #"{"type":"selection_preview","selected_tracks":0,"selected_bytes":0,"adds":0,"serial":"RAW-A","acknowledged_request_id":"request-a"}"#,
      #"{"type":"playlists_update"}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_bytes":0,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":0,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":0,"selected_bytes":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":0,"selected_bytes":0,"playlist_extra_tracks":0,"projected_free_bytes":null,"acknowledged_request_id":"request-a"}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":0,"selected_bytes":0,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"acknowledged_request_id":"request-a"}"#,
      #"{"type":"resolved_tracks","acknowledged_request_id":"request-a"}"#,
    ]

    for fixture in fixtures {
      XCTAssertThrowsError(
        try JSONDecoder().decode(DaemonEvent.self, from: Data(fixture.utf8)),
        "expected incomplete v2 event to be rejected: \(fixture)")
    }
  }

  func testDecodesDevicePreviewWithProjection() throws {
    // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- device_preview example payloads"
    let json =
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":3,"playlist_extra_bytes":90000000,"projected_free_bytes":1200000000,"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .devicePreview(let preview) = ev else { return XCTFail("expected devicePreview") }
    XCTAssertEqual(preview.selectedTracks, 412)
    XCTAssertEqual(preview.selectedBytes, 5_123_456_789)
    XCTAssertEqual(preview.playlistExtraTracks, 3)
    XCTAssertEqual(preview.playlistExtraBytes, 90_000_000)
    XCTAssertEqual(preview.projectedFreeBytes, 1_200_000_000)
    XCTAssertNil(preview.unresolvedSubscriptions)
  }

  func testDecodesDevicePreviewWithUnresolvedSubscriptionsAndNullProjection() throws {
    // doc-literal from docs/ipc-protocol.md "Daemon v1.6.0 -- device_preview example payloads"
    let json =
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"unresolved_subscriptions":["deleted-favorites"],"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .devicePreview(let preview) = ev else { return XCTFail("expected devicePreview") }
    XCTAssertNil(
      preview.projectedFreeBytes,
      "null means the previewed device isn't the one currently connected")
    XCTAssertEqual(preview.unresolvedSubscriptions, ["deleted-favorites"])
  }

  func testEncodesListPlaylists() throws {
    let data = try JSONEncoder().encode(DaemonCommand.listPlaylists(requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "list_playlists")
  }

  func testEncodesGetPlaylist() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.getPlaylist(slug: "gym", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "get_playlist")
    XCTAssertEqual(obj["slug"] as? String, "gym")
  }

  func testEncodesDeletePlaylist() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.deletePlaylist(slug: "gym", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "delete_playlist")
    XCTAssertEqual(obj["slug"] as? String, "gym")
  }

  func testEncodesSavePlaylistManualCreateOmitsSlug() throws {
    let cmd = DaemonCommand.savePlaylist(
      .manual(slug: nil, name: "Gym", tracks: ["Artist/Album/01.flac", "B/02.flac"]),
      requestID: "request-a")
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
    let cmd = DaemonCommand.savePlaylist(
      .manual(slug: "gym", name: "Gym Mix", tracks: []), requestID: "request-a")
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let playlist = obj["playlist"] as! [String: Any]
    XCTAssertEqual(playlist["slug"] as? String, "gym")
    XCTAssertEqual(playlist["name"] as? String, "Gym Mix")
    XCTAssertEqual(playlist["tracks"] as? [String], [])
  }

  func testEncodesSavePlaylistSmartWithRules() throws {
    let rules = SmartRulesWire(
      matching: .all, rules: [SmartRuleWire(field: .genre, op: .is, value: "IDM")])
    let cmd = DaemonCommand.savePlaylist(
      .smart(slug: "recent-idm", name: "Recent IDM", rules: rules), requestID: "request-a")
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let playlist = obj["playlist"] as! [String: Any]
    XCTAssertEqual(playlist["kind"] as? String, "smart")
    XCTAssertEqual(playlist["slug"] as? String, "recent-idm")
    let rulesObj = playlist["rules"] as! [String: Any]
    XCTAssertEqual(rulesObj["version"] as? Int, 1)
    XCTAssertEqual(rulesObj["matching"] as? String, "all")
    XCTAssertTrue(
      rulesObj["limit"] is NSNull, "limit must serialize as explicit null, not be omitted")
    XCTAssertEqual(rulesObj["order"] as? String, "alpha")
    let ruleRows = rulesObj["rules"] as! [[String: Any]]
    XCTAssertEqual(ruleRows.first?["field"] as? String, "genre")
    XCTAssertEqual(ruleRows.first?["op"] as? String, "is")
    XCTAssertEqual(ruleRows.first?["value"] as? String, "IDM")
  }

  func testEncodesGetDeviceConfig() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.getDeviceConfig(serial: "0xABC", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "get_device_config")
    XCTAssertEqual(obj["serial"] as? String, "0xABC")
  }

  func testEncodesPreviewDevice() throws {
    let data = try JSONEncoder().encode(
      DaemonCommand.previewDevice(serial: "0xABC", requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "preview_device")
    XCTAssertEqual(obj["serial"] as? String, "0xABC")
  }

  func testEncodesSaveDeviceConfigPartialPayloadOmitsAbsentParts() throws {
    let cmd = DaemonCommand.saveDeviceConfig(
      serial: "0xABC", selection: nil, subscriptions: nil,
      settings: DeviceSettingsWire(autoSync: false, rockboxCompat: true),
      requestID: "request-a")
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
      settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
      requestID: "request-a")
    let data = try JSONEncoder().encode(cmd)
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let selection = obj["selection"] as! [String: Any]
    XCTAssertEqual(selection["mode"] as? String, "include")
    XCTAssertNil(selection["version"], "selection wire mirrors mode+rules only")
    let subscriptions = obj["subscriptions"] as! [String: Any]
    XCTAssertEqual(subscriptions["playlists"] as? [String], ["gym", "chill"])
    XCTAssertNil(subscriptions["version"], "subscriptions wire mirrors playlists only, no version")
    let settings = obj["settings"] as! [String: Any]
    XCTAssertNil(
      settings["version"], "settings wire mirrors auto_sync+rockbox_compat only, no version")
  }

  // MARK: - resolve_tracks / resolved_tracks (protocol 1.7.0 — Add Songs picker)

  func testEncodesResolveTracksReusingSelectionRuleShape() throws {
    let cmd = DaemonCommand.resolveTracks(
      rules: [
        .artist(name: "Boards of Canada"),
        .album(artist: "Squarepusher", album: "Hard Normal Daddy"),
        .genre(name: "IDM"),
      ], requestID: "request-a")
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
    let data = try JSONEncoder().encode(
      DaemonCommand.resolveTracks(rules: [], requestID: "request-a"))
    let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    XCTAssertEqual(obj["type"] as? String, "resolve_tracks")
    let rules = obj["rules"] as! [[String: Any]]
    XCTAssertTrue(rules.isEmpty)
  }

  func testDecodesResolvedTracks() throws {
    let json =
      #"{"type":"resolved_tracks","tracks":["Artist/Album/01.flac","B/02.flac"],"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .resolvedTracks(let tracks, _) = ev else { return XCTFail() }
    XCTAssertEqual(tracks, ["Artist/Album/01.flac", "B/02.flac"])
  }

  func testDecodesResolvedTracksEmptyIsValidNotAnError() throws {
    let json = #"{"type":"resolved_tracks","tracks":[],"acknowledged_request_id":"request-a"}"#
    let ev = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .resolvedTracks(let tracks, _) = ev else { return XCTFail() }
    XCTAssertEqual(tracks, [])
  }

  // MARK: - Protocol 2.0.0: serial-keyed inventory and correlation

  func testRejectsLegacyConfigUpdateAndDecodesCorrelatedV2Update() throws {
    let legacy = #"{"type":"config_update","source":"/music","daemon":null,"ipod":null}"#
    XCTAssertThrowsError(
      try JSONDecoder().decode(DaemonEvent.self, from: Data(legacy.utf8)))

    let correlated =
      #"{"type":"config_update","source":"/music","daemon":null,"ipod":null,"config_revision":7,"acknowledged_request_id":"req-config"}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(correlated.utf8))
    guard case .configUpdate(_, _, _, let revision, let requestID) = event else {
      return XCTFail("expected configUpdate")
    }
    XCTAssertEqual(revision, 7)
    XCTAssertEqual(requestID, "req-config")
  }

  func testConfigUpdateRequiresNullablePayloadFieldsToBePresent() throws {
    let missingFieldFixtures = [
      #"{"type":"config_update","daemon":null,"ipod":null,"config_revision":7}"#,
      #"{"type":"config_update","source":null,"ipod":null,"config_revision":7}"#,
      #"{"type":"config_update","source":null,"daemon":null,"config_revision":7}"#,
    ]

    for fixture in missingFieldFixtures {
      XCTAssertThrowsError(
        try JSONDecoder().decode(DaemonEvent.self, from: Data(fixture.utf8)),
        "expected config_update missing a required nullable field to be rejected: \(fixture)")
    }

    let explicitNulls =
      #"{"type":"config_update","source":null,"daemon":null,"ipod":null,"config_revision":7}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(explicitNulls.utf8))
    guard case .configUpdate(let source, let daemon, let ipod, let revision, _) = event else {
      return XCTFail("expected configUpdate")
    }
    XCTAssertNil(source)
    XCTAssertNil(daemon)
    XCTAssertNil(ipod)
    XCTAssertEqual(revision, 7)
  }

  func testDecodesTwoDeviceInventorySnapshotRoundTrip() throws {
    let json =
      #"{"type":"device_inventory_snapshot","revision":9,"devices":[{"identity":{"serial":"RAW-A","model_label":"iPod Classic","name":"A"},"configured":true,"connected":true,"mount":"/Volumes/A","phase":"syncing","session_id":42,"storage":{"total_bytes":160000000000,"free_bytes":100000000000},"synced_count":12,"library_count":20,"latest_successful_sync":null,"latest_attempt":null,"last_terminal_error":null,"selection_revision":3,"settings_revision":4,"subscriptions_revision":5},{"identity":{"serial":"raw-B","model_label":"iPod Classic","name":"B"},"configured":false,"connected":true,"mount":"/Volumes/B","phase":"unconfigured","session_id":null,"storage":null,"synced_count":0,"library_count":null,"latest_successful_sync":null,"latest_attempt":null,"last_terminal_error":null,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0}]}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .deviceInventorySnapshot(let snapshot) = event else {
      return XCTFail("expected deviceInventorySnapshot")
    }
    XCTAssertEqual(snapshot.revision, 9)
    XCTAssertEqual(snapshot.devices.map(\.identity.serial), ["RAW-A", "raw-B"])
    XCTAssertEqual(snapshot.devices[0].phase, .syncing)
    XCTAssertEqual(snapshot.devices[0].sessionID, 42)
    XCTAssertEqual(snapshot.devices[1].phase, .unconfigured)
  }

  func testEveryNewDeviceMutationEncodesSerialAndRequestID() throws {
    let commands: [DaemonCommand] = [
      .forgetIpod(serial: "RAW-A", requestID: "req-1"),
      .triggerSync(source: .manual, serial: "RAW-A", requestID: "req-2"),
      .cancelSync(serial: "RAW-A", requestID: "req-3"),
      .pause(serial: "RAW-A", requestID: "req-4"),
      .decidePrompt(id: 7, choice: 1, serial: "RAW-A", requestID: "req-5"),
      .backfillRockbox(serial: "RAW-A", requestID: "req-6"),
      .replaceLibrary(serial: "RAW-A", requestID: "req-7"),
      .saveDeviceConfig(
        serial: "RAW-A", selection: nil, subscriptions: nil,
        settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
        requestID: "req-8"),
    ]

    for command in commands {
      let data = try JSONEncoder().encode(command)
      let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
      XCTAssertEqual(object["serial"] as? String, "RAW-A", "command: \(object)")
      XCTAssertNotNil(object["request_id"] as? String, "command: \(object)")
    }
  }

  func testDeviceActionCommandPinsRequestedSerial() throws {
    let commands = [
      DeviceActionCommand.sync(serial: "RAW-B", requestID: "sync"),
      DeviceActionCommand.cancel(serial: "RAW-B", requestID: "cancel"),
      DeviceActionCommand.pause(serial: "RAW-B", requestID: "pause"),
      DeviceActionCommand.forget(serial: "RAW-B", requestID: "forget"),
      DeviceActionCommand.replaceLibrary(serial: "RAW-B", requestID: "replace"),
      DeviceActionCommand.backfillRockbox(serial: "RAW-B", requestID: "backfill"),
    ]

    for command in commands {
      let data = try JSONEncoder().encode(command)
      let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
      XCTAssertEqual(object["serial"] as? String, "RAW-B")
      XCTAssertNotNil(object["request_id"] as? String)
    }
  }

  func testSaveConfigEncodesExactRequestCorrelation() throws {
    let command = DaemonCommand.saveConfig(
      source: "/music", daemon: nil, ipod: nil, requestID: "req-config")
    let data = try JSONEncoder().encode(command)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    XCTAssertEqual(object["type"] as? String, "save_config")
    XCTAssertEqual(object["source"] as? String, "/music")
    XCTAssertEqual(object["request_id"] as? String, "req-config")
  }

  func testSyncEventEchoesSerialAndSessionID() throws {
    let json =
      #"{"type":"sync_event","line":"{\"type\":\"track_done\"}","serial":"RAW-A","session_id":42}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .syncEvent(let line, let serial, let sessionID) = event else {
      return XCTFail("expected syncEvent")
    }
    XCTAssertEqual(line, #"{"type":"track_done"}"#)
    XCTAssertEqual(serial, "RAW-A")
    XCTAssertEqual(sessionID, 42)
  }

  func testSyncEventWithoutSessionIDIsRejected() throws {
    let json = #"{"type":"sync_event","line":"{\"type\":\"track_done\"}","serial":"RAW-A"}"#
    XCTAssertThrowsError(
      try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8)))
  }

  func testSyncRejectionEchoesRequestAndSerial() throws {
    let json =
      #"{"type":"sync_rejected","reason":"already_syncing","serial":"RAW-A","acknowledged_request_id":"req-sync"}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))
    guard case .syncRejected(let reason, let serial, let requestID) = event else {
      return XCTFail("expected syncRejected")
    }
    XCTAssertEqual(reason, "already_syncing")
    XCTAssertEqual(serial, "RAW-A")
    XCTAssertEqual(requestID, "req-sync")
  }

  func testV2RepliesRejectMissingRequiredIdentityOrCorrelationFields() {
    let legacyPayloads = [
      #"{"type":"config_update","source":null,"daemon":null,"ipod":null}"#,
      #"{"type":"history_update","entries":[]}"#,
      #"{"type":"sync_rejected","reason":"already_syncing"}"#,
      #"{"type":"sync_event","line":"{\"type\":\"track_done\"}"}"#,
      #"{"type":"selection_preview","selected_tracks":1,"selected_bytes":1,"adds":1,"removes":0}"#,
      #"{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":[]}"#,
      #"{"type":"device_config_update","serial":"RAW-A","selection":{"mode":"all","rules":[]},"subscriptions":{"playlists":[]},"settings":{"auto_sync":true,"rockbox_compat":false}}"#,
      #"{"type":"device_preview","serial":"RAW-A","selected_tracks":1,"selected_bytes":1,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null}"#,
      #"{"type":"resolved_tracks","tracks":[]}"#,
    ]

    for payload in legacyPayloads {
      XCTAssertThrowsError(
        try JSONDecoder().decode(DaemonEvent.self, from: Data(payload.utf8)),
        "legacy payload unexpectedly decoded: \(payload)")
    }
  }

  func testV2HistoryEntryRejectsMissingSerial() {
    let legacy =
      #"{"timestamp":"2026-07-17T10:00:00Z","duration_secs":120,"trigger":"manual","outcome":"ok"}"#
    XCTAssertThrowsError(
      try JSONDecoder().decode(HistoryEntry.self, from: Data(legacy.utf8)))
  }

  // MARK: - Protocol 2.0.0 source recovery

  func testRetrySourceMountEncodesRequiredFieldsWithoutDeviceIdentity() throws {
    for allowUI in [true, false] {
      let data = try JSONEncoder().encode(
        DaemonCommand.retrySourceMount(allowUI: allowUI, requestID: "req-123"))
      let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

      XCTAssertEqual(object["type"] as? String, "retry_source_mount")
      XCTAssertEqual(object["allow_ui"] as? Bool, allowUI)
      XCTAssertEqual(object["request_id"] as? String, "req-123")
      XCTAssertNil(object["serial"])
      XCTAssertEqual(object.count, 3)
    }
  }

  func testDecodesAvailableSourceAvailabilityWithResolvedRootAndCorrelation() throws {
    let json =
      #"{"type":"source_availability","state":"available","source_root":"/Volumes/data-1/media/music","acknowledged_request_id":"req-123"}"#
    let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))

    guard case .sourceAvailability(let availability) = event else {
      return XCTFail("expected sourceAvailability")
    }
    XCTAssertEqual(availability.state, .available)
    XCTAssertEqual(availability.sourceRoot, "/Volumes/data-1/media/music")
    XCTAssertEqual(availability.acknowledgedRequestID, "req-123")
  }

  func testDecodesUncorrelatedSourceAvailabilityLifecycleBroadcast() throws {
    for state in ["remounting", "auth_required", "unavailable"] {
      let json = #"{"type":"source_availability","state":"\#(state)"}"#
      let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(json.utf8))

      guard case .sourceAvailability(let availability) = event else {
        return XCTFail("expected sourceAvailability for \(state)")
      }
      XCTAssertEqual(availability.state.rawValue, state)
      XCTAssertNil(availability.sourceRoot)
      XCTAssertNil(availability.acknowledgedRequestID)
    }
  }

  func testSourceAvailabilityRejectsMissingOrInvalidStateAndRootShapes() {
    let invalidPayloads = [
      #"{"type":"source_availability"}"#,
      #"{"type":"source_availability","state":"unknown"}"#,
      #"{"type":"source_availability","state":"available"}"#,
      #"{"type":"source_availability","state":"available","source_root":null}"#,
      #"{"type":"source_availability","state":"auth_required","source_root":"/music"}"#,
      #"{"type":"source_availability","state":"unavailable","source_root":null}"#,
      #"{"type":"source_availability","state":"remounting","source_root":"/music"}"#,
    ]

    for payload in invalidPayloads {
      XCTAssertThrowsError(
        try JSONDecoder().decode(DaemonEvent.self, from: Data(payload.utf8)),
        "invalid payload unexpectedly decoded: \(payload)")
    }
  }
}
