import SwiftUI

/// One row's selection identity — reuses the existing selection-rule shapes
/// (`SelectionRule`'s `.artist`/`.album`/`.genre` cases, already `Hashable`)
/// so a device's persisted `SelectionState.rules` array converts to/from
/// `Set<SelectionKey>` with no new wire type.
typealias SelectionKey = SelectionRule

/// How `.select` mode resolves a row toggle to `SelectionKey` edits.
enum SelectStyle: Equatable, Sendable {
    /// Checking an artist writes ONE `.artist` rule that also covers
    /// future albums (iTunes intuition); unchecking one album under a
    /// fully-checked artist expands that rule into explicit per-album keys
    /// for the remaining albums. Mirrors `SelectionDraft`'s semantics —
    /// this is the style the device Music page (Task 5) binds to, since a
    /// sync selection is meant to auto-follow future library growth.
    case cascading
    /// Each row is independent; an artist toggle checks/unchecks its
    /// currently-known albums directly and never synthesizes an `.artist`
    /// rule. For pickers that resolve to concrete, existing items — e.g.
    /// the Add Songs picker (Task 7), where "future albums" has no
    /// meaning against a fixed playlist track list.
    case flat
}

/// Reusable library browser: renders the wire library aggregates
/// (artists/albums/genres) as rows, either read-only (`.browse` — the
/// Library page; canonical-surface rule: this mode renders ZERO checkbox
/// affordances) or with a checkbox per row bound to an externally-owned
/// `Set<SelectionKey>` (`.select` — the device Music page, the Add Songs
/// picker). ONE component serves all three surfaces per the restructure
/// plan; do not fork it per page.
///
/// The facet picker itself (segmented control) is NOT owned by this view —
/// each host page renders its own (Library hides `.playlists`; the device
/// Music page shows all four) and passes the chosen `Facet` down.
struct LibraryBrowser: View {
    enum Mode {
        case browse
        case select(checked: Binding<Set<SelectionKey>>, style: SelectStyle)
    }

    enum Facet: String, CaseIterable, Sendable {
        case artists = "Artists"
        case albums = "Albums"
        case genres = "Genres"
        case playlists = "Playlists"
    }

    enum CheckState: Equatable, Sendable { case off, on, mixed }

    /// One row of the flat (ungrouped) Albums facet.
    struct FlatAlbumRow: Equatable, Identifiable, Sendable {
        var artist: String
        var album: LibraryAlbum
        var id: String { "\(artist)\u{0}\(album.name)" }
    }

    var library: LibraryInfo
    var facet: Facet
    var mode: Mode
    var search: String = ""

    var body: some View {
        List {
            switch facet {
            case .artists:
                ForEach(Self.orderedArtists(Self.filteredArtists(library.artists, search: search)), id: \.name) { artist in
                    artistRow(artist)
                }
            case .albums:
                ForEach(Self.filteredFlatAlbums(library.artists, search: search)) { row in
                    flatAlbumRow(row)
                }
            case .genres:
                ForEach(Self.orderedGenres(Self.filteredGenres(library.genres, search: search)), id: \.name) { genre in
                    genreRow(genre)
                }
            case .playlists:
                // Only meaningful on device pages (subscriptions checklist,
                // Task 5), which will pass playlist data this view doesn't
                // have yet. The Library page's facet picker never offers
                // this case in `.browse` mode (canonical-surface rule).
                Text("Playlists sync from the Playlists section.")
                    .foregroundStyle(.secondary)
            }
        }
        .listStyle(.inset)
    }

    // MARK: - Rows

    @ViewBuilder
    private func artistRow(_ artist: LibraryArtist) -> some View {
        let title = artist.name.isEmpty ? "Unknown Artist" : artist.name
        let subtitle = "\(artist.albums.count) album\(artist.albums.count == 1 ? "" : "s")"
        DisclosureGroup {
            ForEach(artist.albums, id: \.name) { album in
                albumRow(artist: artist, album: album)
            }
        } label: {
            switch mode {
            case .browse:
                rowLabel(title: title, subtitle: subtitle, isChecked: false, onToggle: nil)
            case let .select(checked, style):
                let state = Self.checkState(for: artist, checked: checked.wrappedValue)
                rowLabel(title: title, subtitle: subtitle, isChecked: state != .off, isMixed: state == .mixed) {
                    checked.wrappedValue = Self.toggledArtist(artist, checked: checked.wrappedValue, style: style)
                }
            }
        }
    }

