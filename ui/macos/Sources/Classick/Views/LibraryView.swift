import SwiftUI

/// The Library page: a browse-only view of the scanned music library — a
/// facet picker (Artists/Albums/Genres; `.playlists` is device-page-only,
/// see `LibraryBrowser.Facet`) and search, over the shared `LibraryBrowser`
/// in `.browse` mode.
///
/// Canonical-surface rule: sync intent (checkboxes, mode pickers) is
/// displayed/edited ONLY on device pages (Task 5) — this page renders NO
/// checkbox affordances. It previously carried its own `SelectionDraft`
/// editing UI; that plumbing moved to the device Music page and was deleted
/// here.
struct LibraryView: View {
    var model: AppModel
    var onScan: () -> Void

    @State private var facet: LibraryBrowser.Facet = .artists
    @State private var search = ""

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
    }

    private var header: some View {
        VStack(spacing: 8) {
            HStack {
                Picker("", selection: $facet) {
                    ForEach(browsableFacets, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .frame(width: 270)
                TextField("Search", text: $search)
                    .textFieldStyle(.roundedBorder)
            }
        }
        .padding(12)
    }

    /// `.playlists` is device-page-only (subscriptions checklist) — never
    /// offered here, which is what keeps `.playlists` structurally
    /// unreachable from this browse-only page.
    private var browsableFacets: [LibraryBrowser.Facet] {
        [.artists, .albums, .genres]
    }

    @ViewBuilder
    private var content: some View {
        if let library = model.library, library.scannedAtUnixSecs != nil {
            LibraryBrowser(library: library, facet: facet, mode: .browse, search: search)
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
}
