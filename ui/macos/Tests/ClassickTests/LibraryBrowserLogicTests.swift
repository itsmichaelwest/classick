import XCTest
@testable import Classick

/// Pure-logic coverage for `LibraryBrowser` (Task 4): deterministic
/// grouping/ordering from the wire `LibraryArtist`/`LibraryAlbum`
/// aggregates, the tri-state artist checkbox, and Set-based toggle
/// behavior for both `SelectStyle`s. No SwiftUI involved — these are all
/// static functions operating on plain values.
final class LibraryBrowserLogicTests: XCTestCase {
    private let library = [
        LibraryArtist(name: "Squarepusher", albums: [
            LibraryAlbum(name: "Go Plastic", genre: nil, tracks: 10, bytes: 100),
            LibraryAlbum(name: "Hard Normal Daddy", genre: nil, tracks: 9, bytes: 90),
        ]),
        LibraryArtist(name: "aphex twin", albums: [
            LibraryAlbum(name: "syro", genre: nil, tracks: 12, bytes: 120),
            LibraryAlbum(name: "Drukqs", genre: nil, tracks: 21, bytes: 210),
        ]),
        LibraryArtist(name: "", albums: [
            LibraryAlbum(name: "", genre: nil, tracks: 1, bytes: 5),
        ]),
    ]

    // MARK: - Grouping / ordering

    func testOrderedArtistsIsCaseInsensitiveAlphaWithUnknownLast() {
        let ordered = LibraryBrowser.orderedArtists(library)
        XCTAssertEqual(ordered.map(\.name), ["aphex twin", "Squarepusher", ""],
            "case-insensitive alpha order; empty/unknown artist sorts last")
    }

    func testOrderedArtistsAlsoOrdersEachArtistsAlbums() {
        let ordered = LibraryBrowser.orderedArtists(library)
        let aphex = ordered.first { $0.name == "aphex twin" }
        XCTAssertEqual(aphex?.albums.map(\.name), ["Drukqs", "syro"],
            "albums within an artist are ordered too")
    }

    func testOrderedArtistsIsDeterministicAcrossRepeatedCalls() {
        XCTAssertEqual(LibraryBrowser.orderedArtists(library), LibraryBrowser.orderedArtists(library.shuffled()))
    }

    func testFlattenedAlbumsOrdersByAlbumNameAcrossArtists() {
        let flat = LibraryBrowser.flattenedAlbums(library)
        XCTAssertEqual(flat.map(\.album.name), ["Drukqs", "Go Plastic", "Hard Normal Daddy", "syro", ""],
            "case-insensitive alpha order; empty/unknown album sorts last")
        XCTAssertEqual(flat.first { $0.album.name == "syro" }?.artist, "aphex twin")
    }

    func testOrderedGenresIsCaseInsensitiveAlpha() {
        let genres = [
            LibraryGenre(name: "Techno", tracks: 3, bytes: 30),
            LibraryGenre(name: "ambient", tracks: 5, bytes: 50),
        ]
        XCTAssertEqual(LibraryBrowser.orderedGenres(genres).map(\.name), ["ambient", "Techno"])
    }

    // MARK: - Tri-state artist checkbox

    private let squarepusher = LibraryArtist(name: "Squarepusher", albums: [
        LibraryAlbum(name: "Go Plastic", genre: nil, tracks: 10, bytes: 100),
        LibraryAlbum(name: "Hard Normal Daddy", genre: nil, tracks: 9, bytes: 90),
    ])

