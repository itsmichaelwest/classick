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
        switch LibraryContentLogic.state(library: model.library, phase: model.phase, configuredSource: model.config?.source) {
        case .needsScan:
            needsScanState
        case let .scanning(current, total):
            scanningState(current: current, total: total)
        case let .libraryEmpty(path):
            libraryEmptyState(path: path)
        case .browse:
            if let library = model.library {
                LibraryBrowser(library: library, facet: facet, mode: .browse, search: search)
            }
        }
    }

    private var needsScanState: some View {
        VStack(spacing: 12) {
            Spacer()
            Text("Classick needs to read your library's tags once")
                .font(.headline)
            Button("Scan Library", action: onScan)
                .keyboardShortcut(.defaultAction)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func scanningState(current: Int, total: Int) -> some View {
        VStack(spacing: 12) {
            Spacer()
            ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
                .frame(maxWidth: 260)
            Text("Scanning… \(current) of \(total)")
                .font(.caption).foregroundStyle(.secondary)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    /// Global Constraints' "library empty" state, verbatim copy: "No audio
    /// files found in <path>". Paired with a Rescan action so a user who
    /// just dropped files into the folder can recover without leaving the
    /// page (mirrors `needsScanState`'s "Scan Library" action).
    private func libraryEmptyState(path: String) -> some View {
        VStack(spacing: 12) {
            Spacer()
            Text("No audio files found in \(path)")
                .font(.headline)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
            Button("Rescan Library", action: onScan)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}

/// Shared pure logic for deciding which content state a library-backed
/// browsing page (this page, `DeviceMusicPage`) should show. "No source
/// configured" isn't one of these cases — `AppModel.needsFirstRunSetup`
/// gates the ENTIRE detail area before any page carrying this logic can
/// even render (see `MainWindow`), so by the time either page's `content`
/// runs, a source is guaranteed to be configured.
enum LibraryContentState: Equatable {
    /// Source configured, but the daemon hasn't completed a first scan yet
    /// and isn't actively scanning right now either (e.g. right after
    /// setup, before the daemon's initial scan kicks off). Not one of the
    /// Global Constraints' four named states — a transient bridge state,
    /// kept from the pre-restructure UI.
    case needsScan
    case scanning(current: Int, total: Int)
    /// Global Constraints: "library empty → 'No audio files found in
    /// <path>'" — a completed scan that found zero tracks.
    case libraryEmpty(path: String)
    case browse
}

enum LibraryContentLogic {
    static func state(library: LibraryInfo?, phase: Phase, configuredSource: String?) -> LibraryContentState {
        if case let .scanning(current, total) = phase {
            return .scanning(current: current, total: total)
        }
        guard let library, library.scannedAtUnixSecs != nil else {
            return .needsScan
        }
        guard library.totalTracks > 0 else {
            return .libraryEmpty(path: library.sourceRoot ?? configuredSource ?? "your music folder")
        }
        return .browse
    }
}
