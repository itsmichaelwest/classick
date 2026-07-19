import SwiftUI

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

    /// One row height for every list surface (artists/genres Lists, the
    /// albums LazyVStack, the device playlists checklist, the playlist
    /// editor's track list). List and the manual albums stack size rows by
    /// different rules, so the height is pinned rather than inherited —
    /// switching facets must not change the table rhythm.
    nonisolated static let rowHeight: CGFloat = 32

    var library: LibraryInfo
    var facet: Facet
    var mode: Mode
    var search: String = ""
    var launchNonce: UUID? = nil

    nonisolated static func dragPayload(
        for rule: SelectionRule, summary: String, mode: Mode, launchNonce: UUID
    ) -> LibraryDragPayload? {
        guard case .browse = mode else { return nil }
        return try? LibraryDragPayload.make(
            rule: rule, summary: summary, launchNonce: launchNonce)
    }

    nonisolated static func dragPayload(
        for rule: SelectionRule?, summary: String, mode: Mode, launchNonce: UUID
    ) -> LibraryDragPayload? {
        guard let rule else { return nil }
        return dragPayload(for: rule, summary: summary, mode: mode, launchNonce: launchNonce)
    }

    var body: some View {
        if facet == .albums {
            albumsTable
        } else {
            facetList
        }
    }

    private var facetList: some View {
        List {
            switch facet {
            case .artists:
                ForEach(Self.orderedArtists(Self.filteredArtists(library.artists, search: search)), id: \.name) { artist in
                    artistRow(artist)
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
            case .albums:
                EmptyView() // rendered by albumsTable
            }
        }
        .listStyle(.inset)
        .environment(\.defaultMinListRowHeight, Self.rowHeight)
    }

    /// The Albums facet (design frame 3:3773: albums grouped under artist
    /// headers) lives in a `ScrollView` + `LazyVStack(pinnedViews:
    /// [.sectionHeaders])` rather than a `List`: List section headers do
    /// NOT stick on scroll on current macOS (verified on both toolbar- and
    /// safeAreaBar-hosted pages), and pinned headers are the point of the
    /// grouping. `pinnedViews` pins by contract.
    private var albumsTable: some View {
        ScrollView {
            albumsTableContent
        }
        // A bare ScrollView isn't reliably picked up as the page's primary
        // scroll view on current macOS (List is), so the toolbar never got
        // its scroll-edge background and rows bled through the chrome above
        // the pinned headers. Declaring the edge effect explicitly engages
        // it — the hard style, per the design's own annotation.
        .modifier(HardTopScrollEdgeIfAvailable())
    }

    private var albumsTableContent: some View {
        LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
            ForEach(Self.orderedArtists(Self.filteredArtists(library.artists, search: search)), id: \.name) { artist in
                Section {
                    ForEach(artist.albums, id: \.name) { album in
                        VStack(spacing: 0) {
                            albumRow(artist: artist, album: album)
                                .padding(.horizontal, 16)
                                .frame(minHeight: Self.rowHeight)
                            Divider()
                                .padding(.leading, 16)
                        }
                    }
                } header: {
                    // Opaque background so rows disappear UNDER the
                    // pinned header instead of showing through it.
                    Text(artist.name.isEmpty ? "Unknown Artist" : artist.name)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 5)
                        .background(.background)
                }
            }
        }
    }

    // MARK: - Rows

    /// Artist row: expandable to its album rows (restored by user decision
    /// after a flat interlude). The artist checkbox shows the mixed
    /// (intermediate) state when only some albums are checked; toggling it
    /// checks/unchecks all of them, and the disclosure exposes per-album
    /// checkboxes without leaving the Artists facet.
    @ViewBuilder
    private func artistRow(_ artist: LibraryArtist) -> some View {
        let title = artist.name.isEmpty ? "Unknown Artist" : artist.name
        let totalBytes = artist.albums.reduce(UInt64(0)) { $0 + $1.bytes }
        let columns = ["\(artist.albums.count) album\(artist.albums.count == 1 ? "" : "s")", formatBytes(totalBytes)]
        DisclosureGroup {
            ForEach(artist.albums, id: \.name) { album in
                albumRow(artist: artist, album: album)
            }
        } label: {
            switch mode {
            case .browse:
                rowLabel(title: title, columns: columns, isChecked: false, onToggle: nil)
                    .libraryDragSource(
                        payload(for: .artist(name: artist.name), summary: title),
                        systemImage: "person.fill")
            case let .select(checked, style):
                let state = Self.checkState(for: artist, checked: checked.wrappedValue)
                rowLabel(title: title, columns: columns, isChecked: state != .off, isMixed: state == .mixed) {
                    checked.wrappedValue = Self.toggledArtist(artist, checked: checked.wrappedValue, style: style)
                }
            }
        }
    }

    @ViewBuilder
    private func albumRow(artist: LibraryArtist, album: LibraryAlbum) -> some View {
        let title = album.name.isEmpty ? "Unknown Album" : album.name
        let columns = [trackCountText(album.tracks), formatBytes(album.bytes)]
        switch mode {
        case .browse:
            rowLabel(title: title, columns: columns, isChecked: false, onToggle: nil)
                .libraryDragSource(
                    payload(for: .album(artist: artist.name, album: album.name), summary: title),
                    systemImage: "opticaldisc.fill")
        case let .select(checked, style):
            let key = SelectionKey.album(artist: artist.name, album: album.name)
            let isChecked = Self.containsCaseInsensitive(.artist(name: artist.name), in: checked.wrappedValue)
                || Self.containsCaseInsensitive(key, in: checked.wrappedValue)
            rowLabel(title: title, columns: columns, isChecked: isChecked) {
                checked.wrappedValue = Self.toggledAlbum(
                    artist: artist.name, album: album.name,
                    siblingAlbums: artist.albums.map(\.name),
                    checked: checked.wrappedValue, style: style)
            }
        }
    }

    /// Genre row: expandable to the albums carrying that genre tag. The
    /// header checkbox toggles the whole-genre rule; child album rows edit
    /// album-level rules (and display as checked when covered by the genre
    /// rule, their artist's rule, or their own).
    @ViewBuilder
    private func genreRow(_ genre: LibraryGenre) -> some View {
        let title = genre.name.isEmpty ? "No Genre" : genre.name
        let columns = [trackCountText(genre.tracks), formatBytes(genre.bytes)]
        let entries = Self.albums(inGenre: genre.name, of: library.artists)
        DisclosureGroup {
            ForEach(entries, id: \.id) { entry in
                genreAlbumRow(entry: entry, genre: genre.name, genreEntries: entries)
            }
        } label: {
            switch mode {
            case .browse:
                rowLabel(title: title, columns: columns, isChecked: false, onToggle: nil)
                    .libraryDragSource(
                        payload(for: .genre(name: genre.name), summary: title),
                        systemImage: "tag.fill")
            case let .select(checked, _):
                let state = Self.genreCheckState(genre.name, artists: library.artists, checked: checked.wrappedValue)
                rowLabel(title: title, columns: columns, isChecked: state != .off, isMixed: state == .mixed) {
                    checked.wrappedValue = Self.toggledGenreHeader(genre.name, artists: library.artists, checked: checked.wrappedValue)
                }
            }
        }
    }

    @ViewBuilder
    private func genreAlbumRow(entry: GenreAlbumEntry, genre: String, genreEntries: [GenreAlbumEntry]) -> some View {
        let title = entry.album.name.isEmpty ? "Unknown Album" : entry.album.name
        let subtitleArtist = entry.artistName.isEmpty ? "Unknown Artist" : entry.artistName
        let columns = [trackCountText(entry.album.tracks), formatBytes(entry.album.bytes)]
        switch mode {
        case .browse:
            rowLabel(title: "\(title) — \(subtitleArtist)", columns: columns, isChecked: false, onToggle: nil)
                .libraryDragSource(
                    payload(
                        for: .album(artist: entry.artistName, album: entry.album.name),
                        summary: title),
                    systemImage: "opticaldisc.fill")
        case let .select(checked, style):
            let key = SelectionKey.album(artist: entry.artistName, album: entry.album.name)
            let isChecked = Self.containsCaseInsensitive(.genre(name: genre), in: checked.wrappedValue)
                || Self.containsCaseInsensitive(.artist(name: entry.artistName), in: checked.wrappedValue)
                || Self.containsCaseInsensitive(key, in: checked.wrappedValue)
            rowLabel(title: "\(title) — \(subtitleArtist)", columns: columns, isChecked: isChecked) {
                checked.wrappedValue = Self.toggledGenreAlbum(
                    entry: entry, genre: genre, genreEntries: genreEntries,
                    checked: checked.wrappedValue, style: style)
            }
        }
    }

    private func trackCountText(_ n: Int) -> String {
        "\(n) track\(n == 1 ? "" : "s")"
    }

    private func payload(for rule: SelectionRule, summary: String) -> LibraryDragPayload? {
        guard let launchNonce else { return nil }
        return Self.dragPayload(
            for: rule, summary: summary, mode: mode, launchNonce: launchNonce)
    }

    /// Shared row chrome. `onToggle == nil` renders NO checkbox at all —
    /// that's the entire mechanism behind the canonical-surface rule: browse
    /// mode never passes a toggle closure, so it is structurally impossible
    /// for a checkbox to render there.
    ///
    /// A partially-selected parent (some albums checked) renders the REAL
    /// system mixed checkbox (blue square with a minus) via
    /// `MixedStateCheckbox`, not a checked box with a dash bolted on.
    ///
    /// Trailing metadata renders as fixed-minimum-width, right-aligned
    /// COLUMNS (tracks/albums count, then size) so values line up down the
    /// list like a table — not one concatenated string ragged against the
    /// row edge.
    private func rowLabel(title: String, columns: [String], isChecked: Bool, isMixed: Bool = false, onToggle: (() -> Void)? = nil) -> some View {
        HStack(spacing: 8) {
            if let onToggle {
                MixedStateCheckbox(
                    state: isMixed ? .mixed : (isChecked ? .on : .off),
                    onToggle: onToggle)
            }
            Text(title)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 12)
            ForEach(Array(columns.enumerated()), id: \.offset) { _, column in
                Text(column)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .frame(minWidth: 84, alignment: .trailing)
            }
        }
    }

}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}

