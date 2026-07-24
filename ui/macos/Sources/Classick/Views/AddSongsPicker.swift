import SwiftUI

/// "Add Songs…" sheet (Task 7) for the manual playlist editor. Presents the
/// shared `LibraryBrowser` in `.select`/`.flat` mode over a temporary
/// checked set — `.flat` (not `.cascading`) because a playlist's tracks are
/// a frozen snapshot the user explicitly curated, unlike a device's sync
/// scope, which is meant to auto-follow future library growth (see
/// `SelectStyle`'s doc comment).
///
/// The checked artist/album/genre rows aren't themselves addable to a
/// manual playlist's `tracks: [String]` — that field wants literal,
/// resolvable source-relative file paths, and the library aggregates this
/// browser renders carry track COUNTS only, never individual filenames
/// (`docs/ipc-protocol.md`, library mutation outcomes). `onAdd` hands the checked
/// rules to the caller, which sends `resolve_tracks` and
/// appends whatever comes back on `resolved_tracks` — see
/// `ManualPlaylistLogic.appendingTracks` and `PlaylistPage`'s
/// `ManualPlaylistEditor`.
struct AddSongsPicker: View {
    var library: LibraryInfo?
    var isResolving: Bool
    var onAdd: (Set<SelectionKey>) -> Void
    var onCancel: () -> Void

    @State private var facet: LibraryBrowser.Facet = .artists
    @State private var search = ""
    @State private var checked: Set<SelectionKey> = []

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(minWidth: 480, minHeight: 420)
    }

    /// Title above, facet picker centered below it — the same vertical
    /// order the Library and device Music pages use for their titlebar +
    /// facet bar, so the sheet reads as the same kind of surface.
    private var header: some View {
        VStack(spacing: 10) {
            HStack(spacing: 12) {
                Text("Add Songs").font(.headline)
                Spacer(minLength: 12)
                TextField("Search", text: $search)
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 200)
            }
            FacetPicker(facet: $facet, facets: pickerFacets)
                .frame(maxWidth: .infinity)
        }
        .padding(12)
    }

    /// Cancel then default action, bottom-trailing: the macOS sheet
    /// convention, and what this app's other sheet
    /// (`ReplaceLibraryConfirmationSheet`) already does. These buttons used
    /// to sit in the top-right corner, which is the iOS/toolbar idiom, not
    /// this platform's.
    private var footer: some View {
        HStack(spacing: 10) {
            if isResolving {
                ProgressView().controlSize(.small)
                Text("Resolving tracks…").foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            // Cancel must NEVER be disabled: with it gated on
            // `isResolving`, a lost `resolve_tracks` reply (daemon
            // restart, dropped send) left BOTH buttons dead and the
            // user sealed in the sheet until app quit (sweep finding
            // #4). Escape is always available; the caller resets its
            // in-flight flag.
            Button("Cancel", action: onCancel)
                .keyboardShortcut(.cancelAction)
            Button(isResolving ? "Adding…" : "Add") { onAdd(checked) }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .disabled(checked.isEmpty || isResolving)
        }
        .padding(12)
    }

    /// `.playlists` isn't a library facet at all (it's the device Music
    /// page's subscriptions checklist) — never offered here, same as
    /// `LibraryView.browsableFacets`.
    private var pickerFacets: [LibraryBrowser.Facet] { LibraryBrowser.Facet.browsable }

    @ViewBuilder
    private var content: some View {
        if let library, library.scannedAtUnixSecs != nil {
            LibraryBrowser(library: library, facet: facet, mode: .select(checked: $checked, style: .flat), search: search)
        } else {
            ContentUnavailableView(
                "No Library Yet", systemImage: "music.note.list",
                description: Text("Scan your library first."))
        }
    }
}

#if DEBUG
#Preview("Library loaded") {
    AddSongsPicker(library: PreviewFixtures.richLibrary, isResolving: false, onAdd: { _ in }, onCancel: {})
}

#Preview("Adding…") {
    AddSongsPicker(library: PreviewFixtures.richLibrary, isResolving: true, onAdd: { _ in }, onCancel: {})
}

#Preview("No library") {
    AddSongsPicker(library: nil, isResolving: false, onAdd: { _ in }, onCancel: {})
}
#endif
