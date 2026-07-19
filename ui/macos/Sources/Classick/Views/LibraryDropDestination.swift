import SwiftUI

struct LibraryDropDestination: ViewModifier {
  let target: LibraryDropTarget
  let launchNonce: UUID
  let feedback: String?
  let submit: @MainActor @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void

  @State private var isTargeted = false
  @State private var accessibilitySummary = "selected music"

  func body(content: Content) -> some View {
    content
      .contentShape(Rectangle())
      .background {
        ZStack(alignment: .trailing) {
          if isTargeted {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
              .fill(.selection.opacity(0.22))
            RoundedRectangle(cornerRadius: 7, style: .continuous)
              .stroke(.tint, lineWidth: 1)
          }
          if let feedback {
            Text(feedback)
              .font(.caption)
              .foregroundStyle(.secondary)
              .padding(.trailing, 8)
              .allowsHitTesting(false)
          }
        }
      }
      .accessibilityLabel(
        LibraryDropFeedback.accessibilityLabel(
          summary: accessibilitySummary, target: target))
      .dropDestination(for: LibraryDragPayload.self) { items, _ in
        guard
          let rules = try? LibraryDropAcceptance.rules(
            from: items, expectedNonce: launchNonce)
        else { return false }
        accessibilitySummary = items.map(\.summary).joined(separator: ", ")
        let requestID = UUID()
        submit(target, rules, requestID)
        return true
      } isTargeted: { targeted in
        isTargeted = targeted
      }
  }
}

extension View {
  @ViewBuilder
  func libraryDropDestination(
    target: LibraryDropTarget?,
    launchNonce: UUID,
    feedback: String? = nil,
    submit: @escaping @MainActor @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void
  ) -> some View {
    if let target {
      modifier(
        LibraryDropDestination(
          target: target, launchNonce: launchNonce, feedback: feedback, submit: submit))
    } else {
      self
    }
  }
}
