import Foundation

/// The Choose Music window's in-memory draft of {mode, rules}. Pure value
/// logic — no I/O, no daemon — so the tri-state/collapse behavior is fully
/// unit-testable. Name comparisons are case-insensitive to mirror the Rust
/// matcher (crates/classick/src/selection.rs).
struct SelectionDraft: Equatable, Sendable {
    var mode: SelectionMode
    var rules: [SelectionRule]

    enum CheckState: Equatable { case off, on, mixed }

    private func hasArtistRule(_ artist: String) -> Bool {
        rules.contains {
            if case let .artist(name) = $0 { return name.lowercased() == artist.lowercased() }
            return false
        }
    }

    func albumIsChecked(artist: String, album: String) -> Bool {
        if hasArtistRule(artist) { return true }
        return rules.contains {
            if case let .album(a, al) = $0 {
                return a.lowercased() == artist.lowercased() && al.lowercased() == album.lowercased()
            }
            return false
        }
    }

    func artistState(_ artist: String, albums: [String]) -> CheckState {
        if hasArtistRule(artist) { return .on }
        let checked = albums.filter { albumIsChecked(artist: artist, album: $0) }.count
        if checked == 0 { return .off }
        return checked == albums.count ? .on : .mixed
    }

    func genreIsChecked(_ name: String) -> Bool {
        rules.contains {
            if case let .genre(n) = $0 { return n.lowercased() == name.lowercased() }
            return false
        }
    }

    mutating func toggleArtist(_ artist: String, albums: [String]) {
        switch artistState(artist, albums: albums) {
        case .on:
            removeArtistAndAlbumRules(artist: artist)
        case .off, .mixed:
            removeArtistAndAlbumRules(artist: artist)
            rules.append(.artist(name: artist))
        }
    }

    mutating func toggleAlbum(artist: String, album: String, siblingAlbums: [String]) {
        if hasArtistRule(artist) {
            // Unchecking one album under a whole-artist check: expand the
            // artist rule into explicit album rules minus this one.
            removeArtistAndAlbumRules(artist: artist)
            for sibling in siblingAlbums where sibling.lowercased() != album.lowercased() {
                rules.append(.album(artist: artist, album: sibling))
            }
            return
        }
        if albumIsChecked(artist: artist, album: album) {
            rules.removeAll {
                if case let .album(a, al) = $0 {
                    return a.lowercased() == artist.lowercased() && al.lowercased() == album.lowercased()
                }
                return false
            }
        } else {
            rules.append(.album(artist: artist, album: album))
            // Collapse: every album now checked -> one artist rule, which
            // also auto-includes future albums (iTunes intuition).
            let allChecked = siblingAlbums.allSatisfy { albumIsChecked(artist: artist, album: $0) }
            if allChecked {
                removeArtistAndAlbumRules(artist: artist)
                rules.append(.artist(name: artist))
            }
        }
    }

    mutating func toggleGenre(_ name: String) {
        if genreIsChecked(name) {
            rules.removeAll {
                if case let .genre(n) = $0 { return n.lowercased() == name.lowercased() }
                return false
            }
        } else {
            rules.append(.genre(name: name))
        }
    }

    private mutating func removeArtistAndAlbumRules(artist: String) {
        rules.removeAll {
            switch $0 {
            case let .artist(name): return name.lowercased() == artist.lowercased()
            case let .album(a, _): return a.lowercased() == artist.lowercased()
            case .genre: return false
            }
        }
    }
}
