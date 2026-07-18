import SwiftUI

extension View {
    /// Uses the system chrome-attached bar on macOS 26+, with the equivalent
    /// material-backed inset on the macOS 15 deployment floor.
    @ViewBuilder
    func facetBarBelowToolbar<Bar: View>(
        @ViewBuilder bar: @escaping () -> Bar
    ) -> some View {
        if #available(macOS 26.0, *) {
            self
                .scrollEdgeEffectStyle(.hard, for: .top)
                .safeAreaBar(edge: .top, spacing: 0) { bar() }
        } else {
            self.safeAreaInset(edge: .top, spacing: 0) {
                bar().background(.bar)
            }
        }
    }
}
