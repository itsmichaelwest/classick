import Foundation

struct WireV3GlobalConfigEvent: Decodable, Equatable, Sendable {
  let requestID: UUID?
  let revision: UInt64
  let sourceRoot: String?
  let settings: WireV3GlobalSettings

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case revision
    case sourceRoot = "source_root"
    case settings
  }
}

struct WireV3SourceAvailabilityEvent: Decodable, Equatable, Sendable {
  let requestID: UUID?
  let state: SourceAvailabilityState
  let sourceRoot: String?

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case state
    case sourceRoot = "source_root"
  }
}

struct WireV3InventorySubscriptionEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let subscribed: Bool

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case subscribed
  }
}

struct WireV3ConfigMutationFailedEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID
  let mutationID: UUID
  let component: String
  let stage: String
  let message: String

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
    case mutationID = "mutation_id"
    case component, stage, message
  }
}

struct WireV3DeviceForgottenEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
  }
}

struct WireV3SyncAcceptedEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let sessionID: UInt64
  let requestID: UUID
  let operation: String

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case sessionID = "session_id"
    case requestID = "request_id"
    case operation
  }
}

struct WireV3SyncRejectedEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID
  let operation: String
  let reason: String
  let message: String

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
    case operation, reason, message
  }
}

struct WireV3HistoryEntry: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let sessionID: UInt64?
  let timestamp: String
  let durationSecs: UInt64
  let trigger: String
  let operation: String
  let outcome: String
  let errorMessage: String?
  let summary: SyncSummaryInfo?
  let dbRestored: Bool

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case sessionID = "session_id"
    case timestamp
    case durationSecs = "duration_secs"
    case trigger, operation, outcome
    case errorMessage = "error_message"
    case summary
    case dbRestored = "db_restored"
  }

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    deviceID = try c.decode(DeviceID.self, forKey: .deviceID)
    sessionID = try c.decodeIfPresent(UInt64.self, forKey: .sessionID)
    timestamp = try c.decode(String.self, forKey: .timestamp)
    durationSecs = try c.decode(UInt64.self, forKey: .durationSecs)
    trigger = try c.decode(String.self, forKey: .trigger)
    operation = try c.decode(String.self, forKey: .operation)
    outcome = try c.decode(String.self, forKey: .outcome)
    errorMessage = try c.decodeIfPresent(String.self, forKey: .errorMessage)
    summary = try c.decodeIfPresent(SyncSummaryInfo.self, forKey: .summary)
    dbRestored = try c.decodeIfPresent(Bool.self, forKey: .dbRestored) ?? false
  }

  var appValue: HistoryEntry {
    HistoryEntry(
      serial: deviceID.rawValue, sessionID: sessionID, timestamp: timestamp,
      durationSecs: durationSecs, trigger: trigger, outcome: outcome,
      summary: summary, dbRestored: dbRestored)
  }
}

struct WireV3HistoryEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let entries: [WireV3HistoryEntry]

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case entries
  }
}

struct WireV3LibraryEvent: Decodable, Equatable, Sendable {
  let requestID: UUID?
  let sourceRoot: String?
  let scannedAtUnixSecs: UInt64?
  let artists: [LibraryArtist]
  let genres: [LibraryGenre]
  let totalTracks: Int
  let totalBytes: UInt64

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case sourceRoot = "source_root"
    case scannedAtUnixSecs = "scanned_at_unix_secs"
    case artists, genres
    case totalTracks = "total_tracks"
    case totalBytes = "total_bytes"
  }
}

struct WireV3SelectionPreviewEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID
  let selectedTracks: Int
  let selectedBytes: UInt64
  let adds: Int
  let removes: Int

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
    case selectedTracks = "selected_tracks"
    case selectedBytes = "selected_bytes"
    case adds, removes
  }
}

struct WireV3DevicePreviewEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID
  let selectedTracks: Int
  let selectedBytes: UInt64
  let playlistExtraTracks: Int
  let playlistExtraBytes: UInt64
  let projectedFreeBytes: UInt64?
  let unresolvedSubscriptions: [String]

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
    case selectedTracks = "selected_tracks"
    case selectedBytes = "selected_bytes"
    case playlistExtraTracks = "playlist_extra_tracks"
    case playlistExtraBytes = "playlist_extra_bytes"
    case projectedFreeBytes = "projected_free_bytes"
    case unresolvedSubscriptions = "unresolved_subscriptions"
  }
}

struct WireV3ResolvedTracksEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let tracks: [String]

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case tracks
  }
}

struct WireV3PlaylistsEvent: Decodable, Equatable, Sendable {
  let requestID: UUID?
  let revision: UInt64
  let playlists: [PlaylistSummary]

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case revision, playlists
  }
}

enum WireV3StoredPlaylist: Decodable, Equatable, Sendable {
  case manual(slug: String, name: String, tracks: [String])
  case smart(slug: String, name: String, rules: SmartRulesWire)

