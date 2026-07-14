import XCTest
@testable import Classick

final class SelectionDraftTests: XCTestCase {
    func testToggleArtistAddsAndRemovesArtistRule() {
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleArtist("Aphex Twin", albums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.artist(name: "Aphex Twin")])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .on)
        d.toggleArtist("Aphex Twin", albums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .off)
    }

    func testAlbumSubsetShowsMixedArtistState() {
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleAlbum(artist: "Aphex Twin", album: "Drukqs", siblingAlbums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.album(artist: "Aphex Twin", album: "Drukqs")])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .mixed)
        XCTAssertTrue(d.albumIsChecked(artist: "Aphex Twin", album: "Drukqs"))
        XCTAssertFalse(d.albumIsChecked(artist: "Aphex Twin", album: "Syro"))
    }

    func testCheckingLastAlbumCollapsesToArtistRule() {
        // iTunes intuition: hand-checking every album == checking the artist,
        // which auto-includes FUTURE albums too. Deliberate & documented.
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleAlbum(artist: "Aphex Twin", album: "Drukqs", siblingAlbums: ["Drukqs", "Syro"])
        d.toggleAlbum(artist: "Aphex Twin", album: "Syro", siblingAlbums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.artist(name: "Aphex Twin")],
            "all albums checked must collapse to one artist rule")
    }

    func testUncheckingAlbumUnderArtistRuleExpands() {
        var d = SelectionDraft(mode: .include, rules: [.artist(name: "Aphex Twin")])
        d.toggleAlbum(artist: "Aphex Twin", album: "Syro", siblingAlbums: ["Drukqs", "Syro", "SAW II"])
        XCTAssertEqual(Set(d.rules), Set([
            .album(artist: "Aphex Twin", album: "Drukqs"),
            .album(artist: "Aphex Twin", album: "SAW II"),
        ]), "artist rule expands into explicit albums minus the unchecked one")
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro", "SAW II"]), .mixed)
    }

    func testGenreToggleRoundTrips() {
        var d = SelectionDraft(mode: .exclude, rules: [])
        d.toggleGenre("Podcast")
        XCTAssertTrue(d.genreIsChecked("Podcast"))
        XCTAssertTrue(d.genreIsChecked("podcast"), "case-insensitive, mirrors the Rust matcher")
        d.toggleGenre("PODCAST")
        XCTAssertFalse(d.genreIsChecked("Podcast"))
    }

    func testModeSwitchKeepsRules() {
        var d = SelectionDraft(mode: .include, rules: [.genre(name: "Ambient")])
        d.mode = .exclude
        XCTAssertEqual(d.rules, [.genre(name: "Ambient")],
            "flipping mode preserves checkbox state; only the meaning flips")
    }
}
