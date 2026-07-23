import XCTest
@testable import Classick

/// Pure-logic coverage for the playlist editor pages (Task 7): the manual
/// track list (`ManualPlaylistLogic`), the shared rename/delete chrome
/// (`PlaylistEditorLogic`), and the smart rule builder (`SmartRulesLogic`).
/// No SwiftUI involved, mirroring `DeviceMusicLogicTests`'/
/// `DeviceSettingsLogicTests`' style.
final class PlaylistEditorLogicTests: XCTestCase {
    // MARK: - ManualPlaylistLogic.trackDisplay

    func testTrackDisplaySplitsArtistAlbumTrackPath() {
        let display = ManualPlaylistLogic.trackDisplay(path: "Boards of Canada/Music Has the Right to Children/01 Wildlife Analysis.flac")
        XCTAssertEqual(display.title, "01 Wildlife Analysis")
        XCTAssertEqual(display.artist, "Boards of Canada")
    }

    func testTrackDisplayHandlesTwoComponentPath() {
        let display = ManualPlaylistLogic.trackDisplay(path: "B/02.flac")
        XCTAssertEqual(display.title, "02")
        XCTAssertEqual(display.artist, "B")
    }

    func testTrackDisplayHandlesBareFilenameWithNoArtist() {
        let display = ManualPlaylistLogic.trackDisplay(path: "track.flac")
        XCTAssertEqual(display.title, "track")
        XCTAssertNil(display.artist)
    }

