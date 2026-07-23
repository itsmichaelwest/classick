import Foundation

enum WireV3DurableIntentKey: Hashable, Sendable {
  case sourceLocation
  case globalSettings
  case adoption(DeviceID)
  case deviceRemoval(DeviceID)
  case deviceSelection(DeviceID)
  case deviceSettings(DeviceID)
  case deviceSubscriptions(DeviceID)
  case playlist(String)
  case deviceSelectionAddition(DeviceID)
  case playlistAppend(String)
}

struct WireV3DurableAcknowledgement: Equatable, Sendable {
  let requestID: UUID
  let revision: UInt64?
  let target: WireV3DurableIntentKey?
  let terminalFailure: Bool
}

extension WireV3Command {
  var durableIntentKey: WireV3DurableIntentKey? {
    switch self {
    case .setSourceLocation: .sourceLocation
    case .setGlobalSettings: .globalSettings
    case .adoptDevice(let deviceID, _, _, _, _, _, _, _): .adoption(deviceID)
    case .forgetDevice(let deviceID, _): .deviceRemoval(deviceID)
    case .setSelection(let deviceID, _, _, _): .deviceSelection(deviceID)
    case .setSettings(let deviceID, _, _, _): .deviceSettings(deviceID)
    case .setSubscriptions(let deviceID, _, _, _): .deviceSubscriptions(deviceID)
    case .savePlaylist(_, let playlist): .playlist(playlist.v3Slug ?? "new:\(requestID)")
    case .deletePlaylist(_, let slug): .playlist(slug)
    case .addSelectionToDevice(let deviceID, _, _, _): .deviceSelectionAddition(deviceID)
    case .appendSelectionToPlaylist(_, let slug, _): .playlistAppend(slug)
    default: nil
    }
  }

  var additiveRules: [SelectionRule]? {
    switch self {
    case .addSelectionToDevice(_, _, _, let rules),
      .appendSelectionToPlaylist(_, _, let rules): rules
    default: nil
    }
  }

  func replacingAdditiveRules(_ rules: [SelectionRule]) -> WireV3Command {
    switch self {
    case .addSelectionToDevice(let deviceID, let requestID, let mutationID, _):
      .addSelectionToDevice(
        deviceID: deviceID, requestID: requestID, mutationID: mutationID, rules: rules)
    case .appendSelectionToPlaylist(let requestID, let slug, _):
      .appendSelectionToPlaylist(requestID: requestID, slug: slug, rules: rules)
    default: self
    }
  }

  func normalizedForDurableEncoding() -> WireV3Command {
    guard case .savePlaylist(let requestID, let playlist) = self else { return self }
    switch playlist {
    case .manual(nil, let name, let tracks):
      return .savePlaylist(
        requestID: requestID,
        playlist: .manual(
          slug: Self.stableCreateSlug(name: name, requestID: requestID),
          name: name, tracks: tracks))
    case .smart(nil, let name, let rules):
      return .savePlaylist(
        requestID: requestID,
        playlist: .smart(
          slug: Self.stableCreateSlug(name: name, requestID: requestID),
          name: name, rules: rules))
    case .manual, .smart: return self
    }
  }

  func isCommitted(by acknowledgement: WireV3DurableAcknowledgement) -> Bool {
    guard acknowledgement.requestID == requestID else { return false }
    if acknowledgement.terminalFailure {
      return acknowledgement.target == durableIntentKey
    }
    return acknowledgement.revision != nil || acknowledgement.target == durableIntentKey
  }

  static func canonicalAdditiveRules(_ rules: [SelectionRule]) -> [SelectionRule] {
    let unique = Dictionary(grouping: rules, by: additiveRuleKey).compactMap { $0.value.first }
    let artists = Set(unique.compactMap { rule -> String? in
      guard case .artist(let name) = rule else { return nil }
      return name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    })
    return unique.filter { rule in
      guard case .album(let artist, _) = rule else { return true }
      return !artists.contains(artist.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())
    }.sorted { additiveRuleKey($0) < additiveRuleKey($1) }
  }

  private static func additiveRuleKey(_ rule: SelectionRule) -> String {
    switch rule {
    case .artist(let name):
      "0\u{0}\(name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())"
    case .album(let artist, let album):
      "1\u{0}\(artist.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())\u{0}\(album.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())"
    case .genre(let name):
      "2\u{0}\(name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased())"
    }
  }

  private static func stableCreateSlug(name: String, requestID: UUID) -> String {
    "\(slugify(name))-\(slugify(requestID.uuidString.lowercased()))"
  }

  private static func slugify(_ value: String) -> String {
    var result = ""
    var lastWasSeparator = true
    for character in value {
      if character.isASCII, character.isLetter || character.isNumber {
        result += character.lowercased()
        lastWasSeparator = false
      } else if !lastWasSeparator {
        result.append("-")
        lastWasSeparator = true
      }
    }
    while result.hasSuffix("-") { result.removeLast() }
    return result.isEmpty ? "playlist" : result
  }
}

extension WireV3Event {
  var durableAcknowledgement: WireV3DurableAcknowledgement? {
    switch self {
    case .globalConfig(let event):
      event.requestID.map {
        WireV3DurableAcknowledgement(
          requestID: $0, revision: event.revision, target: nil, terminalFailure: false)
      }
    case .deviceConfig(let event):
      event.requestID.map {
        WireV3DurableAcknowledgement(
          requestID: $0,
          revision: max(event.selection.revision, event.settings.revision, event.subscriptions.revision),
          target: nil, terminalFailure: false)
      }
    case .deviceForgotten(let event):
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: 1,
        target: .deviceRemoval(event.deviceID), terminalFailure: false)
    case .playlistSaved(let event):
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: event.revision, target: nil, terminalFailure: false)
    case .playlists(let event):
      event.requestID.map {
        WireV3DurableAcknowledgement(
          requestID: $0, revision: event.revision, target: nil, terminalFailure: false)
      }
    case .deviceSelectionAdded(let event):
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: event.selectionRevision,
        target: .deviceSelectionAddition(event.deviceID), terminalFailure: false)
    case .playlistSelectionAppended(let event):
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: event.revision,
        target: .playlistAppend(event.slug), terminalFailure: false)
    case .libraryMutationRejected(let event):
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: nil,
        target: event.target.durableIntentKey, terminalFailure: true)
    case .configMutationFailed(let event) where event.stage == "host_acceptance":
      WireV3DurableAcknowledgement(
        requestID: event.requestID, revision: nil,
        target: event.component.durableIntentKey(deviceID: event.deviceID), terminalFailure: true)
    default: nil
    }
  }
}

extension WireV3LibraryMutationTarget {
  fileprivate var durableIntentKey: WireV3DurableIntentKey {
    switch self {
    case .deviceSelection(let deviceID): .deviceSelectionAddition(deviceID)
    case .manualPlaylist(let slug): .playlistAppend(slug)
    }
  }
}

extension String {
  fileprivate func durableIntentKey(deviceID: DeviceID) -> WireV3DurableIntentKey? {
    switch self {
    case "selection": .deviceSelection(deviceID)
    case "settings": .deviceSettings(deviceID)
    case "subscriptions": .deviceSubscriptions(deviceID)
    default: nil
    }
  }
}

extension PlaylistPayload {
  fileprivate var v3Slug: String? {
    switch self {
    case .manual(let slug, _, _), .smart(let slug, _, _): slug
    }
  }
}
