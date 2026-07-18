import SwiftUI

/// The playlist editor (Task 7, sidebar-click destination). Routes to the
/// manual track-list editor or the smart rule builder (`SmartRulesEditor`,
/// its own file) based on `PlaylistDetail.kind`. Loading/error states per
/// the plan: request `get_playlist` on appear/slug change, render from
/// `model.playlistDetail` once its `slug` matches this page's, show a
/// one-line error (never a crash) when `detail.error != nil`.
struct PlaylistPage: View {
    var model: AppModel
    var slug: String
    var onSavePlaylist: (PlaylistPayload) -> Void
    var onGetPlaylist: (String) -> Void = { _ in }
    var onDeletePlaylist: (String) -> Void = { _ in }
    var onResolveTracks: ([SelectionRule]) -> Void = { _ in }

    /// `model.playlistDetail` is a single slot (not scoped by slug) — only
    /// meaningful once its own `slug` matches the page currently showing;
    /// a reply for a playlist the user already navigated away from must not
    /// render here.
    private var detail: PlaylistDetail? {
        model.playlistDetail?.slug == slug ? model.playlistDetail : nil
    }

    var body: some View {
        content
            .task(id: slug) { onGetPlaylist(slug) }
    }

    @ViewBuilder
    private var content: some View {
        if let detail {
            if let error = detail.error {
                ContentUnavailableView(
                    "Can't Open Playlist", systemImage: "exclamationmark.triangle",
                    description: Text(error))
            } else if detail.kind == .manual {
                ManualPlaylistEditor(
                    model: model, slug: slug, detail: detail,
                    onSavePlaylist: onSavePlaylist, onDeletePlaylist: onDeletePlaylist,
                    onResolveTracks: onResolveTracks)
            } else if detail.kind == .smart {
                SmartRulesEditor(
                    model: model, slug: slug, detail: detail,
                    onSavePlaylist: onSavePlaylist, onDeletePlaylist: onDeletePlaylist)
            } else {
                // `kind` is always set together with the matching content
                // field on success (doc: playlist_detail) — this branch is
                // unreachable in practice, kept only so the switch is total
                // without force-unwrapping.
                ProgressView()
            }
        } else {
            ProgressView("Loading…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// Manual playlist editor: an ordered, reorderable track list loaded from
/// `get_playlist`'s `tracks` (source-relative paths), plus Add Songs / rename
/// / delete. Edits a local draft and auto-saves it debounced (plan Task 7:
/// "All edits send `.savePlaylist` (debounced)") — mirrors
/// `DeviceMusicPage`/`DeviceSettingsPage`'s seed-once, never-re-seed-after-
/// edit pattern.
private struct ManualPlaylistEditor: View {
    var model: AppModel
    var slug: String
    var detail: PlaylistDetail
    var onSavePlaylist: (PlaylistPayload) -> Void
    var onDeletePlaylist: (String) -> Void
    var onResolveTracks: ([SelectionRule]) -> Void

    private struct ManualDraft: Equatable {
        var name: String = ""
        var tracks: [String] = []
    }

    @State private var draft = ManualDraft()
    @State private var seededFromModel = false
    @State private var userEdited = false
    @State private var saveTask: Task<Void, Never>?
    @State private var showAddSongs = false
    @State private var isResolvingAdd = false
    @State private var showDeleteConfirm = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            trackList
        }
        .task { seedIfNeeded() }
        .onChange(of: detail) { _, _ in seedIfNeeded() }
        .onChange(of: draft) { _, newDraft in
            userEdited = true
            scheduleSave(newDraft)
        }
        .onDisappear { saveTask?.cancel() }
        // The Add Songs sheet has no correlation id of its own to watch for
        // (see `AppModel.resolvedTracksRevision`'s doc comment) — this page
        // is the only thing that could have a resolve request in flight
        // (`isResolvingAdd`), so any revision bump while that's true belongs
        // to this page's own request.
        .onChange(of: model.resolvedTracksRevision) { _, _ in
            guard isResolvingAdd else { return }
            isResolvingAdd = false
            draft.tracks = ManualPlaylistLogic.appendingTracks(draft.tracks, adding: model.resolvedTracks)
            showAddSongs = false
        }
        .sheet(isPresented: $showAddSongs) {
            AddSongsPicker(
                library: model.library, isResolving: isResolvingAdd,
                onAdd: { rules in
                    isResolvingAdd = true
                    onResolveTracks(Array(rules))
                },
                onCancel: { showAddSongs = false })
        }
        .confirmationDialog(
            "Delete “\(draft.name)”?", isPresented: $showDeleteConfirm, titleVisibility: .visible
        ) {
            Button("Delete Playlist", role: .destructive) {
                onDeletePlaylist(slug)
                // Navigate away immediately — the just-deleted slug has
                // nothing left to show, and this also cancels any
                // in-flight debounced save via `.onDisappear` before it can
                // resurrect the playlist with a stale write.
                model.selectedDestination = .library
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(PlaylistEditorLogic.deleteConfirmMessage(
                subscribedDeviceCount: PlaylistEditorLogic.subscribedDeviceCount(
                    slug: slug, deviceConfigs: model.deviceConfigs)))
        }
    }

    private var header: some View {
        HStack {
            TextField("Playlist Name", text: Binding(get: { draft.name }, set: { draft.name = $0 }))
                .textFieldStyle(.plain)
                .font(.title2.bold())
            Spacer()
            Button("Add Songs…") { showAddSongs = true }
            Menu {
                Button("Delete Playlist", role: .destructive) { showDeleteConfirm = true }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .frame(width: 24)
        }
        .padding(12)
    }

    @ViewBuilder
    private var trackList: some View {
        if draft.tracks.isEmpty {
            ContentUnavailableView(
                "No Songs", systemImage: "music.note.list",
                description: Text("Add Songs… to build this playlist."))
        } else {
            List {
                ForEach(draft.tracks.indices, id: \.self) { index in
                    trackRow(draft.tracks[index])
                }
                .onMove { from, to in draft.tracks = ManualPlaylistLogic.moved(draft.tracks, from: from, to: to) }
                .onDelete { offsets in draft.tracks = ManualPlaylistLogic.removed(draft.tracks, at: offsets) }
            }
            .listStyle(.inset)
        }
    }

    private func trackRow(_ path: String) -> some View {
        let display = ManualPlaylistLogic.trackDisplay(path: path)
        let missing = ManualPlaylistLogic.isLikelyMissing(path: path, library: model.library)
        return HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(display.title)
                if let artist = display.artist {
                    Text(artist).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if missing {
                Image(systemName: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .help("This file couldn't be found in your library.")
            }
        }
        .opacity(missing ? 0.5 : 1)
    }

    /// Seeds the local draft from `get_playlist`'s reply exactly once, and
    /// never after the user has started editing — same pattern as
    /// `DeviceMusicPage.seedIfNeeded`.
    private func seedIfNeeded() {
        guard !seededFromModel, !userEdited else { return }
        draft = ManualDraft(name: detail.name ?? "", tracks: detail.tracks ?? [])
        seededFromModel = true
    }

    private func scheduleSave(_ d: ManualDraft) {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            guard PlaylistEditorLogic.isNameValid(d.name) else { return }
            onSavePlaylist(.manual(slug: slug, name: d.name, tracks: d.tracks))
        }
    }
}

/// Pure logic backing the manual playlist editor — no SwiftUI, fully
/// unit-testable (see `PlaylistEditorLogicTests`).
enum ManualPlaylistLogic {
    /// Splits a source-relative track path (e.g.
    /// `"Boards of Canada/Geogaddi/01.flac"`) into a display title (the
    /// filename, extension stripped) and an artist (the path's first
    /// component, when the path has more than one component). Backslashes
    /// are normalized first (mirrors `playlist::parse_m3u8`'s own
    /// normalization, so playlists authored/edited on Windows display the
    /// same here).
    nonisolated static func trackDisplay(path: String) -> (title: String, artist: String?) {
        let normalized = path.replacingOccurrences(of: "\\", with: "/")
        let components = normalized.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard let filename = components.last, !filename.isEmpty else {
            return (path, nil)
        }
        let title: String
        if let dotIndex = filename.lastIndex(of: "."), dotIndex != filename.startIndex {
            title = String(filename[filename.startIndex..<dotIndex])
        } else {
            title = filename
        }
        let artist = components.count > 1 ? components[0] : nil
        return (title, artist)
    }

    /// Best-effort "is this track still in the library" proxy. The wire has
    /// no per-file existence flag on `playlist_detail` (only
    /// `playlists_update`'s resolved COUNT, which doesn't say which entries
    /// resolved) — this checks whether the path's derived artist (and, when
    /// present, album) is still known to the current library aggregate.
    /// That's coarser than real file-existence (an artist can be known
    /// while THIS specific track was deleted), but it's the finest signal
    /// available client-side without walking the filesystem. `nil` library
    /// (not yet loaded) never flags anything missing, to avoid a flash of
    /// warning icons before the first `library_update` arrives.
    nonisolated static func isLikelyMissing(path: String, library: LibraryInfo?) -> Bool {
        guard let library else { return false }
        let normalized = path.replacingOccurrences(of: "\\", with: "/")
        let components = normalized.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard components.count > 1 else { return false }
        let artistName = components[0]
        guard let artist = library.artists.first(where: { $0.name.lowercased() == artistName.lowercased() }) else {
            return true
        }
        guard components.count > 2 else { return false }
        let albumName = components[1]
        return !artist.albums.contains { $0.name.lowercased() == albumName.lowercased() }
    }

    /// Add Songs' append step: preserves the existing track order, appends
    /// newly resolved paths in reply order, and dedups both against the
    /// existing list AND within the newly-added batch itself.
    nonisolated static func appendingTracks(_ existing: [String], adding: [String]) -> [String] {
        var seen = Set(existing)
        var result = existing
        for path in adding where !seen.contains(path) {
            result.append(path)
            seen.insert(path)
        }
        return result
    }

    nonisolated static func moved(_ tracks: [String], from: IndexSet, to: Int) -> [String] {
        var tracks = tracks
        tracks.move(fromOffsets: from, toOffset: to)
        return tracks
    }

    nonisolated static func removed(_ tracks: [String], at offsets: IndexSet) -> [String] {
        var tracks = tracks
        tracks.remove(atOffsets: offsets)
        return tracks
    }
}

/// Shared pure logic between the manual and smart playlist editors (rename
/// validity, delete confirmation copy) — no SwiftUI.
enum PlaylistEditorLogic {
    /// Guards the debounced auto-save from persisting a blank/whitespace-only
    /// name — an edit-in-progress (user clearing the field to retype) must
    /// not round-trip a nameless playlist to the daemon.
    nonisolated static func isNameValid(_ name: String) -> Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Devices whose subscriptions include this slug — the delete
    /// confirmation's "Also unsubscribes N device(s)" count.
    nonisolated static func subscribedDeviceCount(slug: String, deviceConfigs: [String: DeviceConfigState]) -> Int {
        deviceConfigs.values.filter { $0.subscriptions.playlists.contains(slug) }.count
    }

    nonisolated static func deleteConfirmMessage(subscribedDeviceCount: Int) -> String {
        guard subscribedDeviceCount > 0 else { return "This can't be undone." }
        return "Also unsubscribes \(subscribedDeviceCount) device\(subscribedDeviceCount == 1 ? "" : "s")."
    }
}
