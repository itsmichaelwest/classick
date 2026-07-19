import Foundation

enum LibraryDropTarget: Hashable, Sendable {
  case device(serial: String, displayName: String)
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
  case addedAndSyncing(serial: String)
  case addedForNextSync(serial: String)
  case alreadyPresent(serial: String)
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
  case device(serial: String)
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

  mutating func completeDevice(
    requestID: String,
    serial: String,
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
      targetIdentity = .device(serial: serial)
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
