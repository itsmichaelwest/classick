import SwiftUI

/// The Choose Music browser: mode picker + Artists/Genres tabs + outline
/// checkboxes + live impact footer. Edits a local SelectionDraft; nothing
/// persists until Save. Layout per the approved design (spec §5).
struct ChooseMusicWindow: View {
    var model: AppModel
    var onAppear: () -> Void
    var onScan: () -> Void
    var onPreview: (SelectionMode, [SelectionRule]) -> Void
    var onSave: (SelectionMode, [SelectionRule]) -> Void
    var onClose: () -> Void

    @State private var draft = SelectionDraft(mode: .all, rules: [])
    @State private var seededFromModel = false
    @State private var tab: Tab = .artists
    @State private var search = ""
    @State private var previewTask: Task<Void, Never>?

    enum Tab: String, CaseIterable { case artists = "Artists", genres = "Genres" }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .onAppear {
            // Seed from an already-known selection (the post-hello get_selection
            // usually landed before this window opened, so .onChange below won't
            // fire for it). The .onChange covers the arrives-after-open case.
            seedDraftIfNeeded()
            onAppear()  // sends get_library + get_selection
        }
        .onChange(of: model.selection) { _, _ in
            // Seed the draft ONCE from the persisted selection; later
            // selection_update echoes (e.g. our own save) must not clobber
            // in-progress edits.
            seedDraftIfNeeded()
        }
        .onChange(of: draft) { _, d in
            schedulePreview(d)
        }
    }

    private var header: some View {
        VStack(spacing: 8) {
            // The mode picker must stay enabled at all times — it's how the
            // user leaves "Entire library". Only the browser controls below
            // gray out in All mode (spec §5: gray the browser, keep state).
            Picker("Sync", selection: $draft.mode) {
                Text("Entire library").tag(SelectionMode.all)
                Text("Only selected").tag(SelectionMode.include)
                Text("All except selected").tag(SelectionMode.exclude)
            }
            .pickerStyle(.segmented)
            if draft.mode == .exclude {
                Text("Checked items will NOT be synced.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text("Checked artists include their future albums.")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Picker("", selection: $tab) {
                    ForEach(Tab.allCases, id: \.self) { Text($0.rawValue) }
                }
                .pickerStyle(.segmented)
                .frame(width: 180)
                TextField("Search", text: $search)
                    .textFieldStyle(.roundedBorder)
            }
            .disabled(draft.mode == .all)  // grayed out, state kept (spec §5)
        }
        .padding(12)
    }

    @ViewBuilder
    private var content: some View {
        if let library = model.library, library.scannedAtUnixSecs != nil {
            browser(library)
        } else {
            emptyState
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Spacer()
            Text("Classick needs to read your library's tags once")
                .font(.headline)
            if case let .scanning(current, total) = model.phase {
                ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
                    .frame(maxWidth: 260)
                Text("Scanning… \(current) of \(total)")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                Button("Scan Library", action: onScan)
                    .keyboardShortcut(.defaultAction)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func browser(_ library: LibraryInfo) -> some View {
        List {
            switch tab {
            case .artists:
                ForEach(filteredArtists(library), id: \.name) { artist in
                    artistRow(artist)
                }
            case .genres:
                ForEach(filteredGenres(library), id: \.name) { genre in
                    genreRow(genre)
                }
            }
        }
        .listStyle(.inset)
        .disabled(draft.mode == .all)
    }

    private func artistRow(_ artist: LibraryArtist) -> some View {
        let albumNames = artist.albums.map(\.name)
        return DisclosureGroup {
            ForEach(artist.albums, id: \.name) { album in
                Toggle(isOn: Binding(
                    get: { draft.albumIsChecked(artist: artist.name, album: album.name) },
                    set: { _ in draft.toggleAlbum(artist: artist.name, album: album.name, siblingAlbums: albumNames) }
                )) {
                    HStack {
                        Text(album.name.isEmpty ? "Unknown Album" : album.name)
                        Spacer()
                        Text("\(album.tracks) tracks · \(formatBytes(album.bytes))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        } label: {
            Toggle(isOn: Binding(
                get: { draft.artistState(artist.name, albums: albumNames) != .off },
                set: { _ in draft.toggleArtist(artist.name, albums: albumNames) }
            )) {
                HStack {
                    Text(artist.name.isEmpty ? "Unknown Artist" : artist.name)
                        .fontWeight(.medium)
                    if draft.artistState(artist.name, albums: albumNames) == .mixed {
                        Text("–").foregroundStyle(.tint)  // mixed marker
                    }
                    Spacer()
                    Text("\(artist.albums.count) albums")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func genreRow(_ genre: LibraryGenre) -> some View {
        Toggle(isOn: Binding(
            get: { draft.genreIsChecked(genre.name) },
            set: { _ in draft.toggleGenre(genre.name) }
        )) {
            HStack {
                Text(genre.name.isEmpty ? "No Genre" : genre.name)
                Spacer()
                Text("\(genre.tracks) tracks · \(formatBytes(genre.bytes))")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                if let p = model.selectionPreview, draft.mode != .all {
                    Text("\(p.selectedTracks) of \(model.library?.totalTracks ?? 0) tracks · ~\(formatBytes(p.selectedBytes))")
                    Text("next sync: +\(p.adds) / −\(p.removes)")
                        .foregroundStyle(.secondary)
                } else if let lib = model.library {
                    Text("\(lib.totalTracks) tracks · \(formatBytes(lib.totalBytes))")
                }
                Spacer()
                if let scanned = model.library?.scannedAtUnixSecs {
                    Text("Scanned \(relativeDate(scanned))")
                        .foregroundStyle(.secondary)
                    Button("Rescan", action: onScan)
                        .disabled(isBusy)
                }
            }
            // Capacity bar vs the connected iPod (spec §5): warn — don't
            // block Save — when the selection won't fit; sync handles
            // disk-full anyway.
            if let storage = model.deviceStorage {
                let selected = selectedBytesForBar
                let over = selected > UInt64(storage.total)
                ProgressView(value: min(Double(selected), Double(storage.total)),
                             total: Double(storage.total))
                    .tint(over ? .red : .accentColor)
                if over {
                    Text("Selection (~\(formatBytes(selected))) exceeds this iPod's capacity (\(formatBytes(UInt64(storage.total)))).")
                        .font(.caption).foregroundStyle(.red)
                }
            }
            HStack {
                Spacer()
                Button("Cancel", action: onClose)
                Button("Save") {
                    onSave(draft.mode, draft.rules)
                    onClose()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .font(.callout)
        .padding(12)
    }

    /// Bytes driving the capacity bar: preview when a filter is active,
    /// whole library otherwise. Source bytes — an estimate of on-iPod size.
    private var selectedBytesForBar: UInt64 {
        if draft.mode != .all, let p = model.selectionPreview { return p.selectedBytes }
        return model.library?.totalBytes ?? 0
    }

    /// Busy = daemon is scanning or syncing (Rescan would be dropped anyway).
    private var isBusy: Bool {
        switch model.phase {
        case .scanning, .syncing: return true
        default: return false
        }
    }

    private func relativeDate(_ unixSecs: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unixSecs))
        return date.formatted(.relative(presentation: .named))
    }

    /// Seed the local draft from the persisted selection exactly once, so
    /// in-progress edits are never clobbered by later selection_update echoes.
    private func seedDraftIfNeeded() {
        guard !seededFromModel, let sel = model.selection else { return }
        draft = SelectionDraft(mode: sel.mode, rules: sel.rules)
        seededFromModel = true
    }

    private func schedulePreview(_ d: SelectionDraft) {
        previewTask?.cancel()
        guard d.mode != .all else { return }
        previewTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            onPreview(d.mode, d.rules)
        }
    }

    private func filteredArtists(_ library: LibraryInfo) -> [LibraryArtist] {
        guard !search.isEmpty else { return library.artists }
        let q = search.lowercased()
        return library.artists.compactMap { artist in
            if artist.name.lowercased().contains(q) { return artist }
            let albums = artist.albums.filter { $0.name.lowercased().contains(q) }
            return albums.isEmpty ? nil : LibraryArtist(name: artist.name, albums: albums)
        }
    }

    private func filteredGenres(_ library: LibraryInfo) -> [LibraryGenre] {
        guard !search.isEmpty else { return library.genres }
        return library.genres.filter { $0.name.lowercased().contains(search.lowercased()) }
    }
}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}
