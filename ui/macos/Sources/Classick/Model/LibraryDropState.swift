import Foundation

struct LibraryDropEligibility: Equatable {
  static func targetForDevice(_ device: DeviceViewState) -> LibraryDropTarget? {
    guard device.configured else { return nil }
    return .device(
      serial: device.deviceID,
      displayName: device.identity.name ?? device.identity.modelLabel)
  }

  static func targetForCard(_ presentation: DeviceRowPresentation) -> LibraryDropTarget? {
    guard let serial = presentation.serial else { return nil }
    return .device(serial: serial, displayName: presentation.title)
  }

  static func targetForPlaylist(_ summary: PlaylistSummary) -> LibraryDropTarget? {
    guard summary.kind == .manual, summary.error == nil, !summary.slug.isEmpty else { return nil }
    return .manualPlaylist(slug: summary.slug, displayName: summary.name)
  }
}

enum LibraryDropAcceptanceError: Error, Equatable {
  case empty
  case tooManyRules
}

enum LibraryDropAcceptance {
  static func rules(
    from items: [LibraryDragPayload], expectedNonce: UUID
  ) throws -> [SelectionRule] {
    guard !items.isEmpty else { throw LibraryDropAcceptanceError.empty }
    var combined: [SelectionRule] = []
    for item in items {
      let rules = try item.validated(expectedNonce: expectedNonce)
      guard combined.count + rules.count <= LibraryDragPayload.maximumRules else {
        throw LibraryDropAcceptanceError.tooManyRules
      }
      combined.append(contentsOf: rules)
    }
    return WireV3Command.canonicalAdditiveRules(combined)
  }
}

func acceptLibraryDrop(
  _ items: [LibraryDragPayload], on target: LibraryDropTarget, expectedNonce: UUID
) -> Bool {
  _ = target
  return (try? LibraryDropAcceptance.rules(from: items, expectedNonce: expectedNonce)) != nil
}

enum LibraryDropFeedback {
  static func accessibilityLabel(summary: String, target: LibraryDropTarget) -> String {
    "Add \(summary) to \(target.displayName)"
  }

  static func belongs(_ outcome: DropOutcome?, to target: LibraryDropTarget) -> Bool {
    guard let outcome else { return false }
    switch (outcome, target) {
    case (.adding(let outcomeTarget), _), (.rejected(let outcomeTarget, _), _):
      return outcomeTarget == target
    case (.addedAndSyncing(let serial), .device(let targetSerial, _)),
      (.addedForNextSync(let serial), .device(let targetSerial, _)),
      (.alreadyPresent(let serial), .device(let targetSerial, _)):
      return serial == targetSerial
    case (.appended(let slug, _), .manualPlaylist(let targetSlug, _)):
      return slug == targetSlug
    default:
      return false
    }
  }
}

enum LibraryDropTarget: Hashable, Sendable {
  case device(serial: DeviceID, displayName: String)
  case manualPlaylist(slug: String, displayName: String)

  fileprivate var identity: LibraryDropTargetIdentity {
    switch self {
    case .device(let serial, _): .device(serial: serial)
    case .manualPlaylist(let slug, _): .manualPlaylist(slug: slug)
    }
  }

  var displayName: String {
    switch self {
    case .device(_, let displayName), .manualPlaylist(_, let displayName): displayName
    }
  }
}

enum DropOutcome: Equatable, Sendable {
  case adding(target: LibraryDropTarget)
  case addedAndSyncing(serial: DeviceID)
  case addedForNextSync(serial: DeviceID)
  case alreadyPresent(serial: DeviceID)
  case appended(slug: String, count: Int)
  case rejected(target: LibraryDropTarget, message: String)

  var accessibleMessage: String {
    switch self {
    case .adding: "Adding…"
    case .addedAndSyncing: "Added and syncing"
    case .addedForNextSync: "Added for next sync"
    case .alreadyPresent: "Already on this iPod"
    case .appended(_, let count): "Appended \(count) songs"
    case .rejected(_, let message): message
    }
  }
}

private enum LibraryDropTargetIdentity: Hashable, Sendable {
  case device(serial: DeviceID)
  case manualPlaylist(slug: String)
}

struct LibraryDropState: Sendable {
  private struct Pending: Sendable {
    var requestID: String
    var target: LibraryDropTarget
  }

  private var pending: [LibraryDropTargetIdentity: Pending] = [:]
  private(set) var outcome: DropOutcome?

  mutating func markAdding(requestID: UUID, target: LibraryDropTarget) {
    pending[target.identity] = Pending(
      requestID: requestID.uuidString.lowercased(), target: target)
  }

  func isAdding(target: LibraryDropTarget) -> Bool {
    pending[target.identity] != nil
  }

  func isAdding(requestID: UUID) -> Bool {
    let requestID = requestID.uuidString.lowercased()
    return pending.values.contains { $0.requestID == requestID }
  }

  mutating func completeDevice(
    requestID: String,
    serial: DeviceID,
    delivery: DropDelivery
  ) -> Bool {
    let identity = LibraryDropTargetIdentity.device(serial: serial)
    guard pending[identity]?.requestID == requestID else { return false }
    pending.removeValue(forKey: identity)
    switch delivery {
    case .addedAndSyncing:
      outcome = .addedAndSyncing(serial: serial)
    case .addedForNextSync:
      outcome = .addedForNextSync(serial: serial)
    case .alreadyPresent:
      outcome = .alreadyPresent(serial: serial)
    }
    return true
  }

  mutating func completePlaylist(
    requestID: String, slug: String, appendedTracks: Int
  ) -> Bool {
    let identity = LibraryDropTargetIdentity.manualPlaylist(slug: slug)
    guard pending[identity]?.requestID == requestID else { return false }
    pending.removeValue(forKey: identity)
    outcome = .appended(slug: slug, count: appendedTracks)
    return true
  }

  mutating func reject(
    requestID: String, target identity: LibraryMutationTarget, message: String
  ) -> Bool {
    let targetIdentity: LibraryDropTargetIdentity
    switch identity {
    case .deviceSelection(let serial):
      guard let deviceID = try? DeviceID(serial) else { return false }
      targetIdentity = .device(serial: deviceID)
    case .manualPlaylist(let slug):
      targetIdentity = .manualPlaylist(slug: slug)
    }
    guard let request = pending[targetIdentity], request.requestID == requestID else {
      return false
    }
    pending.removeValue(forKey: targetIdentity)
    outcome = .rejected(target: request.target, message: message)
    return true
  }

  mutating func rejectLocally(requestID: UUID, target: LibraryDropTarget, message: String) {
    let identity = target.identity
    guard let request = pending[identity],
      request.requestID == requestID.uuidString.lowercased(), request.target == target
    else { return }
    pending.removeValue(forKey: identity)
    outcome = .rejected(target: target, message: message)
  }
}