  private enum CodingKeys: String, CodingKey { case kind, slug, name, tracks, rules }

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    let slug = try c.decode(String.self, forKey: .slug)
    let name = try c.decode(String.self, forKey: .name)
    switch try c.decode(PlaylistKind.self, forKey: .kind) {
    case .manual:
      self = .manual(slug: slug, name: name, tracks: try c.decode([String].self, forKey: .tracks))
    case .smart:
      self = .smart(slug: slug, name: name, rules: try c.decode(SmartRulesWire.self, forKey: .rules))
    }
  }

  var detail: (slug: String, name: String, kind: PlaylistKind, tracks: [String]?, rules: SmartRulesWire?) {
    switch self {
    case .manual(let slug, let name, let tracks): (slug, name, .manual, tracks, nil)
    case .smart(let slug, let name, let rules): (slug, name, .smart, nil, rules)
    }
  }
}

enum WireV3PlaylistDetailResult: Decodable, Equatable, Sendable {
  case found(WireV3StoredPlaylist)
  case unavailable(String)

  private enum CodingKeys: String, CodingKey { case state, playlist, message }

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    switch try c.decode(String.self, forKey: .state) {
    case "found": self = .found(try c.decode(WireV3StoredPlaylist.self, forKey: .playlist))
    case "unavailable": self = .unavailable(try c.decode(String.self, forKey: .message))
    case let state:
      throw DecodingError.dataCorruptedError(
        forKey: .state, in: c, debugDescription: "unknown playlist result \(state)")
    }
  }
}

struct WireV3PlaylistDetailEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let revision: UInt64
  let slug: String
  let result: WireV3PlaylistDetailResult

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case revision, slug, result
  }
}

struct WireV3PlaylistSavedEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let revision: UInt64
  let playlist: WireV3StoredPlaylist

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case revision, playlist
  }
}

struct WireV3DeviceSelectionAddedEvent: Decodable, Equatable, Sendable {
  let deviceID: DeviceID
  let requestID: UUID
  let mutationID: UUID
  let matchedTracks: Int
  let missingTracks: Int
  let selectionChanged: Bool
  let selectionRevision: UInt64
  let selection: WireV3SelectionValue
  let delivery: WireV3Delivery
  let sync: WireV3DropSyncResult

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case requestID = "request_id"
    case mutationID = "mutation_id"
    case matchedTracks = "matched_tracks"
    case missingTracks = "missing_tracks"
    case selectionChanged = "selection_changed"
    case selectionRevision = "selection_revision"
    case selection, delivery, sync
  }

  var dropDelivery: DropDelivery {
    switch sync {
    case .started: .addedAndSyncing
    case .nextSync: .addedForNextSync
    case .alreadyPresent: .alreadyPresent
    }
  }
}

enum WireV3DropSyncResult: Decodable, Equatable, Sendable {
  case started(sessionID: UInt64)
  case nextSync
  case alreadyPresent

  private enum CodingKeys: String, CodingKey { case started }
  private enum StartedKeys: String, CodingKey { case sessionID = "session_id" }

  init(from decoder: Decoder) throws {
    if let value = try? decoder.singleValueContainer().decode(String.self) {
      switch value {
      case "next_sync": self = .nextSync
      case "already_present": self = .alreadyPresent
      default:
        throw DecodingError.dataCorrupted(
          .init(codingPath: decoder.codingPath, debugDescription: "unknown sync result \(value)"))
      }
      return
    }
    let container = try decoder.container(keyedBy: CodingKeys.self)
    let started = try container.nestedContainer(keyedBy: StartedKeys.self, forKey: .started)
    self = .started(sessionID: try started.decode(UInt64.self, forKey: .sessionID))
  }
}

struct WireV3PlaylistSelectionAppendedEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let slug: String
  let appendedTracks: Int
  let revision: UInt64
  let playlist: WireV3StoredPlaylist

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case slug
    case appendedTracks = "appended_tracks"
    case revision, playlist
  }
}

enum WireV3LibraryMutationTarget: Decodable, Equatable, Sendable {
  case deviceSelection(DeviceID)
  case manualPlaylist(String)

  private enum CodingKeys: String, CodingKey { case kind, deviceID = "device_id", slug }

  init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    switch try c.decode(String.self, forKey: .kind) {
    case "device_selection": self = .deviceSelection(try c.decode(DeviceID.self, forKey: .deviceID))
    case "manual_playlist": self = .manualPlaylist(try c.decode(String.self, forKey: .slug))
    case let kind:
      throw DecodingError.dataCorruptedError(
        forKey: .kind, in: c, debugDescription: "unknown library mutation target \(kind)")
    }
  }
}

struct WireV3LibraryMutationRejectedEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let target: WireV3LibraryMutationTarget
  let code: String
  let message: String

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case target, code, message
  }
}

struct WireV3RequestEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  enum CodingKeys: String, CodingKey { case requestID = "request_id" }
}

struct WireV3CommandFailedEvent: Decodable, Equatable, Sendable {
  let requestID: UUID
  let message: String
  enum CodingKeys: String, CodingKey { case requestID = "request_id"; case message }
}
