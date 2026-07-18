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
}