    @ViewBuilder
    private func albumRow(artist: LibraryArtist, album: LibraryAlbum) -> some View {
        let title = album.name.isEmpty ? "Unknown Album" : album.name
        let subtitle = "\(album.tracks) track\(album.tracks == 1 ? "" : "s") · \(formatBytes(album.bytes))"
        switch mode {
        case .browse:
            rowLabel(title: title, subtitle: subtitle, isChecked: false, onToggle: nil)
        case let .select(checked, style):
            let key = SelectionKey.album(artist: artist.name, album: album.name)
            let isChecked = checked.wrappedValue.contains(.artist(name: artist.name)) || checked.wrappedValue.contains(key)
            rowLabel(title: title, subtitle: subtitle, isChecked: isChecked) {
                checked.wrappedValue = Self.toggledAlbum(
                    artist: artist.name, album: album.name,
                    siblingAlbums: artist.albums.map(\.name),
                    checked: checked.wrappedValue, style: style)
            }
        }
    }

    @ViewBuilder
    private func flatAlbumRow(_ row: FlatAlbumRow) -> some View {
        let title = row.album.name.isEmpty ? "Unknown Album" : row.album.name
        let subtitle = "\(row.artist.isEmpty ? "Unknown Artist" : row.artist) · \(row.album.tracks) track\(row.album.tracks == 1 ? "" : "s") · \(formatBytes(row.album.bytes))"
        switch mode {
        case .browse:
            rowLabel(title: title, subtitle: subtitle, isChecked: false, onToggle: nil)
        case let .select(checked, style):
            let key = SelectionKey.album(artist: row.artist, album: row.album.name)
            let isChecked = checked.wrappedValue.contains(.artist(name: row.artist)) || checked.wrappedValue.contains(key)
            rowLabel(title: title, subtitle: subtitle, isChecked: isChecked) {
                let siblings = library.artists.first { $0.name == row.artist }?.albums.map(\.name) ?? [row.album.name]
                checked.wrappedValue = Self.toggledAlbum(
                    artist: row.artist, album: row.album.name,
                    siblingAlbums: siblings, checked: checked.wrappedValue, style: style)
            }
        }
    }

    @ViewBuilder
    private func genreRow(_ genre: LibraryGenre) -> some View {
        let title = genre.name.isEmpty ? "No Genre" : genre.name
        let subtitle = "\(genre.tracks) track\(genre.tracks == 1 ? "" : "s") · \(formatBytes(genre.bytes))"
        switch mode {
        case .browse:
            rowLabel(title: title, subtitle: subtitle, isChecked: false, onToggle: nil)
        case let .select(checked, _):
            let key = SelectionKey.genre(name: genre.name)
            rowLabel(title: title, subtitle: subtitle, isChecked: checked.wrappedValue.contains(key)) {
                checked.wrappedValue = Self.toggledGenre(genre.name, checked: checked.wrappedValue)
            }
        }
    }

    /// Shared row chrome. `onToggle == nil` renders NO checkbox at all —
    /// that's the entire mechanism behind the canonical-surface rule: browse
    /// mode never passes a toggle closure, so it is structurally impossible
    /// for a checkbox to render there.
    private func rowLabel(title: String, subtitle: String, isChecked: Bool, isMixed: Bool = false, onToggle: (() -> Void)? = nil) -> some View {
        HStack {
            if let onToggle {
                Toggle(isOn: Binding(get: { isChecked }, set: { _ in onToggle() })) {
                    EmptyView()
                }
                .toggleStyle(.checkbox)
                .labelsHidden()
            }
            Text(title)
            if isMixed {
                Text("–").foregroundStyle(.tint)
            }
            Spacer()
            Text(subtitle).font(.caption).foregroundStyle(.secondary)
        }
    }

    // MARK: - Pure helpers (exposed for tests)

    nonisolated private static func sortKey(_ name: String) -> String {
        name.isEmpty ? "\u{FFFF}" : name.lowercased()
    }

    /// Deterministic display order for the Artists facet: artists
    /// case-insensitive alpha (empty/"Unknown Artist" sorts last), each
    /// artist's own albums ordered the same way.
    nonisolated static func orderedArtists(_ artists: [LibraryArtist]) -> [LibraryArtist] {
        artists
            .map { LibraryArtist(name: $0.name, albums: $0.albums.sorted { sortKey($0.name) < sortKey($1.name) }) }
            .sorted { sortKey($0.name) < sortKey($1.name) }
    }

    nonisolated static func orderedGenres(_ genres: [LibraryGenre]) -> [LibraryGenre] {
        genres.sorted { sortKey($0.name) < sortKey($1.name) }
    }

