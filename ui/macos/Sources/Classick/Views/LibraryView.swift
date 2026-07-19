import SwiftUI

/// The Library page: a browse-only view of the scanned music library — a
/// facet picker (Artists/Albums/Genres; `.playlists` is device-page-only,
/// see `LibraryBrowser.Facet`) centered in the toolbar, over the shared
/// `LibraryBrowser` in `.browse` mode. No visible title, no search (removed
/// per design; `LibraryBrowser`'s search plumbing remains for a future
/// `.searchable` pass).
///
/// Canonical-surface rule: sync intent (checkboxes, mode pickers) is
/// displayed/edited ONLY on device pages (Task 5) — this page renders NO
/// checkbox affordances. It previously carried its own `SelectionDraft`
/// editing UI; that plumbing moved to the device Music page and was deleted
/// here.
struct LibraryView: View {
  var model: AppModel
  var onScan: () -> Void
  var onConnectSource: () -> Void

  @State private var facet: LibraryBrowser.Facet = .artists

  var body: some View {
    VStack(spacing: 0) {
      if model.sourceNeedsAttention {
        sourceAttention
      }
      content
    }
    // Window still knows its name (app switcher, accessibility) but
    // the toolbar doesn't render it — the centered facet control IS
    // this page's chrome. `.principal` is the system's centered
    // toolbar slot; `.toolbar(removing: .title)` (macOS 14+) is the
    // supported way to suppress the visible title.
    .navigationTitle("Library")
    .toolbar(removing: .title)
    .toolbar {
      ToolbarItem(placement: .principal) {
        Picker("", selection: $facet) {
          ForEach(browsableFacets, id: \.self) { Text($0.rawValue).tag($0) }
        }
        .pickerStyle(.segmented)
      }
    }
  }

  private var sourceAttention: some View {
    HStack(spacing: 12) {
      Label(
        SourceRecoveryPresentation.attentionTitle,
        systemImage: "exclamationmark.triangle.fill"
      )
      .foregroundStyle(.secondary)
      Spacer()
      Button("Connect", action: onConnectSource)
        .disabled(model.sourceRetryPending)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 10)
    .background(.bar)
    .overlay(alignment: .bottom) { Divider() }
  }

  /// `.playlists` is device-page-only (subscriptions checklist) — never
  /// offered here, which is what keeps `.playlists` structurally
  /// unreachable from this browse-only page.
  private var browsableFacets: [LibraryBrowser.Facet] {
    [.artists, .albums, .genres]
  }

  @ViewBuilder
  private var content: some View {
    switch LibraryContentLogic.state(
      library: model.library, phase: model.phase, configuredSource: model.config?.source)
    {
    case .needsScan:
      needsScanState
    case .scanning(let current, let total):
      scanningState(current: current, total: total)
    case .libraryEmpty(let path):
      libraryEmptyState(path: path)
    case .browse:
      if let library = model.library {
        LibraryBrowser(library: library, facet: facet, mode: .browse, search: "")
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
  static func state(library: LibraryInfo?, phase: Phase, configuredSource: String?)
    -> LibraryContentState
  {
    if case .scanning(let current, let total) = phase {
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

#if DEBUG
  /// NavigationStack so the toolbar chrome (centered facet control, removed
  /// title) renders in the canvas — see `DeviceMusicPage`'s preview note.
  @MainActor
  private func libraryPreview(_ model: AppModel) -> some View {
    NavigationStack {
      LibraryView(model: model, onScan: {}, onConnectSource: {})
    }
    .frame(width: 760, height: 560)
  }

  #Preview("Browse") {
    libraryPreview(PreviewFixtures.connectedSyncedModel())
  }

  #Preview("Empty") {
    libraryPreview(PreviewFixtures.emptyLibraryModel())
  }

  #Preview("Scanning") {
    libraryPreview(PreviewFixtures.scanningModel())
  }

  #Preview("Music share needs attention") {
    libraryPreview(PreviewFixtures.sourceAttentionModel())
  }
#endif
