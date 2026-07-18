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
    var onResolveTracks: (_ slug: String, _ rules: [SelectionRule]) -> Void = { _, _ in }

    /// `model.playlistDetail` is a single slot (not scoped by slug) — only
    /// meaningful once its own `slug` matches the page currently showing;
    /// a reply for a playlist the user already navigated away from must not
    /// render here.
    private var detail: PlaylistDetail? {
        model.playlistDetail?.slug == slug ? model.playlistDetail : nil
    }

    // No page-level `navigationTitle` here: the editors declare an EDITABLE
    // titlebar title (`navigationTitle(Binding<String>)`, bound to their
    // draft name) plus their action toolbar — a static title here would
    // fight theirs, and the old in-page header row duplicated the titlebar.
    // Loading/error states set a static title themselves so the titlebar
    // isn't blank before the detail arrives.
    var body: some View {
        content
            .task(id: slug) { onGetPlaylist(slug) }
    }

    /// The sidebar summary usually has the display name before the
    /// `get_playlist` reply lands, so the title doesn't flash "Playlist" →
    /// name on navigation; the bare fallback only shows for a slug that's
    /// in neither (e.g. just deleted elsewhere).
    private var pageTitle: String {
        detail?.name
            ?? model.playlists.first(where: { $0.slug == slug })?.name
            ?? "Playlist"
    }

    @ViewBuilder
    private var content: some View {
        if let detail {
            if let error = detail.error {
                ContentUnavailableView(
                    "Can't Open Playlist", systemImage: "exclamationmark.triangle",
                    description: Text(error))
                    .navigationTitle(pageTitle)
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
                .navigationTitle(pageTitle)
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
    var onResolveTracks: (_ slug: String, _ rules: [SelectionRule]) -> Void

    private struct ManualDraft: Equatable {
        var name: String = ""
        var tracks: [String] = []
    }

    @State private var draft = ManualDraft()
    @State private var seededFromModel = false
    @State private var userEdited = false
    /// Seed-assignment marker — see `.onChange(of: draft)`.
    @State private var isSeeding = false
    @State private var saveTask: Task<Void, Never>?
    @State private var showAddSongs = false
    @State private var isResolvingAdd = false
    @State private var showDeleteConfirm = false
    @State private var showRename = false
    @State private var renameText = ""

    var body: some View {
        trackList
        // Editable titlebar title — `navigationTitle(Binding<String>)` is
        // the system's document-rename affordance (click the title to
        // edit), bound straight into the draft so renames flow through the
        // same debounced save as every other edit. Replaces the old
        // in-page header row, which duplicated the titlebar's title.
        .navigationTitle(Binding(get: { draft.name }, set: { draft.name = $0 }))
        // The binding-title alone only ENABLES renaming — the visible
        // affordances are these: `toolbarTitleMenu` gives the title its
        // chevron menu, and `RenameButton()` is the system control that
        // begins inline titlebar editing (it finds the title binding by
        // itself). One more RenameButton in the ellipsis menu for
        // discoverability.
        .toolbarTitleMenu {
            RenameButton()
        }
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button("Add Songs…") { showAddSongs = true }
                Menu {
                    // Explicit rename path: the system RenameButton /
                    // title-menu affordance doesn't reliably begin editing
                    // on the current OS (27 beta) — this alert always
                    // works and writes through the same debounced save.
                    Button("Rename…") {
                        renameText = draft.name
                        showRename = true
                    }
                    RenameButton()
                    Button("Delete Playlist", role: .destructive) { showDeleteConfirm = true }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
            }
        }
        .task { seedIfNeeded() }
        .onChange(of: detail) { _, _ in seedIfNeeded() }
        .onChange(of: draft) { _, newDraft in
            // Seed assignments must not count as edits or fire saves —
            // same isSeeding guard as the device pages.
            if isSeeding { isSeeding = false; return }
            userEdited = true
            scheduleSave(newDraft)
        }
        .onDisappear { saveTask?.cancel() }
        // `resolved_tracks` carries no correlation id of its own on the wire
        // — `model.latestResolvedTracks` tags each reply with the requesting
        // playlist's slug (see its doc comment), and this page only consumes
        // a reply tagged with ITS OWN slug. This is what keeps a stale reply
        // meant for a different playlist (e.g. one still in flight when the
        // user somehow navigates away and back) from appending to the wrong
        // draft — safe today only because `AddSongsPicker` is a window-modal
        // `.sheet()`, but this guard makes correctness not depend on that.
        .onChange(of: model.resolvedTracksRevision) { _, _ in
            guard isResolvingAdd, let reply = model.latestResolvedTracks,
                  ManualPlaylistLogic.shouldConsumeResolvedTracks(reply: reply, forSlug: slug)
            else { return }
            isResolvingAdd = false
            draft.tracks = ManualPlaylistLogic.appendingTracks(draft.tracks, adding: reply.tracks)
            showAddSongs = false
        }
        .sheet(isPresented: $showAddSongs) {
            AddSongsPicker(
                library: model.library, isResolving: isResolvingAdd,
                onAdd: { rules in
                    isResolvingAdd = true
                    onResolveTracks(slug, Array(rules))
                },
                onCancel: {
                    // Escape hatch for a lost resolve reply (sweep finding
                    // #4): cancel always works and clears the in-flight
                    // flag, so reopening the sheet starts clean.
                    isResolvingAdd = false
                    showAddSongs = false
                })
        }
        .alert("Rename Playlist", isPresented: $showRename) {
            TextField("Name", text: $renameText)
            Button("Rename") {
                if PlaylistEditorLogic.isNameValid(renameText) {
                    draft.name = renameText
                }
            }
            Button("Cancel", role: .cancel) {}
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
            .environment(\.defaultMinListRowHeight, LibraryBrowser.rowHeight)
        }
    }

    // Table-style columns: title (flexible) | artist | album, derived from
    // the source-relative path (the wire carries no per-track tag data for
    // playlist entries; length would need track durations in the library
    // index — a Rust-side addition, not available today). A real SwiftUI
    // `Table` is deliberately NOT used: it has no row-reordering, and
    // drag-to-reorder is this editor's core interaction.
    //
    // Note: no per-track "missing" indicator — a path-derived heuristic
    // false-flagged whole playlists when folder layout ≠ tags; the wire has
    // no per-file existence data.
    private func trackRow(_ path: String) -> some View {
        let display = ManualPlaylistLogic.trackDisplay(path: path)
        let album = ManualPlaylistLogic.albumComponent(path: path)
        // The artist and album have their own columns — the title column
        // shows ONLY the cleaned song title, with filename noise (leading
        // track numbers, "Artist - Album - " prefixes) stripped.
        let title = ManualPlaylistLogic.cleanedTitle(display.title)
        return HStack(spacing: 8) {
            Text(title)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 12)
            Text(display.artist ?? "—")
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 150, alignment: .leading)
            Text(album ?? "—")
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 180, alignment: .leading)
        }
    }

    /// Seeds the draft from `get_playlist`'s reply — EDIT-gated, not
    /// once-only (same rationale as `DeviceMusicPage.seedIfNeeded`): while
    /// unedited, a refreshed `playlist_detail` (e.g. after a sidebar
    /// drag-drop appends tracks to THIS playlist) updates the open editor.
    private func seedIfNeeded() {
        guard !userEdited else { return }
        let seeded = ManualDraft(name: detail.name ?? "", tracks: detail.tracks ?? [])
        if seeded != draft {
            isSeeding = true
            draft = seeded
        }
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
    /// Strips filename noise from a track title for display in a table that
    /// already has artist/album columns. The song title is conventionally
    /// the LAST " - "-separated segment ("Artist - Album - NN - Title"), so
    /// take that and protect it from all further stripping — which also
    /// keeps songs named after their album ("… - Beautiful Lies") or named
    /// with digits ("… - 1979") intact. A leading "NN " / "NN. " / "NN - "
    /// track-number prefix is then removed, unless that would leave nothing.
    nonisolated static func cleanedTitle(_ stem: String) -> String {
        let segments = stem.components(separatedBy: " - ")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        var result = segments.last ?? stem
        if let range = result.range(of: #"^\d{1,3}[\s.\-_]+"#, options: .regularExpression),
           range.upperBound != result.endIndex {
            result = String(result[range.upperBound...])
        }
        return result.isEmpty ? stem : result
    }

    /// The path's second component when it has ≥3 (Artist/Album/file) —
    /// the same folder-layout-derived approximation `trackDisplay` uses for
    /// artist. `nil` for flatter layouts, rendered as "—".
    nonisolated static func albumComponent(path: String) -> String? {
        let normalized = path.replacingOccurrences(of: "\\", with: "/")
        let components = normalized.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        return components.count > 2 ? components[components.count - 2] : nil
    }

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

    /// Add Songs' append step: preserves the existing track order, appends
    /// the newly resolved batch in natural (Finder-style) path order, and
    /// dedups both against the existing list AND within the newly-added
    /// batch itself.
    ///
    /// `resolve_tracks` deliberately returns paths in plain lexicographic
    /// order server-side (the Rust side is unchanged) — fine for
    /// zero-padded filenames, but a non-zero-padded album ("1.flac",
    /// "10.flac", "2.flac") would sort 1, 10, 2 and append out of running
    /// order. `localizedStandardCompare` (Finder's own sort) handles
    /// embedded numeric segments correctly, and since it compares the whole
    /// relative path, tracks stay grouped by their containing directory
    /// (album) even across a multi-album batch.
    nonisolated static func appendingTracks(_ existing: [String], adding: [String]) -> [String] {
        var seen = Set(existing)
        var result = existing
        let naturallyOrdered = adding.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        for path in naturallyOrdered where !seen.contains(path) {
            result.append(path)
            seen.insert(path)
        }
        return result
    }

    /// Resolve-reply correlation guard: a `resolved_tracks` reply belongs to
    /// this editor only when its tagged slug matches the editor's own. See
    /// `AppModel.latestResolvedTracks`'s doc comment for why the reply is
    /// tagged in the first place.
    nonisolated static func shouldConsumeResolvedTracks(reply: ResolvedTracksReply, forSlug slug: String) -> Bool {
        reply.slug == slug
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
        return subscribedDeviceCount == 1
            ? "It will also be removed from 1 iPod that syncs it. This can't be undone."
            : "It will also be removed from \(subscribedDeviceCount) iPods that sync it. This can't be undone."
    }
}

#if DEBUG
/// NavigationStack so the titlebar chrome (editable title, toolbar actions)
/// renders in the canvas — see `DeviceMusicPage`'s preview note.
@MainActor
private func playlistPreview(_ model: AppModel, slug: String, height: CGFloat) -> some View {
    NavigationStack {
        PlaylistPage(model: model, slug: slug, onSavePlaylist: { _ in })
    }
    .frame(width: 640, height: height)
}

#Preview("Manual") {
    playlistPreview(
        PreviewFixtures.playlistDetailModel(PreviewFixtures.manualPlaylistDetail),
        slug: PreviewFixtures.manualPlaylistDetail.slug, height: 520)
}

#Preview("Smart") {
    playlistPreview(
        PreviewFixtures.playlistDetailModel(PreviewFixtures.smartPlaylistDetail),
        slug: PreviewFixtures.smartPlaylistDetail.slug, height: 560)
}

#Preview("Error") {
    playlistPreview(
        PreviewFixtures.playlistDetailModel(PreviewFixtures.brokenPlaylistDetail),
        slug: PreviewFixtures.brokenPlaylistDetail.slug, height: 400)
}

#Preview("Loading") {
    playlistPreview(
        PreviewFixtures.playlistLoadingModel(),
        slug: PreviewFixtures.roadTripMix.slug, height: 400)
}
#endif