    /// Flat, ungrouped Albums facet: every album across every artist,
    /// ordered by album name (not grouped by artist — that's what
    /// distinguishes this facet from Artists).
    nonisolated static func flattenedAlbums(_ artists: [LibraryArtist]) -> [FlatAlbumRow] {
        artists
            .flatMap { artist in artist.albums.map { FlatAlbumRow(artist: artist.name, album: $0) } }
            .sorted { sortKey($0.album.name) < sortKey($1.album.name) }
    }

    /// Tri-state artist checkbox: `.on` when an `.artist` rule is present OR
    /// every one of its albums is individually checked, `.off` when none
    /// are, `.mixed` otherwise.
    nonisolated static func checkState(for artist: LibraryArtist, checked: Set<SelectionKey>) -> CheckState {
        if checked.contains(.artist(name: artist.name)) { return .on }
        guard !artist.albums.isEmpty else { return .off }
        let checkedCount = artist.albums.filter { checked.contains(.album(artist: artist.name, album: $0.name)) }.count
        if checkedCount == 0 { return .off }
        return checkedCount == artist.albums.count ? .on : .mixed
    }

    /// Toggling an artist checks/unchecks all of its albums, one pure `Set`
    /// transform. `.cascading` collapses to/from a single `.artist` rule;
    /// `.flat` checks/unchecks each currently-known album explicitly.
    nonisolated static func toggledArtist(_ artist: LibraryArtist, checked: Set<SelectionKey>, style: SelectStyle) -> Set<SelectionKey> {
        var checked = checked
        let albumKeys = artist.albums.map { SelectionKey.album(artist: artist.name, album: $0.name) }
        let state = checkState(for: artist, checked: checked)
        switch style {
        case .cascading:
            switch state {
            case .on:
                checked.remove(.artist(name: artist.name))
                for key in albumKeys { checked.remove(key) }
            case .off, .mixed:
                for key in albumKeys { checked.remove(key) }
                checked.insert(.artist(name: artist.name))
            }
        case .flat:
            if state == .on {
                for key in albumKeys { checked.remove(key) }
            } else {
                for key in albumKeys { checked.insert(key) }
            }
        }
        return checked
    }

    /// Toggling one album. `.cascading` mirrors `SelectionDraft`: unchecking
    /// an album under a whole-artist rule expands it into explicit per-album
    /// rules minus this one; checking the last unchecked sibling collapses
    /// back to one `.artist` rule. `.flat` just flips that one album's key.
    nonisolated static func toggledAlbum(artist: String, album: String, siblingAlbums: [String], checked: Set<SelectionKey>, style: SelectStyle) -> Set<SelectionKey> {
        var checked = checked
        let albumKey = SelectionKey.album(artist: artist, album: album)
        let artistKey = SelectionKey.artist(name: artist)
        switch style {
        case .flat:
            if checked.contains(albumKey) {
                checked.remove(albumKey)
            } else {
                checked.insert(albumKey)
            }
        case .cascading:
            if checked.contains(artistKey) {
                checked.remove(artistKey)
                for sibling in siblingAlbums where sibling != album {
                    checked.insert(.album(artist: artist, album: sibling))
                }
            } else if checked.contains(albumKey) {
                checked.remove(albumKey)
            } else {
                checked.insert(albumKey)
                let allChecked = siblingAlbums.allSatisfy { checked.contains(.album(artist: artist, album: $0)) }
                if allChecked {
                    for sibling in siblingAlbums { checked.remove(.album(artist: artist, album: sibling)) }
                    checked.insert(artistKey)
                }
            }
        }
        return checked
    }

    nonisolated static func toggledGenre(_ name: String, checked: Set<SelectionKey>) -> Set<SelectionKey> {
        var checked = checked
        let key = SelectionKey.genre(name: name)
        if checked.contains(key) {
            checked.remove(key)
        } else {
            checked.insert(key)
        }
        return checked
    }

    // MARK: - Search filtering

    nonisolated static func filteredArtists(_ artists: [LibraryArtist], search: String) -> [LibraryArtist] {
        guard !search.isEmpty else { return artists }
        let q = search.lowercased()
        return artists.compactMap { artist in
            if artist.name.lowercased().contains(q) { return artist }
            let albums = artist.albums.filter { $0.name.lowercased().contains(q) }
            return albums.isEmpty ? nil : LibraryArtist(name: artist.name, albums: albums)
        }
    }

    nonisolated static func filteredGenres(_ genres: [LibraryGenre], search: String) -> [LibraryGenre] {
        guard !search.isEmpty else { return genres }
        let q = search.lowercased()
        return genres.filter { $0.name.lowercased().contains(q) }
    }

    nonisolated static func filteredFlatAlbums(_ artists: [LibraryArtist], search: String) -> [FlatAlbumRow] {
        flattenedAlbums(filteredArtists(artists, search: search))
    }
}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}