    func testTrackDisplayNormalizesBackslashes() {
        let display = ManualPlaylistLogic.trackDisplay(path: #"Artist\Album\01.flac"#)
        XCTAssertEqual(display.title, "01")
        XCTAssertEqual(display.artist, "Artist")
    }

    // (isLikelyMissing was removed: a path-derived artist/album heuristic
    // false-flagged whole playlists for libraries whose folder layout
    // doesn't mirror their tags. No client-side signal can do better — the
    // wire has no per-file existence data.)

    // MARK: - ManualPlaylistLogic.appendingTracks (dedup, preserve order)

    func testAppendingTracksPreservesExistingOrderAndAppendsNew() {
        let result = ManualPlaylistLogic.appendingTracks(["a.flac", "b.flac"], adding: ["c.flac", "d.flac"])
        XCTAssertEqual(result, ["a.flac", "b.flac", "c.flac", "d.flac"])
    }

    func testAppendingTracksDedupsAgainstExisting() {
        let result = ManualPlaylistLogic.appendingTracks(["a.flac", "b.flac"], adding: ["b.flac", "c.flac"])
        XCTAssertEqual(result, ["a.flac", "b.flac", "c.flac"], "an already-present track must not be duplicated")
    }

    func testAppendingTracksDedupsWithinTheAddedBatchItself() {
        let result = ManualPlaylistLogic.appendingTracks([], adding: ["a.flac", "a.flac", "b.flac"])
        XCTAssertEqual(result, ["a.flac", "b.flac"])
    }

    /// Fix (natural track order on Add): `resolve_tracks` returns
    /// lexicographic path order server-side (deliberate, deterministic — the
    /// Rust side is unchanged), so non-zero-padded filenames like
    /// "1.flac", "10.flac", "2.flac" sort as 1, 10, 2 lexicographically. The
    /// client must natural-sort the NEW batch before appending so the track
    /// list matches the album's actual running order.
    func testAppendingTracksOrdersNewBatchNaturallyNotLexicographically() {
        let result = ManualPlaylistLogic.appendingTracks(
            [], adding: ["Artist/Album/1.flac", "Artist/Album/10.flac", "Artist/Album/2.flac"])
        XCTAssertEqual(result, ["Artist/Album/1.flac", "Artist/Album/2.flac", "Artist/Album/10.flac"])
    }

    /// The existing draft's order must never be disturbed by natural
    /// sorting — only the newly added batch is reordered.
    func testAppendingTracksLeavesExistingDraftOrderUntouched() {
        let result = ManualPlaylistLogic.appendingTracks(
            ["Z/Album/10.flac", "A/Album/2.flac"], adding: ["Artist/Album/10.flac", "Artist/Album/2.flac"])
        XCTAssertEqual(result, ["Z/Album/10.flac", "A/Album/2.flac", "Artist/Album/2.flac", "Artist/Album/10.flac"])
    }

    /// A multi-album batch must stay grouped by album (directory), with each
    /// album's own tracks in natural numeric order within that group — not
    /// interleaved across albums by a global lexicographic sort.
    func testAppendingTracksMultiAlbumBatchStaysAlbumGroupedInNaturalOrder() {
        let result = ManualPlaylistLogic.appendingTracks(
            [],
            adding: [
                "Artist/Album B/1.flac", "Artist/Album A/10.flac",
                "Artist/Album A/2.flac", "Artist/Album B/2.flac", "Artist/Album A/1.flac",
            ])
        XCTAssertEqual(
            result,
            [
                "Artist/Album A/1.flac", "Artist/Album A/2.flac", "Artist/Album A/10.flac",
                "Artist/Album B/1.flac", "Artist/Album B/2.flac",
            ])
    }

    /// Dedup (both against existing and within the batch) must still work
    /// after natural sorting reorders the batch.
    func testAppendingTracksDedupsWithinBatchAfterNaturalSort() {
        let result = ManualPlaylistLogic.appendingTracks(
            ["Artist/Album/1.flac"], adding: ["Artist/Album/10.flac", "Artist/Album/1.flac", "Artist/Album/10.flac"])
        XCTAssertEqual(result, ["Artist/Album/1.flac", "Artist/Album/10.flac"])
    }

    // MARK: - ManualPlaylistLogic.moved / removed

    func testMovedReordersTracks() {
        let result = ManualPlaylistLogic.moved(["a", "b", "c"], from: IndexSet(integer: 2), to: 0)
        XCTAssertEqual(result, ["c", "a", "b"])
    }

    func testRemovedDropsSelectedOffsets() {
        let result = ManualPlaylistLogic.removed(["a", "b", "c"], at: IndexSet(integer: 1))
        XCTAssertEqual(result, ["a", "c"])
    }

    // MARK: - PlaylistEditorLogic.subscribedDeviceCount / deleteConfirmMessage

    func testSubscribedDeviceCountCountsOnlyDevicesSubscribedToThisSlug() {
        let configs: [DeviceID: DeviceConfigState] = [
            "0xA": .init(selection: .init(mode: .all, rules: []), subscriptions: .init(playlists: ["gym"]), settings: .init(autoSync: true, rockboxCompat: false), preview: nil),
            "0xB": .init(selection: .init(mode: .all, rules: []), subscriptions: .init(playlists: ["chill"]), settings: .init(autoSync: true, rockboxCompat: false), preview: nil),
            "0xC": .init(selection: .init(mode: .all, rules: []), subscriptions: .init(playlists: ["gym", "chill"]), settings: .init(autoSync: true, rockboxCompat: false), preview: nil),
        ]
        XCTAssertEqual(PlaylistEditorLogic.subscribedDeviceCount(slug: "gym", deviceConfigs: configs), 2)
    }

    func testDeleteConfirmMessageMentionsSubscribedDeviceCount() {
        XCTAssertEqual(
            PlaylistEditorLogic.deleteConfirmMessage(subscribedDeviceCount: 2),
            "It will also be removed from 2 iPods that sync it. This can't be undone.")
        XCTAssertEqual(
            PlaylistEditorLogic.deleteConfirmMessage(subscribedDeviceCount: 1),
            "It will also be removed from 1 iPod that syncs it. This can't be undone.")
    }

    func testDeleteConfirmMessageWhenNotSubscribedAnywhere() {
        XCTAssertEqual(PlaylistEditorLogic.deleteConfirmMessage(subscribedDeviceCount: 0), "This can't be undone.")
    }

    // MARK: - PlaylistEditorLogic.isNameValid

    func testIsNameValidRejectsBlankOrWhitespaceOnly() {
        XCTAssertFalse(PlaylistEditorLogic.isNameValid(""))
        XCTAssertFalse(PlaylistEditorLogic.isNameValid("   "))
        XCTAssertTrue(PlaylistEditorLogic.isNameValid("Gym"))
    }

    // MARK: - SmartRulesLogic.rulesAreValid (rule-row validity -> save-enabled predicate)

    func testRulesAreValidRequiresNonBlankValueOnEveryRow() {
        XCTAssertTrue(SmartRulesLogic.rulesAreValid([SmartRuleWire(field: .genre, op: .is, value: "IDM")]))
        XCTAssertFalse(SmartRulesLogic.rulesAreValid([SmartRuleWire(field: .genre, op: .is, value: "")]))
        XCTAssertFalse(SmartRulesLogic.rulesAreValid([SmartRuleWire(field: .genre, op: .is, value: "  ")]))
    }

    func testRulesAreValidAllowsEmptyRuleSet() {
        XCTAssertTrue(SmartRulesLogic.rulesAreValid([]), "zero rules is a valid (matches-everything) smart playlist")
    }

    // MARK: - SmartRulesLogic limit round-trip + validity

    func testLimitKindAndValueTextRoundTripBytes() {
        XCTAssertEqual(SmartRulesLogic.limitKind(for: .bytes(500_000_000)), .bytes)
        XCTAssertEqual(SmartRulesLogic.limitValueText(for: .bytes(500_000_000)), "500000000")
    }

    func testLimitKindAndValueTextRoundTripTracks() {
        XCTAssertEqual(SmartRulesLogic.limitKind(for: .tracks(50)), .tracks)
        XCTAssertEqual(SmartRulesLogic.limitValueText(for: .tracks(50)), "50")
    }

    func testLimitKindAndValueTextRoundTripNone() {
        XCTAssertEqual(SmartRulesLogic.limitKind(for: nil), .none)
        XCTAssertEqual(SmartRulesLogic.limitValueText(for: nil), "")
    }

    func testIsLimitValidNoneIsAlwaysValid() {
        XCTAssertTrue(SmartRulesLogic.isLimitValid(kind: .none, valueText: ""))
        XCTAssertTrue(SmartRulesLogic.isLimitValid(kind: .none, valueText: "garbage"))
    }

    func testIsLimitValidRequiresPositiveNumberForBytesAndTracks() {
        XCTAssertTrue(SmartRulesLogic.isLimitValid(kind: .bytes, valueText: "100"))
        XCTAssertFalse(SmartRulesLogic.isLimitValid(kind: .bytes, valueText: "0"))
        XCTAssertFalse(SmartRulesLogic.isLimitValid(kind: .bytes, valueText: "abc"))
        XCTAssertTrue(SmartRulesLogic.isLimitValid(kind: .tracks, valueText: "50"))
        XCTAssertFalse(SmartRulesLogic.isLimitValid(kind: .tracks, valueText: "-5"))
    }

    func testLimitBuildsCorrectWireCase() {
        XCTAssertEqual(SmartRulesLogic.limit(kind: .none, valueText: ""), nil)
        XCTAssertEqual(SmartRulesLogic.limit(kind: .bytes, valueText: "100"), .bytes(100))
        XCTAssertEqual(SmartRulesLogic.limit(kind: .tracks, valueText: "50"), .tracks(50))
    }

    // MARK: - SmartRulesLogic.previewLine

    func testPreviewLineFormatsTracksAndBytesFromSummary() {
        let summary = PlaylistSummary(slug: "recent-idm", name: "Recent IDM", kind: .smart, tracks: 42, bytes: 123_456_789, error: nil)
        XCTAssertEqual(SmartRulesLogic.previewLine(summary: summary), "42 tracks · \(formatBytes(123_456_789))")
    }

    func testPreviewLineSingularTrack() {
        let summary = PlaylistSummary(slug: "x", name: "X", kind: .smart, tracks: 1, bytes: 0, error: nil)
        XCTAssertEqual(SmartRulesLogic.previewLine(summary: summary), "1 track · \(formatBytes(0))")
    }

    func testPreviewLineWhenSummaryNotYetAvailable() {
        XCTAssertEqual(SmartRulesLogic.previewLine(summary: nil), "Calculating…")
    }
}

/// Title-column cleanup: the table has artist/album columns, so the title
/// must be JUST the song name — filename echoes of artist/album and track
/// numbers stripped.
extension PlaylistEditorLogicTests {
    func testCleanedTitleTakesLastSegmentOfDashedFilename() {
        XCTAssertEqual(
            ManualPlaylistLogic.cleanedTitle("Birdy - Beautiful Lies - 01 - Growing Pains"),
            "Growing Pains")
    }

    func testCleanedTitleStripsLeadingTrackNumberPrefix() {
        XCTAssertEqual(ManualPlaylistLogic.cleanedTitle("03 Telephasic Workshop"), "Telephasic Workshop")
        XCTAssertEqual(ManualPlaylistLogic.cleanedTitle("1 So What"), "So What")
        XCTAssertEqual(ManualPlaylistLogic.cleanedTitle("07. Aquarius"), "Aquarius")
    }

    func testCleanedTitleKeepsNumericSongNames() {
        // A song literally titled with digits survives — the prefix strip
        // refuses to leave an empty title.
        XCTAssertEqual(ManualPlaylistLogic.cleanedTitle("1979"), "1979")
        XCTAssertEqual(ManualPlaylistLogic.cleanedTitle("Smashing Pumpkins - 1979"), "1979")
    }

    func testCleanedTitleSongNamedAfterAlbumSurvives() {
        XCTAssertEqual(
            ManualPlaylistLogic.cleanedTitle("Birdy - Beautiful Lies - 14 - Beautiful Lies"),
            "Beautiful Lies",
            "the last segment is the title by convention — protected even when it matches the album")
    }
}
