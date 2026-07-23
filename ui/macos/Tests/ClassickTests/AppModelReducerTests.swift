import XCTest

@testable import Classick

@MainActor
final class AppModelReducerTests: XCTestCase {
  func testCorrelatedDeviceDropClearsPendingAndPublishesDeliveryOutcome() {
    let model = AppModel()
    seedDevices(["A"], in: model)
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "My iPod")
    model.markLibraryDropAdding(requestID: requestID, target: target)

    model.apply(
      .deviceSelectionAdded(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(), serial: "A",
          matchedTracks: 1, missingTracks: 0, selectionChanged: true, selectionRevision: 2,
          selection: .init(mode: .include, rules: [.artist(name: "Birdy")]),
          delivery: .addedForNextSync)))

    XCTAssertFalse(model.isLibraryDropAdding(target: target))
    XCTAssertEqual(model.dropOutcome, .addedForNextSync(serial: "A"))
  }

  func testStaleDeviceDropDoesNotOverwriteCanonicalStateOrClearPending() {
    let model = AppModel()
    seedDevices(["A"], in: model)
    model.apply(
      .deviceSelectionAdded(
        .init(
          acknowledgedRequestID: "unrelated", serial: "A", matchedTracks: 1, missingTracks: 0,
          selectionChanged: true, selectionRevision: 4,
          selection: .init(mode: .include, rules: [.genre(name: "Pop")]),
          delivery: .alreadyPresent)))
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000002")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "A")
    model.markLibraryDropAdding(requestID: requestID, target: target)

    model.apply(
      .deviceSelectionAdded(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(), serial: "A",
          matchedTracks: 1, missingTracks: 0, selectionChanged: true, selectionRevision: 3,
          selection: .init(mode: .include, rules: [.artist(name: "Birdy")]),
          delivery: .addedAndSyncing)))

    XCTAssertTrue(model.isLibraryDropAdding(target: target))
    XCTAssertNil(model.dropOutcome)
    XCTAssertEqual(model.devices["A"]?.selectionRevision, 4)
    XCTAssertEqual(
      model.devices["A"]?.config?.selection,
      SelectionState(mode: .include, rules: [.genre(name: "Pop")]))
  }

  func testLibraryMutationRejectionRequiresExactRequestAndTarget() {
    let model = AppModel()
    seedDevices(["A", "B"], in: model)
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000003")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "My iPod")
    model.markLibraryDropAdding(requestID: requestID, target: target)

    model.apply(
      .libraryMutationRejected(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(),
          target: .deviceSelection(serial: "B"), code: "invalid_rules", message: "Wrong target")))
    model.apply(
      .libraryMutationRejected(
        .init(
          acknowledgedRequestID: "wrong", target: .deviceSelection(serial: "A"),
          code: "invalid_rules", message: "Wrong request")))
    XCTAssertTrue(model.isLibraryDropAdding(target: target))
    XCTAssertNil(model.dropOutcome)

    model.apply(
      .libraryMutationRejected(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(),
          target: .deviceSelection(serial: "A"), code: "invalid_rules", message: "No matches")))
    XCTAssertFalse(model.isLibraryDropAdding(target: target))
    XCTAssertEqual(model.dropOutcome, .rejected(target: target, message: "No matches"))
  }

  func testPlaylistDropRequiresNondecreasingRevisionBeforeCompleting() {
    let model = AppModel()
    model.apply(
      .playlistsUpdate(
        [
          PlaylistSummary(slug: "mix", name: "Mix", kind: .manual, tracks: 1, bytes: 10, error: nil)
        ],
        playlistRevision: 5, acknowledgedRequestID: nil))
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000004")!
    let target = LibraryDropTarget.manualPlaylist(slug: "mix", displayName: "Mix")
    model.markLibraryDropAdding(requestID: requestID, target: target)

    model.apply(
      .playlistSelectionAppended(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(), slug: "mix",
          appendedTracks: 1, playlistRevision: 4,
          playlist: .init(slug: "mix", name: "Mix", tracks: ["old.flac"]))))
    XCTAssertTrue(model.isLibraryDropAdding(target: target))
    XCTAssertNil(model.dropOutcome)
    XCTAssertEqual(model.playlistRevision, 5)

    model.apply(
      .playlistSelectionAppended(
        .init(
          acknowledgedRequestID: requestID.uuidString.lowercased(), slug: "mix",
          appendedTracks: 2, playlistRevision: 5,
          playlist: .init(slug: "mix", name: "Mix", tracks: ["a.flac", "b.flac"]))))
    XCTAssertFalse(model.isLibraryDropAdding(target: target))
    XCTAssertEqual(model.dropOutcome, .appended(slug: "mix", count: 2))
    XCTAssertEqual(model.playlistDetail?.tracks, ["a.flac", "b.flac"])
    XCTAssertEqual(
      model.playlistAcknowledgedRequestID, requestID.uuidString.lowercased())
  }

  func testPersistedDropAcknowledgementIsRecordedOnceOnlyForCorrelatedTerminalEvent() {
    let model = AppModel()
    seedDevices(["A"], in: model)
    let requestID = UUID(uuidString: "00000000-0000-0000-0000-000000000005")!
    let target = LibraryDropTarget.device(serial: "A", displayName: "A")
    model.markLibraryDropAdding(requestID: requestID, target: target)
    let reply = DeviceSelectionAddedInfo(
      acknowledgedRequestID: requestID.uuidString.lowercased(), serial: "A",
      matchedTracks: 1, missingTracks: 0, selectionChanged: true, selectionRevision: 1,
      selection: .init(mode: .include, rules: [.artist(name: "Birdy")]),
      delivery: .addedAndSyncing)

    model.apply(.deviceSelectionAdded(reply))
    model.apply(.deviceSelectionAdded(reply))

    XCTAssertEqual(model.persistedDropAcknowledgements, [requestID.uuidString.lowercased()])
  }

  func testLocalDropRejectionClearsOnlyExactPendingWithoutPersistedAcknowledgement() {
    let model = AppModel()
    let firstID = UUID(uuidString: "00000000-0000-0000-0000-000000000006")!
    let secondID = UUID(uuidString: "00000000-0000-0000-0000-000000000007")!
    let first = LibraryDropTarget.device(serial: "A", displayName: "A")
    let second = LibraryDropTarget.manualPlaylist(slug: "mix", displayName: "Mix")
    model.markLibraryDropAdding(requestID: firstID, target: first)
    model.markLibraryDropAdding(requestID: secondID, target: second)

    model.rejectLibraryDropLocally(requestID: firstID, target: second, message: "Wrong target")
    XCTAssertTrue(model.isLibraryDropAdding(target: first))
    XCTAssertTrue(model.isLibraryDropAdding(target: second))

    model.rejectLibraryDropLocally(requestID: firstID, target: first, message: "Not sent")
    XCTAssertFalse(model.isLibraryDropAdding(target: first))
    XCTAssertTrue(model.isLibraryDropAdding(target: second))
    XCTAssertEqual(model.dropOutcome, .rejected(target: first, message: "Not sent"))
    XCTAssertTrue(model.persistedDropAcknowledgements.isEmpty)
  }

  func testDropOutcomeAccessibleMessagesAreExact() {
    XCTAssertEqual(
      DropOutcome.adding(target: .device(serial: "A", displayName: "A")).accessibleMessage,
      "Adding…")
    XCTAssertEqual(DropOutcome.addedAndSyncing(serial: "A").accessibleMessage, "Added and syncing")
    XCTAssertEqual(
      DropOutcome.addedForNextSync(serial: "A").accessibleMessage, "Added for next sync")
    XCTAssertEqual(
      DropOutcome.alreadyPresent(serial: "A").accessibleMessage, "Already on this iPod")
    XCTAssertEqual(
      DropOutcome.appended(slug: "mix", count: 2).accessibleMessage, "Appended 2 songs")
    XCTAssertEqual(
      DropOutcome.rejected(
        target: .manualPlaylist(slug: "mix", displayName: "Mix"), message: "No matches"
      ).accessibleMessage,
      "No matches")
  }
  /// Regression: the first-run setup wizard must NOT reset an enabled Rockbox
  /// compatibility toggle back to off (SaveConfig replaces the whole daemon
  /// blob, so the wizard has to carry the existing value through).
  func testSetupWizardPreservesRockboxCompat() {
    let preserved = AppDelegate.setupDaemonSettings(autoSync: true, preservingRockboxCompat: true)
    XCTAssertTrue(preserved.rockboxCompat, "wizard must preserve an enabled Rockbox toggle")
    let off = AppDelegate.setupDaemonSettings(autoSync: true, preservingRockboxCompat: false)
    XCTAssertFalse(off.rockboxCompat)
  }

  /// Regression (protocol 1.5.0): finishing setup builds a fresh
  /// `IpodIdentity` from the connected device, and SaveConfig replaces the
  /// whole `ipod` blob — so re-running setup against the *same* paired
  /// device must carry `custom_selection` through, mirroring
  /// `testSetupWizardPreservesRockboxCompat` for `rockboxCompat`.
  func testSetupWizardPreservesCustomSelection() {
    let device = DeviceState(
      serial: "0xA", model: "iPod Classic (3rd gen)", name: "iPod", drive: "/Volumes/IPOD")
    let preserved = AppDelegate.setupIpodIdentity(device: device, preservingCustomSelection: true)
    XCTAssertEqual(
      preserved?.customSelection, true, "wizard must preserve an enabled custom-selection toggle")
    let off = AppDelegate.setupIpodIdentity(device: device, preservingCustomSelection: false)
    XCTAssertEqual(off?.customSelection, false)
    XCTAssertNil(
      AppDelegate.setupIpodIdentity(device: nil, preservingCustomSelection: true),
      "no device -> no identity to save")
  }

  func testFinishSyncEventPopulatesSkippedForSpaceArtworkAndDbRestoredState() {
    let m = AppModel()
    seedDevice("0xA", phase: .syncing, sessionID: 1, in: m)
    m.apply(
      .syncEvent(
        line:
          #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#,
        serial: "0xA", sessionID: 1))
    XCTAssertEqual(
      m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
    XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
    XCTAssertTrue(m.lastRunDbRestored)
  }

  /// Regression: a library-scan's `finish` event never carries
  /// `skipped_for_space`/`artwork`/`db_restored` — those fields are
  /// sync-only. A scan finishing right after a real sync must not clobber
  /// that sync's rollup back to nil/nil/false.
  func testScanFinishDoesNotClobberPriorSyncRollup() {
    let m = AppModel()
    seedDevice("0xA", phase: .syncing, sessionID: 1, in: m)
    m.apply(
      .syncEvent(
        line:
          #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#,
        serial: "0xA", sessionID: 1))
    XCTAssertEqual(
      m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
    XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
    XCTAssertTrue(m.lastRunDbRestored)

    m.apply(
      .statusUpdate(
        .init(state: .scanning, configured: true, ipodConnected: true, lastSync: nil, storage: nil))
    )
    m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#, serial: nil, sessionID: 2))

    XCTAssertEqual(
      m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
    XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
    XCTAssertTrue(m.lastRunDbRestored)
  }

  func testDeviceConnectThenDisconnect() {
    let m = AppModel()
    m.apply(
      .deviceConnected(
        serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD",
        name: "Michael’s iPod"))
    XCTAssertEqual(m.device?.name, "Michael’s iPod")
    m.apply(.deviceDisconnected(serial: "0xA"))
    XCTAssertNil(m.device)
    XCTAssertEqual(m.phase, .noDevice)
  }

  func testSyncProgressFromForwardedEvents() {
    let m = AppModel()
    seedDevice("0xA", phase: .syncing, sessionID: 1, in: m)
    m.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":34,"total":120,"label":"Karma Police"}"#,
        serial: "0xA", sessionID: 1))
    guard case .syncing(let cur, let total, let label, _) = m.phase else { return XCTFail() }
    XCTAssertEqual(cur, 34)
    XCTAssertEqual(total, 120)
    XCTAssertEqual(label, "Karma Police")
    m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#, serial: "0xA", sessionID: 1))
    guard case .syncing = m.phase else { return XCTFail("finish must await the terminal snapshot") }
  }

  func testRejectionBecomesError() {
    let m = AppModel()
    m.apply(.deviceConnected(serial: "0xA", modelLabel: "x", drive: "/Volumes/IPOD", name: nil))
    m.apply(
      .syncRejected(reason: "not_configured", serial: "0xA", acknowledgedRequestID: "request-a"))
    if case .error = m.phase {} else { XCTFail("expected error phase") }
  }

  func testNeedsFirstRunSetupOnlyAfterEmptyConfigSeen() {
    // The first-run auto-present trigger must stay false until the daemon's
    // get_config reply lands (avoids firing during the startup race), then
    // become true only when no music-library source is configured.
    let m = AppModel()
    XCTAssertFalse(m.needsFirstRunSetup, "unknown before the config reply")

    m.apply(
      .configUpdate(
        source: nil, daemon: nil, ipod: nil, configRevision: 1, acknowledgedRequestID: "request-a"))
    XCTAssertTrue(m.needsFirstRunSetup, "empty config == never set up")

    m.apply(
      .configUpdate(
        source: "/music", daemon: nil, ipod: nil, configRevision: 1,
        acknowledgedRequestID: "request-a"))
    XCTAssertFalse(m.needsFirstRunSetup, "source set == setup completed")
  }

  func testPausedInventorySnapshotEntersPausedPhase() {
    let m = AppModel()
    seedDevice("0xA", phase: .paused, syncedCount: 50, libraryCount: 1500, in: m)
    guard case .paused = m.phase else { return XCTFail("expected .paused") }
  }

  func testPausedPhaseSurvivesTrailingIdleStatus() {
    let m = AppModel()
    seedDevice("0xA", phase: .paused, syncedCount: 84, libraryCount: 1381, in: m)
    guard case .paused = m.phase else { return XCTFail("expected .paused after snapshot") }
    // Subprocess exits → daemon broadcasts idle. Paused MUST persist and
    // refresh its counts, not revert to plain idle.
    m.apply(
      .statusUpdate(
        .init(
          state: .idle, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
          syncedCount: 84, libraryCount: 1381)))
    guard case .paused(let synced, let total) = m.phase else {
      return XCTFail("paused state lost after trailing idle status")
    }
    XCTAssertEqual(synced, 84)
    XCTAssertEqual(total, 1381)
  }

  func testResumeFromPausedEntersSyncing() {
    let m = AppModel()
    seedDevice("0xA", phase: .paused, syncedCount: 84, libraryCount: 1381, in: m)
    guard case .paused = m.phase else { return XCTFail("expected .paused") }
    // Resume sends TriggerSync; the next authoritative snapshot starts a new session.
    m.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 2,
          devices: [
            deviceSnapshot(
              "0xA", phase: .syncing, sessionID: 2, syncedCount: 84, libraryCount: 1381)
          ])))
    guard case .syncing = m.phase else { return XCTFail("expected .syncing after resume") }
  }

  func testPausedClearsOnDeviceDisconnect() {
    let m = AppModel()
    seedDevice("0xA", phase: .paused, syncedCount: 84, libraryCount: 1381, in: m)
    guard case .paused = m.phase else { return XCTFail("expected .paused") }
    m.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 2,
          devices: [
            deviceSnapshot(
              "0xA", connected: false, phase: .disconnected, syncedCount: 84,
              libraryCount: 1381)
          ])))
    guard case .noDevice = m.phase else { return XCTFail("expected .noDevice after unplug") }
  }

  func testSyncingPhaseCarriesEta() {
    let m = AppModel()
    seedDevice("0xA", phase: .syncing, sessionID: 1, in: m)
    m.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#,
        serial: "0xA", sessionID: 1))
    if case .syncing(let current, let total, _, let eta) = m.phase {
      XCTAssertEqual(current, 5)
      XCTAssertEqual(total, 10)
      XCTAssertEqual(eta, 42)
    } else {
      XCTFail("expected syncing, got \(m.phase)")
    }
  }

  // MARK: - Task 17: Replace Library, selection toggle, device-row rollup lines

  /// Typed-confirmation gate: only an exact, case-sensitive match of the
  /// device name arms the Replace Library confirm button.
  func testReplaceConfirmationArmsOnlyOnExactName() {
    XCTAssertTrue(
      ReplaceConfirmation.isArmed(input: "Michael's iPod", deviceName: "Michael's iPod"))
    XCTAssertFalse(ReplaceConfirmation.isArmed(input: "", deviceName: "Michael's iPod"))
    XCTAssertFalse(
      ReplaceConfirmation.isArmed(input: "michael's ipod", deviceName: "Michael's iPod"),
      "must be case-sensitive")
    XCTAssertFalse(
      ReplaceConfirmation.isArmed(input: "Michael's iPo", deviceName: "Michael's iPod"),
      "must be an exact match, not a prefix")
    XCTAssertFalse(
      ReplaceConfirmation.isArmed(input: "Michael's iPod ", deviceName: "Michael's iPod"),
      "trailing whitespace must not arm")
    // An empty device name (shouldn't happen in practice, but guards
    // against an empty input trivially "matching" an empty name).
    XCTAssertFalse(ReplaceConfirmation.isArmed(input: "", deviceName: ""))
  }

  /// Bytes -> GB formatting is fixed at one decimal place, not
  /// `ByteCountFormatter`'s auto-unit/auto-precision behavior.
  func testSkippedForSpaceLabelFormatting() {
    XCTAssertEqual(DeviceRowFormatting.gbString(9_876_543_210), "9.9 GB")
    XCTAssertEqual(DeviceRowFormatting.gbString(1_000_000_000), "1.0 GB")
    XCTAssertEqual(DeviceRowFormatting.gbString(0), "0.0 GB")

    let line = DeviceRowFormatting.skippedForSpaceLine(
      syncedSummary: "1317 of 1500",
      skipped: SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
    XCTAssertEqual(line, "Synced 1317 of 1500 — 14 albums didn't fit (9.9 GB)")

    XCTAssertNil(
      DeviceRowFormatting.skippedForSpaceLine(syncedSummary: "1500 of 1500", skipped: nil))
    XCTAssertNil(
      DeviceRowFormatting.skippedForSpaceLine(
        syncedSummary: "1500 of 1500",
        skipped: SkippedForSpace(albums: 0, tracks: 0, bytes: 0)),
      "nothing skipped this run -> no line")
  }

  /// Artwork-missing line only shows when the run's rollup indicates a
  /// shortfall, and reports the shortfall count (falling back to
  /// `failedSources` if the counts don't line up).
  func testArtworkMissingLineVisibility() {
    XCTAssertNil(DeviceRowFormatting.artworkMissingLine(nil))
    XCTAssertNil(
      DeviceRowFormatting.artworkMissingLine(
        ArtworkSummary(embedded: 42, eligible: 42, failedSources: 0)),
      "everything embedded, nothing failed -> no line")

    XCTAssertEqual(
      DeviceRowFormatting.artworkMissingLine(
        ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2)),
      "Art missing for 2 tracks")
    XCTAssertEqual(
      DeviceRowFormatting.artworkMissingLine(
        ArtworkSummary(embedded: 1, eligible: 2, failedSources: 0)),
      "Art missing for 1 track", "singular for a shortfall of exactly one")
    XCTAssertEqual(
      DeviceRowFormatting.artworkMissingLine(
        ArtworkSummary(embedded: 42, eligible: 42, failedSources: 3)),
      "Art missing for 3 tracks", "failedSources > 0 with no embed shortfall still surfaces")
  }

  /// The Selection picker's save path (Task 17): SaveConfig replaces the
  /// whole `ipod` blob, so flipping `customSelection` must carry the
  /// existing serial/model_label/name through untouched — mirrors
  /// `testSetupWizardPreservesCustomSelection` for the setup wizard's own
  /// identity-construction site.
  func testSaveIpodSelectionPreservesIdentityFields() {
    let existing = IpodIdentity(
      serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: "Michael's iPod",
      customSelection: false)
    let flipped = AppDelegate.withCustomSelection(true, from: existing)
    XCTAssertEqual(flipped?.serial, "0xA")
    XCTAssertEqual(flipped?.modelLabel, "iPod Classic (3rd gen)")
    XCTAssertEqual(flipped?.name, "Michael's iPod")
    XCTAssertEqual(flipped?.customSelection, true)

    XCTAssertNil(
      AppDelegate.withCustomSelection(true, from: nil),
      "no persisted identity yet -> nothing to save")
  }

  @MainActor
  func testHistoryRetained() {
    let m = AppModel()
    let e = HistoryEntry(
      serial: "0xA", timestamp: "2026-07-14T10:00:00Z", durationSecs: 5,
      trigger: "manual", outcome: "ok")
    m.apply(.historyUpdate(entries: [e], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(m.history.count, 1)
    XCTAssertEqual(m.history.first?.trigger, "manual")
  }

  // MARK: - Protocol 1.6.0: playlists, per-device config, device preview

  func testPlaylistsUpdateReplacesList() {
    let m = AppModel()
    m.apply(
      .playlistsUpdate(
        [
          PlaylistSummary(
            slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil)
        ], playlistRevision: 0, acknowledgedRequestID: nil))
    XCTAssertEqual(m.playlists.map(\.slug), ["gym"])

    m.apply(
      .playlistsUpdate(
        [
          PlaylistSummary(
            slug: "chill", name: "Chill", kind: .smart, tracks: 5, bytes: 100, error: nil)
        ], playlistRevision: 0, acknowledgedRequestID: nil))
    XCTAssertEqual(
      m.playlists.map(\.slug), ["chill"], "playlists_update must replace the list, not append")
  }

  func testPlaylistDetailStoresMostRecentReply() {
    let m = AppModel()
    m.apply(
      .playlistDetail(
        PlaylistDetail(
          slug: "gym", name: "Gym", kind: .manual, tracks: ["a.flac"], rules: nil, error: nil,
          playlistRevision: 0, acknowledgedRequestID: "request-a")))
    XCTAssertEqual(m.playlistDetail?.slug, "gym")
    XCTAssertEqual(m.playlistDetail?.tracks, ["a.flac"])
  }

  func testDevicePreviewWithoutMatchingLocalRequestIsDropped() {
    let m = AppModel()
    seedDevices(["0xA"], in: m)
    m.apply(
      .devicePreview(
        DevicePreview(
          serial: "0xA",
          selectedTracks: 1, selectedBytes: 1, playlistExtraTracks: 0, playlistExtraBytes: 0,
          projectedFreeBytes: nil, unresolvedSubscriptions: nil,
          acknowledgedRequestID: "request-a")))
    XCTAssertNil(m.deviceConfigs["0xA"]?.preview)
  }

  func testHelloClearsActionableDeviceProjectionUntilFreshInventory() {
    let m = AppModel()
    seedDevices(["0xA"], in: m)
    m.selectedDestination = .device(serial: "0xA", page: .music)
    m.willRequestDeviceConfig(serial: "0xA", requestID: "config", intent: .read)
    m.apply(
      .deviceConfigUpdate(
        serial: "0xA", selection: .init(mode: .all, rules: []),
        subscriptions: .init(playlists: []),
        settings: .init(autoSync: true, rockboxCompat: false),
        selectionRevision: 0, settingsRevision: 0, subscriptionsRevision: 0,
        acknowledgedRequestID: "config"))
    m.willRequestDevicePreview(serial: "0xA", requestID: "preview")
    m.apply(
      .devicePreview(
        .init(
          serial: "0xA", selectedTracks: 1, selectedBytes: 1, playlistExtraTracks: 0,
          playlistExtraBytes: 0, projectedFreeBytes: nil, unresolvedSubscriptions: nil,
          acknowledgedRequestID: "preview")))

    m.apply(.hello(protocolVersion: "2.0.0", coreVersion: "2.0.0"))

    XCTAssertTrue(m.devices.isEmpty)
    XCTAssertTrue(m.deviceConfigs.isEmpty)
    XCTAssertNil(m.device)
    XCTAssertNil(m.focusedDeviceSerial)
    XCTAssertEqual(m.phase, .noDevice)
    XCTAssertFalse(m.deviceActionsAvailable)
    XCTAssertNil(
      MenuContentLogic.actionTarget(focusedSerial: m.focusedDeviceSerial, devices: m.devices))
  }

  func testSeriallessSyncEventOutsideScanIsDropped() {
    let m = AppModel()

    m.apply(
      .syncEvent(
        line: #"{"type":"track_start","current":5,"total":10,"label":"Not a scan"}"#,
        serial: nil, sessionID: 7))

    XCTAssertEqual(m.phase, .noDevice)
  }

  func testDestinationForDeviceRowClickSelectsMusicPage() {
    XCTAssertEqual(
      SidebarDestination.destinationForDeviceRowClick(serial: "0xA"),
      .device(serial: "0xA", page: .music))
  }

  // MARK: - Task 3: sidebar "+ New Playlist" selection flow

  /// The "+" button's flow (plan Task 3): emit `.savePlaylist` for a
  /// manual "New Playlist", snapshotting the slugs that existed before the
  /// request, then once the next `playlists_update` contains a slug that
  /// wasn't in that snapshot, selection moves to `.playlist(that slug)`.
  func testDestinationForNewlyCreatedPlaylistSelectsTheNewSlug() {
    let prior: Set<String> = ["gym", "chill"]
    let updated = [
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
      PlaylistSummary(
        slug: "chill", name: "Chill", kind: .smart, tracks: 5, bytes: 100, error: nil),
      PlaylistSummary(
        slug: "new-playlist", name: "New Playlist", kind: .manual, tracks: 0, bytes: 0, error: nil),
    ]
    XCTAssertEqual(
      SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated),
      .playlist(slug: "new-playlist"))
  }

  /// An unrelated `playlists_update` (nothing new since the snapshot) must
  /// not steal selection — the caller drops the snapshot only once a match
  /// is found, so a `nil` result here means "keep waiting".
  func testDestinationForNewlyCreatedPlaylistReturnsNilWhenNothingNew() {
    let prior: Set<String> = ["gym"]
    let updated = [
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil)
    ]
    XCTAssertNil(
      SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated))
  }

  func testDestinationForNewlyCreatedPlaylistOnEmptyPriorSnapshot() {
    let updated = [
      PlaylistSummary(slug: "first", name: "First", kind: .manual, tracks: 0, bytes: 0, error: nil)
    ]
    XCTAssertEqual(
      SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: [], updated: updated),
      .playlist(slug: "first"))
  }

  /// Review finding #3: the daemon broadcasts `playlists_update` to every
  /// client and sorts alphabetically by slug, so a concurrently-created
  /// playlist from ANOTHER client with an alphabetically-earlier slug must
  /// not steal this client's selection. Among the new slugs, one that
  /// starts with the expected "new-playlist" prefix must win even if it
  /// sorts later than an unrelated new slug.
  func testDestinationForNewlyCreatedPlaylistPrefersExpectedPrefixOverEarlierSortingSlug() {
    let prior: Set<String> = ["gym"]
    let updated = [
      PlaylistSummary(
        slug: "another-clients-mix", name: "Another Client's Mix", kind: .manual, tracks: 0,
        bytes: 0, error: nil),
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
      PlaylistSummary(
        slug: "new-playlist", name: "New Playlist", kind: .manual, tracks: 0, bytes: 0, error: nil),
    ]
    XCTAssertEqual(
      SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated),
      .playlist(slug: "new-playlist"),
      "the expected-prefix slug must win even though 'another-clients-mix' sorts first")
  }

  /// When neither new slug carries the expected prefix (e.g. this
  /// heuristic can't disambiguate at all), fall back to the first new slug
  /// in `updated`'s order — a best-effort default, not a guarantee.
  func testDestinationForNewlyCreatedPlaylistFallsBackToFirstNewWhenNeitherIsPrefixed() {
    let prior: Set<String> = ["gym"]
    let updated = [
      PlaylistSummary(
        slug: "alpha-mix", name: "Alpha Mix", kind: .manual, tracks: 0, bytes: 0, error: nil),
      PlaylistSummary(
        slug: "beta-mix", name: "Beta Mix", kind: .manual, tracks: 0, bytes: 0, error: nil),
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
    ]
    XCTAssertEqual(
      SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated),
      .playlist(slug: "alpha-mix"))
  }

  // MARK: - Fix: sidebar bounded-wait for "+ New Playlist" (premature-clear regression)

  /// An unrelated interleaved `playlists_update` (nothing new yet, e.g.
  /// another client's own unrelated change) must not clear the pending
  /// snapshot before the real creation reply arrives — the caller keeps
  /// waiting (not yet at the bound) and a later matching update still
  /// selects the new playlist.
  func testShouldClearPendingNewPlaylistKeepsWaitingOnUnrelatedUpdateThenClearsOnMatch() {
    XCTAssertFalse(
      SidebarDestination.shouldClearPendingNewPlaylist(matched: false, revisionsElapsed: 1),
      "an unrelated update before the bound must not clear pending")
    XCTAssertTrue(
      SidebarDestination.shouldClearPendingNewPlaylist(matched: true, revisionsElapsed: 2),
      "a matching update must always clear pending, regardless of elapsed count")
  }

  /// Wedge-forever must remain impossible: once the bound is reached with
  /// no match, pending clears anyway (re-enabling the "+" button), even
  /// though no destination was selected.
  func testShouldClearPendingNewPlaylistClearsOnceBoundIsExceededWithNoMatch() {
    XCTAssertFalse(
      SidebarDestination.shouldClearPendingNewPlaylist(matched: false, revisionsElapsed: 1))
    XCTAssertFalse(
      SidebarDestination.shouldClearPendingNewPlaylist(matched: false, revisionsElapsed: 2))
    XCTAssertTrue(
      SidebarDestination.shouldClearPendingNewPlaylist(
        matched: false, revisionsElapsed: SidebarDestination.maxRevisionsToWaitForNewPlaylist),
      "must clear once the bound is reached, so the '+' button can never wedge disabled forever")
  }

  // MARK: - Task 3 review fix: playlists_update revision (finding #2)

  /// The sidebar's in-flight "+" guard must clear even when a
  /// `playlists_update` reply is content-identical to the prior one (e.g.
  /// the daemon's error path, which re-sends the unchanged list) — a plain
  /// `onChange(of: playlists)` wouldn't fire in that case since the
  /// Equatable value hasn't changed. `playlistsUpdateRevision` increments
  /// on every `playlists_update` event regardless of content so the
  /// sidebar has something to observe that always fires.
  func testPlaylistsUpdateRevisionIncrementsOnEveryEventEvenWhenContentIsUnchanged() {
    let m = AppModel()
    let list = [
      PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil)
    ]
    m.apply(.playlistsUpdate(list, playlistRevision: 0, acknowledgedRequestID: nil))
    XCTAssertEqual(m.playlistsUpdateRevision, 1)
    m.apply(.playlistsUpdate(list, playlistRevision: 0, acknowledgedRequestID: nil))
    XCTAssertEqual(
      m.playlistsUpdateRevision, 2, "revision must bump even when the list content is identical")
  }

  // MARK: - resolve_tracks / resolved_tracks (protocol 1.7.0 — Add Songs picker)

  /// Mirrors `testDevicePreviewAttachesToTheRequestedSerial`'s bookkeeping
  /// discipline: a request must be in flight for a reply to attach.
  func testResolvedTracksStoresResultTaggedWithSlugAndBumpsRevisionWhenRequestPending() {
    let m = AppModel()
    m.willRequestResolveTracks(slug: "gym")
    m.apply(
      .resolvedTracks(
        tracks: ["Artist/Album/01.flac", "B/02.flac"], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(m.latestResolvedTracks?.slug, "gym")
    XCTAssertEqual(m.latestResolvedTracks?.tracks, ["Artist/Album/01.flac", "B/02.flac"])
    XCTAssertEqual(m.resolvedTracksRevision, 1)
  }

  /// An empty reply is a valid outcome (no rules matched anything in the
  /// index) — must still bump the revision so the Add Songs sheet's
  /// `onChange` fires and stops showing "Adding…", not be treated as
  /// nothing having happened.
  func testResolvedTracksEmptyReplyStillBumpsRevision() {
    let m = AppModel()
    m.willRequestResolveTracks(slug: "gym")
    m.apply(.resolvedTracks(tracks: [], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(m.latestResolvedTracks?.slug, "gym")
    XCTAssertEqual(m.latestResolvedTracks?.tracks, [])
    XCTAssertEqual(m.resolvedTracksRevision, 1)
  }

  /// This older reducer path still uses its pending-slug queue to attach
  /// the acknowledged reply to an editor.
  func testResolvedTracksWithNoPendingRequestIsDropped() {
    let m = AppModel()
    m.apply(.resolvedTracks(tracks: ["a.flac"], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(m.resolvedTracksRevision, 0)
    XCTAssertNil(m.latestResolvedTracks)
  }

  /// Fix (resolve-reply correlation hardening): `pendingResolveTracks` is a
  /// FIFO queue keyed by slug — two interleaved requests from different
  /// playlist editors must each attach
  /// their reply to the right slug, in request order, not overwrite each
  /// other or cross-attach.
  func testResolvedTracksQueueAttachesInterleavedRepliesToTheRightSlugs() {
    let m = AppModel()
    m.willRequestResolveTracks(slug: "gym")
    m.willRequestResolveTracks(slug: "chill")

    m.apply(.resolvedTracks(tracks: ["gym1.flac"], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(
      m.latestResolvedTracks?.slug, "gym", "first reply must attach to the first (oldest) request")
    XCTAssertEqual(m.latestResolvedTracks?.tracks, ["gym1.flac"])

    m.apply(.resolvedTracks(tracks: ["chill1.flac"], acknowledgedRequestID: "request-a"))
    XCTAssertEqual(
      m.latestResolvedTracks?.slug, "chill", "second reply must attach to the second request")
    XCTAssertEqual(m.latestResolvedTracks?.tracks, ["chill1.flac"])
  }

  // MARK: - ManualPlaylistLogic.shouldConsumeResolvedTracks (editor-side correlation guard)

  func testShouldConsumeResolvedTracksAcceptsMatchingSlug() {
    let reply = ResolvedTracksReply(slug: "gym", tracks: ["a.flac"])
    XCTAssertTrue(ManualPlaylistLogic.shouldConsumeResolvedTracks(reply: reply, forSlug: "gym"))
  }

  /// The editor showing playlist "chill" must never consume a reply tagged
  /// for playlist "gym" — this is the actual regression fix: previously
  /// ANY bump while `isResolvingAdd` was true was consumed, regardless of
  /// which playlist actually requested it.
  func testShouldConsumeResolvedTracksRejectsMismatchedSlug() {
    let reply = ResolvedTracksReply(slug: "gym", tracks: ["a.flac"])
    XCTAssertFalse(ManualPlaylistLogic.shouldConsumeResolvedTracks(reply: reply, forSlug: "chill"))
  }

  func testSidebarInventorySortsByNameAndPreservesRememberedDevices() {
    let model = AppModel()
    model.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 1,
          devices: [
            DeviceSnapshotWire(
              identity: .init(serial: "B", modelLabel: "iPod Classic", name: "Zebra"),
              configured: false, connected: true, mount: "/Volumes/B", phase: .unconfigured,
              sessionID: nil, storage: nil, syncedCount: 0, libraryCount: nil,
              latestSuccessfulSync: nil, latestAttempt: nil, lastTerminalError: nil,
              selectionRevision: 0, settingsRevision: 0, subscriptionsRevision: 0),
            DeviceSnapshotWire(
              identity: .init(serial: "A", modelLabel: "iPod Classic", name: "Alpha"),
              configured: true, connected: false, mount: nil, phase: .disconnected,
              sessionID: nil, storage: nil, syncedCount: 12, libraryCount: 12,
              latestSuccessfulSync: nil, latestAttempt: nil, lastTerminalError: nil,
              selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1),
          ])))

    let rows = SidebarInventory.rows(from: model.devices)

    XCTAssertEqual(rows.map(\.serial), ["A", "B"])
    XCTAssertEqual(rows.map(\.connected), [false, true])
    XCTAssertEqual(rows.map(\.configured), [true, false])
  }

  func testMenuDoesNotChooseSyncTargetWhenTwoDevicesAreConnectedWithoutFocus() {
    let model = AppModel()
    seedDevices(["A", "B"], in: model)

    XCTAssertNil(model.focusedDeviceSerial)
    XCTAssertNil(
      MenuContentLogic.actionTarget(
        focusedSerial: model.focusedDeviceSerial, devices: model.devices))
  }

  // MARK: - Protocol 2.0.0 source recovery

  func testSourceAvailabilityFailurePreservesCachedLibraryAndCount() {
    let model = AppModel()
    let cached = LibraryInfo(
      sourceRoot: "/Volumes/data/media/music", scannedAtUnixSecs: 42,
      artists: [], genres: [], totalTracks: 1407, totalBytes: 99)
    model.apply(.libraryUpdate(cached))
    model.apply(
      .sourceAvailability(
        .init(
          state: .authRequired, sourceRoot: nil, acknowledgedRequestID: nil)))

    XCTAssertEqual(model.sourceAvailability?.state, .authRequired)
    XCTAssertEqual(model.library, cached)
    XCTAssertEqual(model.library?.totalTracks, 1407)
    XCTAssertTrue(model.sourceNeedsAttention)
    XCTAssertEqual(SourceRecoveryPresentation.attentionTitle, "Music share needs attention")
  }

  func testSourceRetryRequiresAttentionAndActiveApplication() {
    let model = AppModel()
    let inactive = UUID(), active = UUID(), duplicate = UUID()
    model.apply(
      .sourceAvailability(
        .init(state: .unavailable, sourceRoot: nil, acknowledgedRequestID: nil)))

    XCTAssertNil(
      model.prepareSourceMountRetry(isApplicationActive: false, requestID: inactive))
    XCTAssertFalse(model.sourceRetryPending)

    let command = model.prepareSourceMountRetry(
      isApplicationActive: true, requestID: active)
    guard case .retrySourceMount(let requestID, let allowUI) = command else {
      return XCTFail("expected retrySourceMount")
    }
    XCTAssertTrue(allowUI)
    XCTAssertEqual(requestID, active)
    XCTAssertTrue(model.sourceRetryPending)
    XCTAssertNil(
      model.prepareSourceMountRetry(isApplicationActive: true, requestID: duplicate),
      "a pending retry must coalesce duplicate clicks")
  }

  func testStaleSourceAvailabilityReplyCannotOverwriteCurrentRequest() {
    let model = AppModel()
    let current = UUID(), stale = UUID()
    model.apply(
      .sourceAvailability(
        .init(state: .authRequired, sourceRoot: nil, acknowledgedRequestID: nil)))
    _ = model.prepareSourceMountRetry(isApplicationActive: true, requestID: current)

    model.apply(
      .sourceAvailability(
        .init(
          state: .available, sourceRoot: "/Volumes/stale/media/music",
          acknowledgedRequestID: stale.uuidString)))

    XCTAssertEqual(model.sourceAvailability?.state, .authRequired)
    XCTAssertTrue(model.sourceRetryPending)
  }

  func testOtherClientSourceAvailabilityReplyIsAnAuthoritativeBroadcast() {
    let model = AppModel()
    model.apply(
      .sourceAvailability(
        .init(state: .remounting, sourceRoot: nil, acknowledgedRequestID: nil)))

    model.apply(
      .sourceAvailability(
        .init(
          state: .available, sourceRoot: "/Volumes/data-1/media/music",
          acknowledgedRequestID: "another-client")))

    XCTAssertEqual(model.sourceAvailability?.state, .available)
    XCTAssertEqual(model.sourceAvailability?.sourceRoot, "/Volumes/data-1/media/music")
  }

  func testMatchingAvailableReplyClearsAttentionAndReflectsReturnedRoot() {
    let model = AppModel()
    let requestID = UUID()
    let cached = LibraryInfo(
      sourceRoot: "/Volumes/data/media/music", scannedAtUnixSecs: 42,
      artists: [], genres: [], totalTracks: 1407, totalBytes: 99)
    model.apply(.libraryUpdate(cached))
    model.apply(
      .configUpdate(
        source: "/Volumes/data/media/music", daemon: nil, ipod: nil,
        configRevision: 1, acknowledgedRequestID: "config"))
    model.apply(
      .sourceAvailability(
        .init(state: .authRequired, sourceRoot: nil, acknowledgedRequestID: nil)))
    _ = model.prepareSourceMountRetry(isApplicationActive: true, requestID: requestID)

    model.apply(
      .sourceAvailability(
        .init(
          state: .available, sourceRoot: "/Volumes/data-1/media/music",
          acknowledgedRequestID: requestID.uuidString)))

    XCTAssertEqual(model.sourceAvailability?.state, .available)
    XCTAssertEqual(model.sourceAvailability?.sourceRoot, "/Volumes/data-1/media/music")
    XCTAssertEqual(model.library?.sourceRoot, "/Volumes/data-1/media/music")
    XCTAssertEqual(model.config?.source, "/Volumes/data-1/media/music")
    XCTAssertFalse(model.sourceNeedsAttention)
    XCTAssertFalse(model.sourceRetryPending)
    XCTAssertEqual(model.library?.totalTracks, 1407)
  }

  func testSourceConnectIntentWaitsForExplicitInactiveClickAndCoalescesActivation() {
    var intent = SourceConnectIntent()

    XCTAssertFalse(intent.applicationDidBecomeActive(), "incidental activation must never prompt")
    XCTAssertFalse(intent.userRequestedConnect(isApplicationActive: false))
    XCTAssertFalse(intent.userRequestedConnect(isApplicationActive: false))
    XCTAssertTrue(intent.applicationDidBecomeActive())
    XCTAssertFalse(intent.applicationDidBecomeActive(), "one click burst produces one retry")
    XCTAssertTrue(intent.userRequestedConnect(isApplicationActive: true))
  }

  private func seedDevices(_ serials: [String], in model: AppModel) {
    let devices = serials.map { deviceSnapshot($0) }
    model.apply(.deviceInventorySnapshot(.init(revision: 1, devices: devices)))
  }

  private func seedDevice(
    _ serial: String,
    phase: DevicePhaseLabel,
    sessionID: UInt64? = nil,
    syncedCount: Int = 0,
    libraryCount: Int? = nil,
    in model: AppModel
  ) {
    model.apply(
      .deviceInventorySnapshot(
        .init(
          revision: 1,
          devices: [
            deviceSnapshot(
              serial, phase: phase, sessionID: sessionID, syncedCount: syncedCount,
              libraryCount: libraryCount)
          ])))
  }

  private func deviceSnapshot(
    _ serial: String,
    connected: Bool = true,
    phase: DevicePhaseLabel = .idle,
    sessionID: UInt64? = nil,
    syncedCount: Int = 0,
    libraryCount: Int? = nil,
    latestSuccessfulSync: HistoryEntry? = nil,
    latestAttempt: HistoryEntry? = nil,
    lastTerminalError: String? = nil
  ) -> DeviceSnapshotWire {
    DeviceSnapshotWire(
      identity: DeviceIdentityWire(serial: serial, modelLabel: "iPod Classic", name: nil),
      configured: true,
      connected: connected,
      mount: connected ? "/Volumes/\(serial)" : nil,
      phase: phase,
      sessionID: sessionID,
      storage: nil,
      syncedCount: syncedCount,
      libraryCount: libraryCount,
      latestSuccessfulSync: latestSuccessfulSync,
      latestAttempt: latestAttempt,
      lastTerminalError: lastTerminalError,
      selectionRevision: 0,
      settingsRevision: 0,
      subscriptionsRevision: 0)
  }

  private func terminalHistory(sessionID: UInt64, outcome: String) -> HistoryEntry {
    HistoryEntry(
      serial: "0xA", sessionID: sessionID,
      timestamp: "2026-07-19T12:00:00Z", durationSecs: 10,
      trigger: "manual", outcome: outcome)
  }
}
