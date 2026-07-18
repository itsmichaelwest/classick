import XCTest
@testable import Classick

/// Pure-logic coverage for the device Music page (Task 5) — no SwiftUI
/// involved, all static functions on `DeviceMusicLogic` operating on plain
/// values. `seededSelection` is THE trust-critical function in this plan:
/// it must reproduce the device's current contents exactly (album
/// granularity) on an Entire->Selected mode switch, so the switch is
/// zero-diff (nothing gets silently removed by merely changing the mode
/// picker). Every cell of the 3x3 `SelectionMode` transition truth table is
/// covered explicitly below, per the plan's mandate.
final class DeviceMusicLogicTests: XCTestCase {
    private let radiohead = LibraryArtist(name: "Radiohead", albums: [
        LibraryAlbum(name: "Kid A", genre: nil, tracks: 10, bytes: 100),
        LibraryAlbum(name: "OK Computer", genre: nil, tracks: 12, bytes: 120),
    ])
    private let aphex = LibraryArtist(name: "Aphex Twin", albums: [
        LibraryAlbum(name: "Drukqs", genre: nil, tracks: 21, bytes: 210),
    ])
    private var library: [LibraryArtist] { [radiohead, aphex] }

    // MARK: - seededSelection: full 3x3 truth table

    func testSeed_allToAll_isNoOpKeepsCurrent() {
        let current: [SelectionRule] = [.artist(name: "Aphex Twin")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .all, newMode: .all, current: current)
        XCTAssertEqual(result, current)
    }

