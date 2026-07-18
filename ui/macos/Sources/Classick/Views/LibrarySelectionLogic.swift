typealias SelectionKey = SelectionRule

extension SelectionRule {
    fileprivate func matchesCaseInsensitive(_ other: SelectionRule) -> Bool {
        switch (self, other) {
        case let (.artist(a), .artist(b)):
            return a.lowercased() == b.lowercased()
        case let (.album(a1, al1), .album(a2, al2)):
            return a1.lowercased() == a2.lowercased() && al1.lowercased() == al2.lowercased()
        case let (.genre(a), .genre(b)):
            return a.lowercased() == b.lowercased()
        default:
            return false
        }
    }
}

enum SelectStyle: Equatable, Sendable {
    case cascading
    case flat
}

extension LibraryBrowser {
    nonisolated private static func sortKey(_ name: String) -> String {
        name.isEmpty ? "\u{FFFF}" : name.lowercased()
    }

    nonisolated static func orderedArtists(_ artists: [LibraryArtist]) -> [LibraryArtist] {
        artists
            .map { LibraryArtist(name: $0.name, albums: $0.albums.sorted { sortKey($0.name) < sortKey($1.name) }) }
            .sorted { sortKey($0.name) < sortKey($1.name) }
    }

    nonisolated static func orderedGenres(_ genres: [LibraryGenre]) -> [LibraryGenre] {
        genres.sorted { sortKey($0.name) < sortKey($1.name) }
    }

    struct GenreAlbumEntry: Equatable, Sendable {
        var artistName: String
        var album: LibraryAlbum
        var siblingAlbums: [String]
        var id: String { "\(artistName)\u{0}\(album.name)" }
    }

    nonisolated static func albums(inGenre genre: String, of artists: [LibraryArtist]) -> [GenreAlbumEntry] {
        orderedArtists(artists).flatMap { artist in
            artist.albums
                .filter { ($0.genre ?? "").lowercased() == genre.lowercased() }
                .map { GenreAlbumEntry(artistName: artist.name, album: $0, siblingAlbums: artist.albums.map(\.name)) }
        }
    }

    nonisolated static func checkState(for artist: LibraryArtist, checked: Set<SelectionKey>) -> CheckState {
        if containsCaseInsensitive(.artist(name: artist.name), in: checked) { return .on }
        guard !artist.albums.isEmpty else { return .off }
        let checkedCount = artist.albums.filter {
            containsCaseInsensitive(.album(artist: artist.name, album: $0.name), in: checked)
        }.count
        if checkedCount == 0 { return .off }
        return checkedCount == artist.albums.count ? .on : .mixed
    }

    nonisolated static func toggledArtist(
        _ artist: LibraryArtist,
        checked: Set<SelectionKey>,
        style: SelectStyle
    ) -> Set<SelectionKey> {
        var checked = checked
        let albumKeys = artist.albums.map { SelectionKey.album(artist: artist.name, album: $0.name) }
        let state = checkState(for: artist, checked: checked)
        switch style {
        case .cascading:
            switch state {
            case .on:
                removeCaseInsensitive(.artist(name: artist.name), from: &checked)
                for key in albumKeys { removeCaseInsensitive(key, from: &checked) }
            case .off, .mixed:
                for key in albumKeys { removeCaseInsensitive(key, from: &checked) }
                checked.insert(.artist(name: artist.name))
            }
        case .flat:
            if state == .on {
                for key in albumKeys { removeCaseInsensitive(key, from: &checked) }
            } else {
                for key in albumKeys where !containsCaseInsensitive(key, in: checked) {
                    checked.insert(key)
                }
            }
        }
        return checked
    }

    nonisolated static func toggledAlbum(
        artist: String,
        album: String,
        siblingAlbums: [String],
        checked: Set<SelectionKey>,
        style: SelectStyle
    ) -> Set<SelectionKey> {
        var checked = checked
        let albumKey = SelectionKey.album(artist: artist, album: album)
        let artistKey = SelectionKey.artist(name: artist)
        switch style {
        case .flat:
            if containsCaseInsensitive(albumKey, in: checked) {
                removeCaseInsensitive(albumKey, from: &checked)
            } else {
                checked.insert(albumKey)
            }
        case .cascading:
            if containsCaseInsensitive(artistKey, in: checked) {
                expandArtistRuleExcluding(
                    artist: artist, excludedAlbum: album,
                    siblingAlbums: siblingAlbums, checked: &checked)
            } else if containsCaseInsensitive(albumKey, in: checked) {
                removeCaseInsensitive(albumKey, from: &checked)
            } else {
                checked.insert(albumKey)
                let allChecked = siblingAlbums.allSatisfy {
                    containsCaseInsensitive(.album(artist: artist, album: $0), in: checked)
                }
                if allChecked {
                    for sibling in siblingAlbums {
                        removeCaseInsensitive(.album(artist: artist, album: sibling), from: &checked)
                    }
                    checked.insert(artistKey)
                }
            }
        }
        return checked
    }

