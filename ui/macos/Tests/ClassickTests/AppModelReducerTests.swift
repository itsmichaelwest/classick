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

    @MainActor
    func testHistoryRetained() {
        let m = AppModel()
        let e = HistoryEntry(timestamp: "2026-07-14T10:00:00Z", durationSecs: 5,
                             trigger: "manual", outcome: "ok")
        m.apply(.historyUpdate(entries: [e]))
        XCTAssertEqual(m.history.count, 1)
        XCTAssertEqual(m.history.first?.trigger, "manual")
    }
}
