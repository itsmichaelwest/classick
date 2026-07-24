import SwiftUI

/// The Artists / Albums / Genres [/ Playlists] switcher — one definition for
/// every surface that offers it (the Library page, the device Music page,
/// the Add Songs sheet). There used to be three copies in three different
/// places at three widths: centered in the toolbar via `.principal`, in a
/// 320pt bar below the toolbar, and in a 270pt hand-built sheet header.
/// Same control, so: same look, same place, same width rule.
///
/// Each copy also passed `Picker("")` — an empty label string, which
/// VoiceOver announces as nothing at all on the app's primary navigation
/// control. A real label, hidden visually, fixes that once for all sites.
struct FacetPicker: View {
  @Binding var facet: LibraryBrowser.Facet
  var facets: [LibraryBrowser.Facet]

  /// Per-segment rather than absolute, so the 4-facet device bar and the
  /// 3-facet library bar share a rhythm instead of the old 320/270 mismatch.
  private static let segmentWidth: CGFloat = 84

  var body: some View {
    Picker("View", selection: $facet) {
      ForEach(facets, id: \.self) { Text($0.rawValue).tag($0) }
    }
    .pickerStyle(.segmented)
    .labelsHidden()
    .frame(width: CGFloat(facets.count) * Self.segmentWidth)
  }
}

extension LibraryBrowser.Facet {
  /// `.playlists` is a device-page concept (the subscriptions checklist),
  /// never a library facet — see `LibraryView.browsableFacets`.
  static let browsable: [LibraryBrowser.Facet] = [.artists, .albums, .genres]
}

#if DEBUG
  private struct FacetPickerPreviewHost: View {
    var facets: [LibraryBrowser.Facet]
    @State private var facet: LibraryBrowser.Facet = .artists

    var body: some View {
      FacetPicker(facet: $facet, facets: facets).padding()
    }
  }

  #Preview("Library (3)") {
    FacetPickerPreviewHost(facets: LibraryBrowser.Facet.browsable)
  }

  #Preview("Device (4)") {
    FacetPickerPreviewHost(facets: LibraryBrowser.Facet.allCases)
  }
#endif
