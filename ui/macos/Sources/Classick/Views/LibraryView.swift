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
  @State private var expandedDisclosures: Set<LibraryBrowser.DisclosureKey> = []

  var body: some View {
    VStack(spacing: 0) {
      if model.sourceNeedsAttention {
        sourceAttention
      }
      content
    }
    // `.principal` is the system's centered toolbar slot and the stock
    // macOS home for a view switcher (Calendar's D/W/M/Y). The device
    // Music page puts its own facet picker in the same slot, so the
    // control doesn't move when you switch pages in the sidebar.
    //
    // The title stays visible: `.principal` sits centered while the title
    // renders leading, so suppressing it (which this page used to do) was
    // never the price of the centered control — it just made Library the
    // one detail page that couldn't tell you where you were.
    .navigationTitle("Library")
    .toolbar {
      ToolbarItem(placement: .principal) {
        FacetPicker(facet: $facet, facets: browsableFacets)
      }
    }
    .hardTopScrollEdge()
  }

  private var sourceAttention: some View {
    HStack(spacing: 12) {
      // The whole Label used to be `.secondary`, which drained the one
      // warning symbol in the window down to the same gray as ordinary
      // metadata. Tint the symbol, keep the text at full contrast: this
      // strip is the only thing telling the user their music folder is
      // gone.
      Label {
        Text(SourceRecoveryPresentation.attentionTitle)
      } icon: {
        Image(systemName: "exclamationmark.triangle.fill")
          .foregroundStyle(.orange)
      }
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
    LibraryBrowser.Facet.browsable
  }

  @ViewBuilder
  private var content: some View {
    switch LibraryContentLogic.state(
      library: model.library, phase: model.phase, configuredSource: model.config?.source)
    {
    case .needsScan:
      LibraryStateView.needsScan(onScan: onScan)
    case .scanning(let current, let total):
      LibraryStateView.scanning(current: current, total: total)
    case .libraryEmpty(let path):
      LibraryStateView.libraryEmpty(path: path, onScan: onScan)
    case .browse:
      if let library = model.library {
        LibraryBrowser(
          library: library, facet: facet, mode: .browse, search: "",
          launchNonce: model.libraryDragLaunchNonce,
          expandedDisclosures: $expandedDisclosures)
      }
    }
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
