import SwiftUI

extension View {
  /// Declares the hard top scroll-edge effect (macOS 26+), so content meets
  /// the toolbar with a definite edge rather than bleeding through it. No-op
  /// on the macOS 15 floor, where the toolbar manages its own on-scroll
  /// background.
  ///
  /// Pages declare this themselves because a bare `ScrollView` isn't reliably
  /// picked up as the primary scroll view on current macOS (a `List` is) —
  /// see `LibraryBrowser.albumsTable`, whose pinned section headers depend on
  /// it. It used to ride along on `facetBarBelowToolbar`; that bar is gone
  /// now that both facet pickers live in the toolbar itself.
  @ViewBuilder
  func hardTopScrollEdge() -> some View {
    if #available(macOS 26.0, *) {
      scrollEdgeEffectStyle(.hard, for: .top)
    } else {
      self
    }
  }
}
