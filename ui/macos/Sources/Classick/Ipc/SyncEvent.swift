import Foundation

// MARK: - SyncEvent (inner v1.0.0 `sync_event.line`, `finish` extended in 1.3.0)

/// `finish.skipped_for_space` — whole-album fit-pass deferral rollup. Absent
/// when nothing was deferred this run.
struct SkippedForSpace: Codable, Equatable, Sendable {
  var albums: Int
  var tracks: Int
  var bytes: UInt64
}

/// `finish.artwork` — cover-art embed rollup across this run's
/// Add/Modify/MetadataOnly actions. Absent when the run never reached the
/// apply loop.
struct ArtworkSummary: Codable, Equatable, Sendable {
  var embedded: Int
  var eligible: Int
  var failedSources: Int

  enum CodingKeys: String, CodingKey {
    case embedded
    case eligible
    case failedSources = "failed_sources"
  }
}

enum SyncEvent: Decodable, Sendable {
  case hello(protocolVersion: String, coreVersion: String)
  case header(source: String, ipod: String, manifest: String)
  case summary(
    add: Int, modify: Int, metadataOnly: Int, remove: Int, unchanged: Int, totalPlanned: Int)
  case trackStart(current: Int, total: Int, label: String, etaSecs: UInt64?)
  case trackDone
  case log(message: String)
  case prompt(id: UInt64, message: String, options: [String])
  case form(id: UInt64, label: String, initial: String?, hint: String?)
  case error(message: String, recoveryHints: [String]?)
  /// **Since subprocess protocol 1.3.0:** `skippedForSpace`/`artwork` are
  /// `nil` when absent from the wire (nothing to report); `dbRestored`
  /// defaults `false` when absent (mirroring the wire's own
  /// absent-means-false convention) rather than being optional.
  case finish(
    success: Bool, skippedForSpace: SkippedForSpace?, artwork: ArtworkSummary?, dbRestored: Bool)
  case paused
  case other  // forward-compat: unknown inner types (e.g. `review`)

  private enum CodingKeys: String, CodingKey {
    case type
    case protocolVersion = "protocol_version"
    case coreVersion = "core_version"
    case source
    case ipod
    case manifest
    case add
    case modify
    case metadataOnly = "metadata_only"
    case remove
    case unchanged
    case totalPlanned = "total_planned"
    case current
    case total
    case label
    case message
    case id
    case options
    case initial
    case hint
    case recoveryHints = "recovery_hints"
    case success
    case etaSecs = "eta_secs"
    case skippedForSpace = "skipped_for_space"
    case artwork
    case dbRestored = "db_restored"
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    let type = try container.decode(String.self, forKey: .type)
    switch type {
    case "hello":
      let protocolVersion = try container.decode(String.self, forKey: .protocolVersion)
      let coreVersion = try container.decode(String.self, forKey: .coreVersion)
      self = .hello(protocolVersion: protocolVersion, coreVersion: coreVersion)
    case "header":
      let source = try container.decode(String.self, forKey: .source)
      let ipod = try container.decode(String.self, forKey: .ipod)
      let manifest = try container.decode(String.self, forKey: .manifest)
      self = .header(source: source, ipod: ipod, manifest: manifest)
    case "summary":
      let add = try container.decode(Int.self, forKey: .add)
      let modify = try container.decode(Int.self, forKey: .modify)
      let metadataOnly = try container.decode(Int.self, forKey: .metadataOnly)
      let remove = try container.decode(Int.self, forKey: .remove)
      let unchanged = try container.decode(Int.self, forKey: .unchanged)
      let totalPlanned = try container.decode(Int.self, forKey: .totalPlanned)
      self = .summary(
        add: add, modify: modify, metadataOnly: metadataOnly, remove: remove, unchanged: unchanged,
        totalPlanned: totalPlanned)
    case "track_start":
      let current = try container.decode(Int.self, forKey: .current)
      let total = try container.decode(Int.self, forKey: .total)
      let label = try container.decode(String.self, forKey: .label)
      let etaSecs = try container.decodeIfPresent(UInt64.self, forKey: .etaSecs)
      self = .trackStart(current: current, total: total, label: label, etaSecs: etaSecs)
    case "track_done":
      self = .trackDone
    case "log":
      let message = try container.decode(String.self, forKey: .message)
      self = .log(message: message)
    case "prompt":
      let id = try container.decode(UInt64.self, forKey: .id)
      let message = try container.decode(String.self, forKey: .message)
      let options = try container.decode([String].self, forKey: .options)
      self = .prompt(id: id, message: message, options: options)
    case "form":
      let id = try container.decode(UInt64.self, forKey: .id)
      let label = try container.decode(String.self, forKey: .label)
      let initial = try container.decodeIfPresent(String.self, forKey: .initial)
      let hint = try container.decodeIfPresent(String.self, forKey: .hint)
      self = .form(id: id, label: label, initial: initial, hint: hint)
    case "error":
      let message = try container.decode(String.self, forKey: .message)
      let recoveryHints = try container.decodeIfPresent([String].self, forKey: .recoveryHints)
      self = .error(message: message, recoveryHints: recoveryHints)
    case "finish":
      let success = try container.decode(Bool.self, forKey: .success)
      let skippedForSpace = try container.decodeIfPresent(
        SkippedForSpace.self, forKey: .skippedForSpace)
      let artwork = try container.decodeIfPresent(ArtworkSummary.self, forKey: .artwork)
      let dbRestored = try container.decodeIfPresent(Bool.self, forKey: .dbRestored) ?? false
      self = .finish(
        success: success, skippedForSpace: skippedForSpace, artwork: artwork, dbRestored: dbRestored
      )
    case "paused":
      self = .paused
    default:
      self = .other
    }
  }
}