    func testCheckStateOffWhenNoRulesPresent() {
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: []), .off)
    }

    func testCheckStateOnWhenArtistRulePresent() {
        let checked: Set<SelectionKey> = [.artist(name: "Squarepusher")]
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .on)
    }

    func testCheckStateOnWhenEveryAlbumIndividuallyChecked() {
        let checked: Set<SelectionKey> = [
            .album(artist: "Squarepusher", album: "Go Plastic"),
            .album(artist: "Squarepusher", album: "Hard Normal Daddy"),
        ]
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .on)
    }

    func testCheckStateMixedWhenSomeAlbumsChecked() {
        let checked: Set<SelectionKey> = [.album(artist: "Squarepusher", album: "Go Plastic")]
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .mixed)
    }

    // MARK: - Toggling an artist (checks/unchecks all its albums)

    func testToggleArtistCascadingAddsSingleArtistRule() {
        let checked = LibraryBrowser.toggledArtist(squarepusher, checked: [], style: .cascading)
        XCTAssertEqual(checked, [.artist(name: "Squarepusher")])
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .on)
    }

    func testToggleArtistCascadingOffRemovesEverything() {
        let checkedOn: Set<SelectionKey> = [.artist(name: "Squarepusher")]
        let checked = LibraryBrowser.toggledArtist(squarepusher, checked: checkedOn, style: .cascading)
        XCTAssertEqual(checked, [])
    }

    func testToggleArtistCascadingFromMixedGoesToFullyOn() {
        let mixed: Set<SelectionKey> = [.album(artist: "Squarepusher", album: "Go Plastic")]
        let checked = LibraryBrowser.toggledArtist(squarepusher, checked: mixed, style: .cascading)
        XCTAssertEqual(checked, [.artist(name: "Squarepusher")])
    }

    func testToggleArtistFlatChecksEachAlbumIndividuallyNoArtistRule() {
        let checked = LibraryBrowser.toggledArtist(squarepusher, checked: [], style: .flat)
        XCTAssertEqual(checked, [
            .album(artist: "Squarepusher", album: "Go Plastic"),
            .album(artist: "Squarepusher", album: "Hard Normal Daddy"),
        ], "flat style never synthesizes a future-albums artist rule")
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .on)
    }

    func testToggleArtistFlatOffRemovesAllAlbums() {
        let allOn: Set<SelectionKey> = [
            .album(artist: "Squarepusher", album: "Go Plastic"),
            .album(artist: "Squarepusher", album: "Hard Normal Daddy"),
        ]
        let checked = LibraryBrowser.toggledArtist(squarepusher, checked: allOn, style: .flat)
        XCTAssertEqual(checked, [])
    }

    // MARK: - Toggling an individual album

    func testToggleAlbumCascadingUncheckingUnderArtistRuleExpands() {
        let checkedOn: Set<SelectionKey> = [.artist(name: "Squarepusher")]
        let checked = LibraryBrowser.toggledAlbum(
            artist: "Squarepusher", album: "Go Plastic",
            siblingAlbums: ["Go Plastic", "Hard Normal Daddy"], checked: checkedOn, style: .cascading)
        XCTAssertEqual(checked, [.album(artist: "Squarepusher", album: "Hard Normal Daddy")])
    }

    func testToggleAlbumCascadingCheckingLastAlbumCollapsesToArtistRule() {
        let oneChecked: Set<SelectionKey> = [.album(artist: "Squarepusher", album: "Go Plastic")]
        let checked = LibraryBrowser.toggledAlbum(
            artist: "Squarepusher", album: "Hard Normal Daddy",
            siblingAlbums: ["Go Plastic", "Hard Normal Daddy"], checked: oneChecked, style: .cascading)
        XCTAssertEqual(checked, [.artist(name: "Squarepusher")])
    }

    func testToggleAlbumFlatTogglesOnlyThatAlbum() {
        let checked = LibraryBrowser.toggledAlbum(
            artist: "Squarepusher", album: "Go Plastic",
            siblingAlbums: ["Go Plastic", "Hard Normal Daddy"], checked: [], style: .flat)
        XCTAssertEqual(checked, [.album(artist: "Squarepusher", album: "Go Plastic")])
        XCTAssertEqual(LibraryBrowser.checkState(for: squarepusher, checked: checked), .mixed)
    }

    // MARK: - Toggling a genre

    func testToggleGenreRoundTrips() {
        let checked = LibraryBrowser.toggledGenre("Ambient", checked: [])
        XCTAssertEqual(checked, [.genre(name: "Ambient")])
        XCTAssertEqual(LibraryBrowser.toggledGenre("Ambient", checked: checked), [])
    }

    // MARK: - Case-insensitive matching (fix: Set<SelectionRule> membership
    // is exact-case via synthesized Hashable; the Rust matcher and the
    // deleted SelectionDraft both compare names case-insensitively —
    // `crates/classick/src/selection.rs`'s `a.to_lowercase() ==
    // b.to_lowercase()`. A persisted rule's case need not match the current
    // scan's, so these helpers must reconcile the two.)

    private let radiohead = LibraryArtist(name: "Radiohead", albums: [
        LibraryAlbum(name: "Kid A", genre: nil, tracks: 10, bytes: 100),
        LibraryAlbum(name: "OK Computer", genre: nil, tracks: 12, bytes: 120),
    ])

    func testCheckStateIsCaseInsensitiveForArtistRule() {
        let checked: Set<SelectionKey> = [.artist(name: "radiohead")]
        XCTAssertEqual(LibraryBrowser.checkState(for: radiohead, checked: checked), .on,
            "a lowercase-persisted artist rule must still register as checked for 'Radiohead'")
    }

    func testCheckStateIsCaseInsensitiveForAlbumRules() {
        let checked: Set<SelectionKey> = [
            .album(artist: "RADIOHEAD", album: "kid a"),
            .album(artist: "radiohead", album: "OK COMPUTER"),
        ]
        XCTAssertEqual(LibraryBrowser.checkState(for: radiohead, checked: checked), .on)
    }

    func testToggleArtistOffWithDifferingCaseActuallyRemovesTheRule() {
        // The library's canonical case is "Radiohead"; the persisted rule is
        // lowercase. Toggling the artist off (cascading) must remove the
        // existing differently-cased rule, not leave it behind alongside a
        // no-op.
        let checked: Set<SelectionKey> = [.artist(name: "radiohead")]
        let result = LibraryBrowser.toggledArtist(radiohead, checked: checked, style: .cascading)
        XCTAssertEqual(result, [], "toggling off must remove the differently-cased persisted rule")
    }

    func testToggleArtistOffFlatWithDifferingCaseRemovesAllAlbumRules() {
        let checked: Set<SelectionKey> = [
            .album(artist: "radiohead", album: "kid a"),
            .album(artist: "Radiohead", album: "OK Computer"),
        ]
        let result = LibraryBrowser.toggledArtist(radiohead, checked: checked, style: .flat)
        XCTAssertEqual(result, [], "flat toggle-off must remove differently-cased album rules too")
    }

    func testToggleAlbumOffWithDifferingCaseRemovesTheRuleNotDuplicates() {
        let checked: Set<SelectionKey> = [.album(artist: "RADIOHEAD", album: "KID A")]
        let result = LibraryBrowser.toggledAlbum(
            artist: "Radiohead", album: "Kid A",
            siblingAlbums: ["Kid A", "OK Computer"], checked: checked, style: .flat)
        XCTAssertEqual(result, [], "toggling off must remove the differently-cased persisted rule, not insert a duplicate")
    }

    func testToggleAlbumCascadingCollapseIsCaseInsensitive() {
        // "Kid A" already checked (uppercase artist/lowercase-ish album
        // casing from a persisted rule); checking the last sibling must
        // still collapse to a single artist rule.
        let checked: Set<SelectionKey> = [.album(artist: "radiohead", album: "Kid A")]
        let result = LibraryBrowser.toggledAlbum(
            artist: "Radiohead", album: "OK Computer",
            siblingAlbums: ["Kid A", "OK Computer"], checked: checked, style: .cascading)
        XCTAssertEqual(result, [.artist(name: "Radiohead")])
    }

    func testToggleGenreOffWithDifferingCaseRemovesTheRule() {
        let checked: Set<SelectionKey> = [.genre(name: "AMBIENT")]
        let result = LibraryBrowser.toggledGenre("ambient", checked: checked)
        XCTAssertEqual(result, [], "toggling off must remove the differently-cased persisted rule")
    }

    func testContainsCaseInsensitiveHelperDirectly() {
        let checked: Set<SelectionKey> = [.artist(name: "radiohead")]
        XCTAssertTrue(LibraryBrowser.containsCaseInsensitive(.artist(name: "Radiohead"), in: checked))
        XCTAssertFalse(LibraryBrowser.containsCaseInsensitive(.artist(name: "Aphex Twin"), in: checked))
    }
}