    nonisolated static func toggledGenreAlbum(
        entry: GenreAlbumEntry,
        genre: String,
        genreEntries: [GenreAlbumEntry],
        checked: Set<SelectionKey>,
        style: SelectStyle
    ) -> Set<SelectionKey> {
        let albumKey = SelectionKey.album(artist: entry.artistName, album: entry.album.name)
        let genreKey = SelectionKey.genre(name: genre)
        let artistKey = SelectionKey.artist(name: entry.artistName)
        let isCoveredByGenre = containsCaseInsensitive(genreKey, in: checked)
        let isCoveredByArtist = containsCaseInsensitive(artistKey, in: checked)

        guard isCoveredByGenre || isCoveredByArtist else {
            return toggledAlbum(
                artist: entry.artistName, album: entry.album.name,
                siblingAlbums: entry.siblingAlbums, checked: checked, style: style)
        }

        var result = checked
        if isCoveredByGenre {
            removeCaseInsensitive(genreKey, from: &result)
            for other in genreEntries where other.id != entry.id {
                result.insert(.album(artist: other.artistName, album: other.album.name))
            }
        }
        if isCoveredByArtist {
            expandArtistRuleExcluding(
                artist: entry.artistName, excludedAlbum: entry.album.name,
                siblingAlbums: entry.siblingAlbums, checked: &result)
        }
        removeCaseInsensitive(albumKey, from: &result)
        return result
    }

    nonisolated static func toggledGenre(_ name: String, checked: Set<SelectionKey>) -> Set<SelectionKey> {
        var checked = checked
        let key = SelectionKey.genre(name: name)
        if containsCaseInsensitive(key, in: checked) {
            removeCaseInsensitive(key, from: &checked)
        } else {
            checked.insert(key)
        }
        return checked
    }

    nonisolated static func genreCheckState(
        _ genre: String,
        artists: [LibraryArtist],
        checked: Set<SelectionKey>
    ) -> CheckState {
        if containsCaseInsensitive(.genre(name: genre), in: checked) { return .on }
        let entries = albums(inGenre: genre, of: artists)
        guard !entries.isEmpty else { return .off }
        let covered = entries.filter {
            containsCaseInsensitive(.artist(name: $0.artistName), in: checked)
                || containsCaseInsensitive(.album(artist: $0.artistName, album: $0.album.name), in: checked)
        }.count
        if covered == 0 { return .off }
        return covered == entries.count ? .on : .mixed
    }

    nonisolated static func toggledGenreHeader(
        _ genre: String,
        artists: [LibraryArtist],
        checked: Set<SelectionKey>
    ) -> Set<SelectionKey> {
        var result = checked
        let entries = albums(inGenre: genre, of: artists)
        switch genreCheckState(genre, artists: artists, checked: checked) {
        case .on:
            removeCaseInsensitive(.genre(name: genre), from: &result)
            let grouped = Dictionary(grouping: entries, by: \.artistName)
            for (artistName, genreAlbums) in grouped {
                if containsCaseInsensitive(.artist(name: artistName), in: result),
                   let artist = artists.first(where: { $0.name.lowercased() == artistName.lowercased() }) {
                    removeCaseInsensitive(.artist(name: artistName), from: &result)
                    let genreNames = Set(genreAlbums.map { $0.album.name.lowercased() })
                    for album in artist.albums where !genreNames.contains(album.name.lowercased()) {
                        result.insert(.album(artist: artist.name, album: album.name))
                    }
                }
            }
            for entry in entries {
                removeCaseInsensitive(.album(artist: entry.artistName, album: entry.album.name), from: &result)
            }
        case .off, .mixed:
            result.insert(.genre(name: genre))
        }
        return result
    }

    nonisolated static func containsCaseInsensitive(
        _ key: SelectionKey,
        in set: Set<SelectionKey>
    ) -> Bool {
        set.contains { $0.matchesCaseInsensitive(key) }
    }

    nonisolated static func removeCaseInsensitive(
        _ key: SelectionKey,
        from set: inout Set<SelectionKey>
    ) {
        set = set.filter { !$0.matchesCaseInsensitive(key) }
    }

    nonisolated static func filteredArtists(_ artists: [LibraryArtist], search: String) -> [LibraryArtist] {
        guard !search.isEmpty else { return artists }
        let query = search.lowercased()
        return artists.compactMap { artist in
            if artist.name.lowercased().contains(query) { return artist }
            let albums = artist.albums.filter { $0.name.lowercased().contains(query) }
            return albums.isEmpty ? nil : LibraryArtist(name: artist.name, albums: albums)
        }
    }

    nonisolated static func filteredGenres(_ genres: [LibraryGenre], search: String) -> [LibraryGenre] {
        guard !search.isEmpty else { return genres }
        let query = search.lowercased()
        return genres.filter { $0.name.lowercased().contains(query) }
    }

    nonisolated private static func expandArtistRuleExcluding(
        artist: String,
        excludedAlbum: String,
        siblingAlbums: [String],
        checked: inout Set<SelectionKey>
    ) {
        removeCaseInsensitive(.artist(name: artist), from: &checked)
        for sibling in siblingAlbums where sibling.lowercased() != excludedAlbum.lowercased() {
            checked.insert(.album(artist: artist, album: sibling))
        }
    }
}