    func testSeed_allToInclude_reproducesCurrentContentsAtAlbumGranularity() {
        // THE zero-diff case: switching from Entire library to Selected
        // items must reproduce exactly what's currently syncing (the whole
        // known library) as album-level rules, so nothing drops out merely
        // because the mode changed.
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .all, newMode: .include, current: [])
        XCTAssertEqual(Set(result), [
            .album(artist: "Radiohead", album: "Kid A"),
            .album(artist: "Radiohead", album: "OK Computer"),
            .album(artist: "Aphex Twin", album: "Drukqs"),
        ])
    }

    func testSeed_allToInclude_ignoresStaleDormantCurrentRules() {
        // A dormant leftover rule from an earlier selected/except session
        // (kept around by the ->entire rule below) must NOT leak into the
        // freshly seeded set — the seed is a full snapshot of contents, not
        // a merge with whatever was dormant.
        let stale: [SelectionRule] = [.artist(name: "Some Other Artist")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .all, newMode: .include, current: stale)
        XCTAssertFalse(result.contains(.artist(name: "Some Other Artist")))
        XCTAssertEqual(result.count, 3, "exactly the 3 albums in contents, nothing from the stale rule")
    }

    func testSeed_allToExclude_isEmptyBecauseExcludingNothingIsAlreadyZeroDiff() {
        // Entire library -> All except selected: an EMPTY exclude rule set
        // already means "exclude nothing" == the entire library, so this is
        // zero-diff without seeding anything from contents. Any dormant
        // leftover in `current` must be discarded here — keeping it would
        // reactivate it as an active removal the instant this mode-switch
        // fires, which is exactly the silent-removal bug the zero-diff
        // invariant guards against.
        let stale: [SelectionRule] = [.artist(name: "Radiohead")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .all, newMode: .exclude, current: stale)
        XCTAssertEqual(result, [])
    }

    func testSeed_includeToInclude_isNoOpKeepsCurrent() {
        let current: [SelectionRule] = [.artist(name: "Radiohead")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .include, newMode: .include, current: current)
        XCTAssertEqual(result, current)
    }

    func testSeed_includeToExclude_keepsRulesVerbatim() {
        // Selected <-> Except: the SAME rule list is reinterpreted under the
        // opposite mode's semantics — an explicit content flip the user
        // asked for by picking a different mode, not something this fn
        // should mask by recomputing.
        let current: [SelectionRule] = [.artist(name: "Radiohead"), .genre(name: "Ambient")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .include, newMode: .exclude, current: current)
        XCTAssertEqual(result, current)
    }

    func testSeed_excludeToInclude_keepsRulesVerbatim() {
        let current: [SelectionRule] = [.album(artist: "Aphex Twin", album: "Drukqs")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .exclude, newMode: .include, current: current)
        XCTAssertEqual(result, current)
    }

    func testSeed_excludeToExclude_isNoOpKeepsCurrent() {
        let current: [SelectionRule] = [.genre(name: "IDM")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .exclude, newMode: .exclude, current: current)
        XCTAssertEqual(result, current)
    }

    func testSeed_includeToAll_keepsRulesDormant() {
        let current: [SelectionRule] = [.artist(name: "Radiohead")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .include, newMode: .all, current: current)
        XCTAssertEqual(result, current, "rules must stay dormant, not clear, so switching back restores them")
    }

    func testSeed_excludeToAll_keepsRulesDormant() {
        let current: [SelectionRule] = [.genre(name: "Ambient")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: library, previousMode: .exclude, newMode: .all, current: current)
        XCTAssertEqual(result, current)
    }

    /// Mixed-case truth-table case (fix commit companion): the library's
    /// canonical casing is "Radiohead"/"Kid A", but a stale dormant rule
    /// persisted in a different case ("radiohead") must not affect the
    /// freshly seeded album rules, which must use the CONTENTS' canonical
    /// case verbatim, not the stale rule's case.
    func testSeed_allToInclude_usesContentsCanonicalCaseRegardlessOfStaleRuleCase() {
        let staleDifferentCase: [SelectionRule] = [.artist(name: "radiohead")]
        let result = DeviceMusicLogic.seededSelection(
            fromDeviceContents: [radiohead], previousMode: .all, newMode: .include, current: staleDifferentCase)
        XCTAssertEqual(Set(result), [
            .album(artist: "Radiohead", album: "Kid A"),
            .album(artist: "Radiohead", album: "OK Computer"),
        ], "seeded rules must use the library's canonical case, not the stale rule's")
    }

    // MARK: - caption(for mode:)

    func testCaptionEntireLibrary() {
        XCTAssertEqual(
            DeviceMusicLogic.caption(mode: .all, isConnected: true),
            "Everything in your library syncs to this iPod.")
    }

    func testCaptionSelectedItems() {
        XCTAssertEqual(
            DeviceMusicLogic.caption(mode: .include, isConnected: true),
            "Checked items sync to this iPod.")
    }

    func testCaptionAllExceptSelected() {
        XCTAssertEqual(
            DeviceMusicLogic.caption(mode: .exclude, isConnected: true),
            "Checked items are left off this iPod.")
    }

    func testCaptionDisconnectedOverridesModeCaption() {
        for mode in [SelectionMode.all, .include, .exclude] {
            XCTAssertEqual(
                DeviceMusicLogic.caption(mode: mode, isConnected: false),
                "Not connected — changes apply on next sync")
        }
    }

    // MARK: - Sync Now disabled predicate

    func testSyncNowDisabledWhileSyncing() {
        XCTAssertTrue(DeviceMusicLogic.isSyncNowDisabled(phase: .syncing(current: 1, total: 10, label: "x", etaSecs: nil)))
    }

    func testSyncNowDisabledWhileScanning() {
        XCTAssertTrue(DeviceMusicLogic.isSyncNowDisabled(phase: .scanning(current: 1, total: 10)))
    }

    func testSyncNowDisabledWhenDisconnected() {
        XCTAssertTrue(DeviceMusicLogic.isSyncNowDisabled(phase: .noDevice))
    }

    func testSyncNowEnabledWhenIdle() {
        XCTAssertFalse(DeviceMusicLogic.isSyncNowDisabled(phase: .idle))
    }

    func testSyncNowEnabledWhenPaused() {
        XCTAssertFalse(DeviceMusicLogic.isSyncNowDisabled(phase: .paused(synced: 5, total: 10)))
    }

    // MARK: - Capacity bar (supporting formatting helper)

    func testCapacityBarNilWithoutPreview() {
        XCTAssertNil(DeviceMusicLogic.capacityBar(storage: (free: 1_000, total: 10_000), preview: nil))
    }

    func testCapacityBarNilWithoutStorage() {
        let preview = DevicePreview(selectedTracks: 1, selectedBytes: 100, playlistExtraTracks: 0, playlistExtraBytes: 0, projectedFreeBytes: nil, unresolvedSubscriptions: nil)
        XCTAssertNil(DeviceMusicLogic.capacityBar(storage: nil, preview: preview))
    }

    func testCapacityBarComputesUsedAndProjectedFractions() {
        let preview = DevicePreview(
            selectedTracks: 100, selectedBytes: 4_000_000_000,
            playlistExtraTracks: 5, playlistExtraBytes: 1_000_000_000,
            projectedFreeBytes: 4_000_000_000, unresolvedSubscriptions: nil)
        guard let bar = DeviceMusicLogic.capacityBar(storage: (free: 6_000_000_000, total: 10_000_000_000), preview: preview) else {
            return XCTFail("expected a capacity bar")
        }
        XCTAssertEqual(bar.usedBytes, 5_000_000_000)
        XCTAssertEqual(bar.projectedBytes, 6_000_000_000)
        XCTAssertEqual(bar.usedFraction, 0.5, accuracy: 0.0001)
        XCTAssertEqual(bar.projectedFraction, 0.6, accuracy: 0.0001)
    }

    // MARK: - Unresolved subscriptions line

    func testUnresolvedSubscriptionsLineNilWhenAbsentOrEmpty() {
        XCTAssertNil(DeviceMusicLogic.unresolvedSubscriptionsLine(nil))
        XCTAssertNil(DeviceMusicLogic.unresolvedSubscriptionsLine([]))
    }

    func testUnresolvedSubscriptionsLineSingularAndPlural() {
        XCTAssertEqual(DeviceMusicLogic.unresolvedSubscriptionsLine(["gym"]), "1 subscribed playlist couldn't be resolved")
        XCTAssertEqual(DeviceMusicLogic.unresolvedSubscriptionsLine(["gym", "chill"]), "2 subscribed playlists couldn't be resolved")
    }
}
