import AppKit
import Foundation

@MainActor
final class LibraryDropSubmissionCoordinator {
  private struct Intent {
    let target: LibraryDropTarget
    let rules: [SelectionRule]
    let requestID: UUID

    var command: WireV3Command {
      switch target {
      case .device(let serial, _):
        .addSelectionToDevice(
          deviceID: serial, requestID: requestID, mutationID: UUID(), rules: rules)
      case .manualPlaylist(let slug, _):
        .appendSelectionToPlaylist(requestID: requestID, slug: slug, rules: rules)
      }
    }
  }

  private var pending: [Intent] = []
  private var drainTask: Task<Void, Never>?
  private let send: @Sendable (WireV3Command) async -> SendDisposition
  private let rejectLocally: @MainActor (UUID, LibraryDropTarget, String) -> Void

  init(
    send: @escaping @Sendable (WireV3Command) async -> SendDisposition,
    rejectLocally: @escaping @MainActor (UUID, LibraryDropTarget, String) -> Void
  ) {
    self.send = send
    self.rejectLocally = rejectLocally
  }

  convenience init(send: @escaping @Sendable (WireV3Command) async -> SendDisposition) {
    self.init(send: send, rejectLocally: { _, _, _ in })
  }

  func submit(target: LibraryDropTarget, rules: [SelectionRule], requestID: UUID) {
    pending.append(Intent(target: target, rules: rules, requestID: requestID))
    guard drainTask == nil else { return }
    drainTask = Task { await drain() }
  }

  private func drain() async {
    while !pending.isEmpty {
      let intent = pending.removeFirst()
      switch await send(intent.command) {
      case .sent, .queued:
        break
      case .dropped:
        rejectLocally(
          intent.requestID, intent.target,
          "Couldn’t send this addition to Classick.")
      }
    }
    drainTask = nil
  }
}

@MainActor
final class LibraryDropAnnouncementCoordinator {
  private var announcedRequestIDs: Set<String> = []
  private let post: @MainActor (String) -> Void

  init(
    post: @escaping @MainActor (String) -> Void =
      LibraryDropAnnouncementCoordinator.postToAppKit
  ) {
    self.post = post
  }

  func announce(requestID: String, outcome: DropOutcome) {
    guard announcedRequestIDs.insert(requestID).inserted else { return }
    post(outcome.accessibleMessage)
  }

  private static func postToAppKit(_ text: String) {
    NSAccessibility.post(
      element: NSApplication.shared, notification: .announcementRequested,
      userInfo: [
        .announcement: text,
        .priority: NSAccessibilityPriorityLevel.medium.rawValue,
      ])
  }
}
