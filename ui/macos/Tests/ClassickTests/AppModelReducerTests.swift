import XCTest
@testable import Classick

@MainActor
final class AppModelReducerTests: XCTestCase {
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
        let device = DeviceState(serial: "0xA", model: "iPod Classic (3rd gen)", name: "iPod", drive: "/Volumes/IPOD")
        let preserved = AppDelegate.setupIpodIdentity(device: device, preservingCustomSelection: true)
        XCTAssertEqual(preserved?.customSelection, true, "wizard must preserve an enabled custom-selection toggle")
        let off = AppDelegate.setupIpodIdentity(device: device, preservingCustomSelection: false)
        XCTAssertEqual(off?.customSelection, false)
        XCTAssertNil(AppDelegate.setupIpodIdentity(device: nil, preservingCustomSelection: true), "no device -> no identity to save")
    }

    func testFinishSyncEventPopulatesSkippedForSpaceArtworkAndDbRestoredState() {
        let m = AppModel()
        m.apply(.syncEvent(line: #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#))
        XCTAssertEqual(m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
        XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
        XCTAssertTrue(m.lastRunDbRestored)
    }

    /// Regression: a library-scan's `finish` event never carries
    /// `skipped_for_space`/`artwork`/`db_restored` — those fields are
    /// sync-only. A scan finishing right after a real sync must not clobber
    /// that sync's rollup back to nil/nil/false.
    func testScanFinishDoesNotClobberPriorSyncRollup() {
        let m = AppModel()
        m.apply(.syncEvent(line: #"{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210},"artwork":{"embedded":40,"eligible":42,"failed_sources":2},"db_restored":true}"#))
        XCTAssertEqual(m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
        XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
        XCTAssertTrue(m.lastRunDbRestored)

        m.apply(.statusUpdate(.init(state: .scanning, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#))

        XCTAssertEqual(m.lastRunSkippedForSpace, SkippedForSpace(albums: 14, tracks: 183, bytes: 9_876_543_210))
        XCTAssertEqual(m.lastRunArtwork, ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2))
        XCTAssertTrue(m.lastRunDbRestored)
    }

    func testDeviceConnectThenDisconnect() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "Michael’s iPod"))
        XCTAssertEqual(m.device?.name, "Michael’s iPod")
        m.apply(.deviceDisconnected(serial: "0xA"))
        XCTAssertNil(m.device)
        XCTAssertEqual(m.phase, .noDevice)
    }

    func testSyncProgressFromForwardedEvents() {
        let m = AppModel()
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        m.apply(.syncEvent(line: #"{"type":"track_start","current":34,"total":120,"label":"Karma Police"}"#))
        guard case let .syncing(cur, total, label, _) = m.phase else { return XCTFail() }
        XCTAssertEqual(cur, 34); XCTAssertEqual(total, 120); XCTAssertEqual(label, "Karma Police")
        m.apply(.syncEvent(line: #"{"type":"finish","success":true}"#))
        XCTAssertEqual(m.phase, .idle)
    }

    func testPromptSurfaced() {
        let m = AppModel()
        m.apply(.syncEvent(line: #"{"type":"prompt","id":7,"message":"Source changed","options":["Apply","Cancel"]}"#))
        XCTAssertEqual(m.pendingPrompt?.id, 7)
        XCTAssertEqual(m.pendingPrompt?.options, ["Apply", "Cancel"])
    }

    func testRejectionBecomesError() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "x", drive: "/Volumes/IPOD", name: nil))
        m.apply(.syncRejected(reason: "not_configured"))
        if case .error = m.phase {} else { XCTFail("expected error phase") }
    }

    func testConfigUpdateWithIpodFlipsNotConfiguredToIdle() {
        // Regression: after first-run save_config the daemon emits config_update
        // (not a pushed status_update), so the menu must leave .notConfigured.
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        XCTAssertEqual(m.phase, .notConfigured)
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        XCTAssertEqual(m.phase, .idle)
    }

    func testNeedsFirstRunSetupOnlyAfterEmptyConfigSeen() {
        // The first-run auto-present trigger must stay false until the daemon's
        // get_config reply lands (avoids firing during the startup race), then
        // become true only when no music-library source is configured.
        let m = AppModel()
        XCTAssertFalse(m.needsFirstRunSetup, "unknown before the config reply")

        m.apply(.configUpdate(source: nil, daemon: nil, ipod: nil))
        XCTAssertTrue(m.needsFirstRunSetup, "empty config == never set up")

        m.apply(.configUpdate(source: "/music", daemon: nil, ipod: nil))
        XCTAssertFalse(m.needsFirstRunSetup, "source set == setup completed")
    }

    func testReconnectOfConfiguredDeviceDoesNotFlashSetUp() {
        // Regression: on reconnect the daemon's status_update (configured=true)
        // arrives before config_update, so before we know the paired serial the
        // menu must trust that flag and stay .idle — not flash "Set Up Classick…".
        let m = AppModel()
        m.apply(.statusUpdate(.init(state: .idle, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        XCTAssertEqual(m.phase, .idle)
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        XCTAssertEqual(m.phase, .idle, "must not flash .notConfigured before config_update")
    }

    func testStatusUpdateCarriesSyncedCounts() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        m.apply(.statusUpdate(.init(state: .idle, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
                                    syncedCount: 119, libraryCount: 1500)))
        XCTAssertEqual(m.syncedCount, 119)
        XCTAssertEqual(m.libraryCount, 1500)
    }

    func testPausedSyncEventEntersPausedPhase() {
        let m = AppModel()
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
                                    syncedCount: 50, libraryCount: 1500)))
        m.apply(.syncEvent(line: #"{"type":"paused"}"#))
        guard case .paused = m.phase else { return XCTFail("expected .paused") }
    }

    func testPausedPhaseSurvivesTrailingIdleStatus() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
                                    syncedCount: 84, libraryCount: 1381)))
        m.apply(.syncEvent(line: #"{"type":"paused"}"#))
        guard case .paused = m.phase else { return XCTFail("expected .paused after pause event") }
        // Subprocess exits → daemon broadcasts idle. Paused MUST persist and
        // refresh its counts, not revert to plain idle.
        m.apply(.statusUpdate(.init(state: .idle, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
                                    syncedCount: 84, libraryCount: 1381)))
        guard case let .paused(synced, total) = m.phase else {
            return XCTFail("paused state lost after trailing idle status")
        }
        XCTAssertEqual(synced, 84)
        XCTAssertEqual(total, 1381)
    }

    func testResumeFromPausedEntersSyncing() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        m.apply(.syncEvent(line: #"{"type":"paused"}"#))
        guard case .paused = m.phase else { return XCTFail("expected .paused") }
        // Resume sends TriggerSync → daemon reports syncing → leave paused.
        m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true, lastSync: nil, storage: nil,
                                    syncedCount: 84, libraryCount: 1381)))
        guard case .syncing = m.phase else { return XCTFail("expected .syncing after resume") }
    }

    func testPausedClearsOnDeviceDisconnect() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        m.apply(.syncEvent(line: #"{"type":"paused"}"#))
        guard case .paused = m.phase else { return XCTFail("expected .paused") }
        m.apply(.deviceDisconnected(serial: "0xA"))
        guard case .noDevice = m.phase else { return XCTFail("expected .noDevice after unplug") }
    }

    func testDeviceSwapToUnpairedShowsNotConfigured() {
        // Regression: "configured" must be checked against the *currently
        // connected* device's serial, not just "some iPod was ever paired" —
        // otherwise swapping in an unpaired iPod after a paired one shows
        // "Sync Now" instead of "Set Up Classick…".
        let m = AppModel()
        m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        m.apply(.configUpdate(source: "/music", daemon: nil,
                              ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
        XCTAssertEqual(m.phase, .idle)

        m.apply(.deviceDisconnected(serial: "0xA"))
        m.apply(.deviceConnected(serial: "0xB", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
        XCTAssertEqual(m.phase, .notConfigured)
    }

    func testLibraryAndSelectionEventsPopulateModel() {
        let m = AppModel()
        m.apply(.libraryUpdate(LibraryInfo(
            sourceRoot: "/m", scannedAtUnixSecs: 1,
            artists: [], genres: [], totalTracks: 0, totalBytes: 0)))
        XCTAssertEqual(m.library?.sourceRoot, "/m")
        m.apply(.selectionUpdate(mode: .include, rules: [.genre(name: "IDM")]))
        XCTAssertEqual(m.selection?.mode, .include)
        XCTAssertEqual(m.selection?.rules, [.genre(name: "IDM")])
        m.apply(.selectionPreview(SelectionPreviewInfo(
            selectedTracks: 10, selectedBytes: 100, adds: 2, removes: 3)))
        XCTAssertEqual(m.selectionPreview?.removes, 3)
    }

    func testScanningStatusMakesScanningPhaseAndRoutesTrackStart() {
        let m = AppModel()
        // device + config so computePhase doesn't fall into noDevice/notConfigured
        m.apply(.deviceConnected(serial: "S", modelLabel: "iPod", drive: "/Volumes/IPOD", name: nil))
        m.apply(.configUpdate(source: "/m", daemon: nil,
                              ipod: IpodIdentity(serial: "S", modelLabel: "iPod", name: nil)))
        m.apply(.statusUpdate(.init(state: .scanning, configured: true, ipodConnected: true, lastSync: nil, storage: nil)))
        guard case .scanning = m.phase else { return XCTFail("expected scanning, got \(m.phase)") }

        // Forwarded scan progress must update .scanning, NOT flip to .syncing.
        m.apply(.syncEvent(line: #"{"type":"track_start","current":5,"total":100,"label":"x.flac"}"#))
        guard case let .scanning(current, total) = m.phase else {
            return XCTFail("track_start during a scan must stay in scanning; got \(m.phase)")
        }
        XCTAssertEqual(current, 5)
        XCTAssertEqual(total, 100)
    }

    @MainActor
    func testSyncingPhaseCarriesEta() {
        let m = AppModel()
        m.apply(.deviceConnected(serial: "S", modelLabel: "iPod", drive: "/V", name: nil))
        m.apply(.configUpdate(source: "/m", daemon: nil,
                              ipod: IpodIdentity(serial: "S", modelLabel: "iPod", name: nil)))
        m.apply(.syncEvent(line: #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#))
        if case let .syncing(current, total, _, eta) = m.phase {
            XCTAssertEqual(current, 5); XCTAssertEqual(total, 10); XCTAssertEqual(eta, 42)
        } else { XCTFail("expected syncing, got \(m.phase)") }
    }

    // MARK: - Task 17: Replace Library, selection toggle, device-row rollup lines

    /// Typed-confirmation gate: only an exact, case-sensitive match of the
    /// device name arms the Replace Library confirm button.
    func testReplaceConfirmationArmsOnlyOnExactName() {
        XCTAssertTrue(ReplaceConfirmation.isArmed(input: "Michael's iPod", deviceName: "Michael's iPod"))
        XCTAssertFalse(ReplaceConfirmation.isArmed(input: "", deviceName: "Michael's iPod"))
        XCTAssertFalse(ReplaceConfirmation.isArmed(input: "michael's ipod", deviceName: "Michael's iPod"), "must be case-sensitive")
        XCTAssertFalse(ReplaceConfirmation.isArmed(input: "Michael's iPo", deviceName: "Michael's iPod"), "must be an exact match, not a prefix")
        XCTAssertFalse(ReplaceConfirmation.isArmed(input: "Michael's iPod ", deviceName: "Michael's iPod"), "trailing whitespace must not arm")
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

        XCTAssertNil(DeviceRowFormatting.skippedForSpaceLine(syncedSummary: "1500 of 1500", skipped: nil))
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
            DeviceRowFormatting.artworkMissingLine(ArtworkSummary(embedded: 42, eligible: 42, failedSources: 0)),
            "everything embedded, nothing failed -> no line")

        XCTAssertEqual(
            DeviceRowFormatting.artworkMissingLine(ArtworkSummary(embedded: 40, eligible: 42, failedSources: 2)),
            "Art missing for 2 tracks")
        XCTAssertEqual(
            DeviceRowFormatting.artworkMissingLine(ArtworkSummary(embedded: 1, eligible: 2, failedSources: 0)),
            "Art missing for 1 track", "singular for a shortfall of exactly one")
        XCTAssertEqual(
            DeviceRowFormatting.artworkMissingLine(ArtworkSummary(embedded: 42, eligible: 42, failedSources: 3)),
            "Art missing for 3 tracks", "failedSources > 0 with no embed shortfall still surfaces")
    }

    /// The Selection picker's save path (Task 17): SaveConfig replaces the
    /// whole `ipod` blob, so flipping `customSelection` must carry the
    /// existing serial/model_label/name through untouched — mirrors
    /// `testSetupWizardPreservesCustomSelection` for the setup wizard's own
    /// identity-construction site.
    func testSaveIpodSelectionPreservesIdentityFields() {
        let existing = IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: "Michael's iPod", customSelection: false)
        let flipped = AppDelegate.withCustomSelection(true, from: existing)
        XCTAssertEqual(flipped?.serial, "0xA")
        XCTAssertEqual(flipped?.modelLabel, "iPod Classic (3rd gen)")
        XCTAssertEqual(flipped?.name, "Michael's iPod")
        XCTAssertEqual(flipped?.customSelection, true)

        XCTAssertNil(AppDelegate.withCustomSelection(true, from: nil), "no persisted identity yet -> nothing to save")
    }

    @MainActor
    func testHistoryRetained() {
        let m = AppModel()
        let e = HistoryEntry(timestamp: "2026-07-14T10:00:00Z", durationSecs: 5,
                             trigger: "manual", outcome: "ok")
        m.apply(.historyUpdate(entries: [e]))
        XCTAssertEqual(m.history.count, 1)
        XCTAssertEqual(m.history.first?.trigger, "manual")
    }

    // MARK: - Protocol 1.6.0: playlists, per-device config, device preview

    func testPlaylistsUpdateReplacesList() {
        let m = AppModel()
        m.apply(.playlistsUpdate([
            PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
        ]))
        XCTAssertEqual(m.playlists.map(\.slug), ["gym"])

        m.apply(.playlistsUpdate([
            PlaylistSummary(slug: "chill", name: "Chill", kind: .smart, tracks: 5, bytes: 100, error: nil),
        ]))
        XCTAssertEqual(m.playlists.map(\.slug), ["chill"], "playlists_update must replace the list, not append")
    }

    func testPlaylistDetailStoresMostRecentReply() {
        let m = AppModel()
        m.apply(.playlistDetail(PlaylistDetail(slug: "gym", name: "Gym", kind: .manual, tracks: ["a.flac"], rules: nil, error: nil)))
        XCTAssertEqual(m.playlistDetail?.slug, "gym")
        XCTAssertEqual(m.playlistDetail?.tracks, ["a.flac"])
    }

    func testDeviceConfigUpdateUpsertsBySerial() {
        let m = AppModel()
        m.apply(.deviceConfigUpdate(
            serial: "0xA",
            selection: SelectionState(mode: .include, rules: [.artist(name: "Boards of Canada")]),
            subscriptions: SubscriptionsWire(playlists: ["gym"]),
            settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false)))
        XCTAssertEqual(m.deviceConfigs["0xA"]?.selection.mode, .include)
        XCTAssertEqual(m.deviceConfigs["0xA"]?.subscriptions.playlists, ["gym"])

        // A config update for a second serial must upsert, not clobber the first.
        m.apply(.deviceConfigUpdate(
            serial: "0xB",
            selection: SelectionState(mode: .all, rules: []),
            subscriptions: SubscriptionsWire(playlists: []),
            settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false)))
        XCTAssertEqual(m.deviceConfigs.count, 2)
        XCTAssertEqual(m.deviceConfigs["0xA"]?.selection.mode, .include, "upsert must not disturb other serials")
    }

    func testDevicePreviewAttachesToTheRequestedSerial() {
        let m = AppModel()
        m.willRequestDevicePreview(serial: "0xA")
        m.apply(.devicePreview(DevicePreview(
            selectedTracks: 412, selectedBytes: 5_123_456_789,
            playlistExtraTracks: 3, playlistExtraBytes: 90_000_000,
            projectedFreeBytes: 1_200_000_000, unresolvedSubscriptions: nil)))
        XCTAssertEqual(m.deviceConfigs["0xA"]?.preview?.selectedTracks, 412)
        XCTAssertEqual(m.deviceConfigs["0xA"]?.preview?.projectedFreeBytes, 1_200_000_000)
    }

    /// `device_preview` carries no correlation id — without a pending
    /// request to attach it to, the app must drop it rather than guess.
    func testDevicePreviewWithNoPendingRequestIsDropped() {
        let m = AppModel()
        m.apply(.devicePreview(DevicePreview(
            selectedTracks: 1, selectedBytes: 1, playlistExtraTracks: 0, playlistExtraBytes: 0,
            projectedFreeBytes: nil, unresolvedSubscriptions: nil)))
        XCTAssertTrue(m.deviceConfigs.isEmpty)
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
            PlaylistSummary(slug: "chill", name: "Chill", kind: .smart, tracks: 5, bytes: 100, error: nil),
            PlaylistSummary(slug: "new-playlist", name: "New Playlist", kind: .manual, tracks: 0, bytes: 0, error: nil),
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
            PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
        ]
        XCTAssertNil(SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated))
    }

    func testDestinationForNewlyCreatedPlaylistOnEmptyPriorSnapshot() {
        let updated = [
            PlaylistSummary(slug: "first", name: "First", kind: .manual, tracks: 0, bytes: 0, error: nil),
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
            PlaylistSummary(slug: "another-clients-mix", name: "Another Client's Mix", kind: .manual, tracks: 0, bytes: 0, error: nil),
            PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
            PlaylistSummary(slug: "new-playlist", name: "New Playlist", kind: .manual, tracks: 0, bytes: 0, error: nil),
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
            PlaylistSummary(slug: "alpha-mix", name: "Alpha Mix", kind: .manual, tracks: 0, bytes: 0, error: nil),
            PlaylistSummary(slug: "beta-mix", name: "Beta Mix", kind: .manual, tracks: 0, bytes: 0, error: nil),
            PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil),
        ]
        XCTAssertEqual(
            SidebarDestination.destinationForNewlyCreatedPlaylist(priorSlugs: prior, updated: updated),
            .playlist(slug: "alpha-mix"))
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
        let list = [PlaylistSummary(slug: "gym", name: "Gym", kind: .manual, tracks: 12, bytes: 900, error: nil)]
        m.apply(.playlistsUpdate(list))
        XCTAssertEqual(m.playlistsUpdateRevision, 1)
        m.apply(.playlistsUpdate(list))
        XCTAssertEqual(m.playlistsUpdateRevision, 2, "revision must bump even when the list content is identical")
    }
}