/// The system checkbox with real mixed-state support — AppKit's `NSButton`
/// (`allowsMixedState`), which SwiftUI's `Toggle` has never exposed. Not a
/// custom control: this is the exact checkbox Finder/Mail render for
/// partial selections. State is one-way from SwiftUI: a click calls
/// `onToggle`, the model recomputes, and `updateNSView` writes the
/// resulting state back — overriding whatever NSButton's own click cycle
/// (which can pass THROUGH mixed) chose, so users can toggle but never
/// manually park a row on "mixed".
private struct MixedStateCheckbox: NSViewRepresentable {
    var state: LibraryBrowser.CheckState
    var onToggle: () -> Void

    func makeNSView(context: Context) -> NSButton {
        let button = NSButton(checkboxWithTitle: "", target: context.coordinator, action: #selector(Coordinator.clicked))
        button.allowsMixedState = true
        button.setContentHuggingPriority(.required, for: .horizontal)
        return button
    }

    func updateNSView(_ button: NSButton, context: Context) {
        context.coordinator.onToggle = onToggle
        switch state {
        case .on: button.state = .on
        case .off: button.state = .off
        case .mixed: button.state = .mixed
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    @MainActor
    final class Coordinator: NSObject {
        var onToggle: () -> Void = {}
        @objc func clicked(_ sender: NSButton) { onToggle() }
    }
}

#if DEBUG
/// `.select` mode's `checked` is a `Binding<Set<SelectionKey>>` — this host
/// owns the `@State` a preview needs to hand one in, seeded with a few rows
/// pre-checked so the tri-state artist checkbox (`.mixed`) is visible too.
private struct LibraryBrowserSelectPreviewHost: View {
    var style: SelectStyle
    @State private var checked: Set<SelectionKey> = [
        .artist(name: PreviewFixtures.boardsOfCanada.name),
        .album(artist: PreviewFixtures.radiohead.name, album: "OK Computer"),
    ]

    var body: some View {
        LibraryBrowser(
            library: PreviewFixtures.richLibrary, facet: .artists,
            mode: .select(checked: $checked, style: style))
    }
}

#Preview("Browse artists") {
    LibraryBrowser(library: PreviewFixtures.richLibrary, facet: .artists, mode: .browse)
        .frame(width: 420, height: 500)
}

#Preview("Select cascading") {
    LibraryBrowserSelectPreviewHost(style: .cascading)
        .frame(width: 420, height: 500)
}

#Preview("Select flat") {
    LibraryBrowserSelectPreviewHost(style: .flat)
        .frame(width: 420, height: 500)
}

#Preview("Genres") {
    LibraryBrowser(library: PreviewFixtures.richLibrary, facet: .genres, mode: .browse)
        .frame(width: 420, height: 500)
}
#endif

/// Explicitly declares the hard top scroll-edge effect (macOS 26+). No-op on
/// macOS 15, where the toolbar manages its own on-scroll background.
private struct HardTopScrollEdgeIfAvailable: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            content.scrollEdgeEffectStyle(.hard, for: .top)
        } else {
            content
        }
    }
}
