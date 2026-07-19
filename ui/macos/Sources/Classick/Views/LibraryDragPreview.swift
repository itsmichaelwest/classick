import SwiftUI

struct LibraryDragPreview: View {
  let summary: String
  let systemImage: String

  var body: some View {
    Label(summary, systemImage: systemImage)
      .font(.callout.weight(.medium))
      .lineLimit(1)
      .padding(.horizontal, 12)
      .padding(.vertical, 8)
      .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 9))
      .accessibilityLabel(summary)
  }
}

extension View {
  @ViewBuilder
  func libraryDragSource(_ payload: LibraryDragPayload?, systemImage: String) -> some View {
    if let payload {
      draggable(payload) {
        LibraryDragPreview(summary: payload.summary, systemImage: systemImage)
      }
    } else {
      self
    }
  }
}
