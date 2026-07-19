import CoreTransferable
import Foundation
import UniformTypeIdentifiers

extension UTType {
  static let classickLibrarySelection = UTType(
    exportedAs: "st.michaelwe.classick.library-selection", conformingTo: .data)
}

enum LibraryDragPayloadError: Error, Equatable {
  case unsupportedVersion
  case anotherLaunch
  case invalidRuleCount
  case malformedRule
  case duplicateRule
  case nonNormalizedRules
  case invalidSummary
}

struct LibraryDragPayload: Codable, Transferable, Sendable, Equatable {
  static let currentVersion: UInt16 = 1
  static let maximumRules = 64

  let version: UInt16
  let launchNonce: UUID
  let rules: [SelectionRule]
  let summary: String

  static var transferRepresentation: some TransferRepresentation {
    CodableRepresentation(contentType: .classickLibrarySelection)
  }

  static func make(
    rule: SelectionRule, summary: String, launchNonce: UUID
  ) throws -> LibraryDragPayload {
    let normalizedRules = try normalized([rule])
    let normalizedSummary = summary.trimmingCharacters(in: .whitespacesAndNewlines)
    guard isValidSummary(normalizedSummary) else {
      throw LibraryDragPayloadError.invalidSummary
    }
    return LibraryDragPayload(
      version: currentVersion, launchNonce: launchNonce,
      rules: normalizedRules, summary: normalizedSummary)
  }

  func validated(expectedNonce: UUID) throws -> [SelectionRule] {
    guard version == Self.currentVersion else {
      throw LibraryDragPayloadError.unsupportedVersion
    }
    guard launchNonce == expectedNonce else {
      throw LibraryDragPayloadError.anotherLaunch
    }
    guard (1...Self.maximumRules).contains(rules.count) else {
      throw LibraryDragPayloadError.invalidRuleCount
    }
    let normalizedRules = try Self.normalized(rules)
    guard normalizedRules == rules else {
      throw LibraryDragPayloadError.nonNormalizedRules
    }
    guard Self.isValidSummary(summary),
          summary == summary.trimmingCharacters(in: .whitespacesAndNewlines)
    else {
      throw LibraryDragPayloadError.invalidSummary
    }
    return normalizedRules
  }

  private static func normalized(_ rules: [SelectionRule]) throws -> [SelectionRule] {
    guard (1...maximumRules).contains(rules.count) else {
      throw LibraryDragPayloadError.invalidRuleCount
    }

    var keys = Set<String>()
    var result: [SelectionRule] = []
    for rule in rules {
      let normalized: SelectionRule
      let key: String
      switch rule {
      case .artist(let name):
        let name = try component(name)
        normalized = .artist(name: name)
        key = "artist\u{0}\(name.lowercased())"
      case .album(let artist, let album):
        let artist = try component(artist)
        let album = try component(album)
        normalized = .album(artist: artist, album: album)
        key = "album\u{0}\(artist.lowercased())\u{0}\(album.lowercased())"
      case .genre(let name):
        let name = try component(name)
        normalized = .genre(name: name)
        key = "genre\u{0}\(name.lowercased())"
      }
      guard keys.insert(key).inserted else {
        throw LibraryDragPayloadError.duplicateRule
      }
      result.append(normalized)
    }
    return result.sorted(by: ruleComesBefore)
  }

  private static func component(_ value: String) throws -> String {
    let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !normalized.isEmpty, normalized.unicodeScalars.count <= 256 else {
      throw LibraryDragPayloadError.malformedRule
    }
    return normalized
  }

  private static func isValidSummary(_ value: String) -> Bool {
    !value.isEmpty && value.count <= 128
  }

  private static func ruleComesBefore(_ lhs: SelectionRule, _ rhs: SelectionRule) -> Bool {
    let left = sortComponents(lhs)
    let right = sortComponents(rhs)
    for (a, b) in zip(left, right) where a != b {
      let lowerA = a.lowercased()
      let lowerB = b.lowercased()
      if lowerA != lowerB { return lowerA < lowerB }
      return a < b
    }
    return left.count < right.count
  }

  private static func sortComponents(_ rule: SelectionRule) -> [String] {
    switch rule {
    case .artist(let name): return ["0", name]
    case .album(let artist, let album): return ["1", artist, album]
    case .genre(let name): return ["2", name]
    }
  }
}
