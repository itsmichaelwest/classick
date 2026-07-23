import Foundation

struct IpodIdentity: Codable, Equatable, Sendable {
  var serial: String
  var modelLabel: String
  var name: String?
  /// **Since daemon protocol 1.5.0.** Default `false` (shared selection).
  /// `true` routes this device's selection to its own per-device
  /// `devices/<serial>/selection.json` instead of the shared file. Rides
  /// the existing `ipod` field on `config_update`/`save_config` — see
  /// docs/ipc-protocol.md "IpodIdentity gains custom_selection". Every
  /// Swift construction site MUST thread through the existing value (never
  /// a bare default) or a save silently resets it — see the 0.2.1
  /// wizard-clobber lesson this mirrors for `rockboxCompat`.
  var customSelection: Bool

  enum CodingKeys: String, CodingKey {
    case serial
    case modelLabel = "model_label"
    case name
    case customSelection = "custom_selection"
  }

  init(serial: String, modelLabel: String, name: String? = nil, customSelection: Bool = false) {
    self.serial = serial
    self.modelLabel = modelLabel
    self.name = name
    self.customSelection = customSelection
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    serial = try container.decode(String.self, forKey: .serial)
    modelLabel = try container.decode(String.self, forKey: .modelLabel)
    name = try container.decodeIfPresent(String.self, forKey: .name)
    customSelection = try container.decode(Bool.self, forKey: .customSelection)
  }
}

struct DaemonSettings: Codable, Equatable, Sendable {
  var enabled: Bool
  var autostartWithWindows: Bool
  var firstSyncMode: String  // "review" | "auto_apply"
  var subsequentSyncMode: String  // "review" | "auto_apply"
  var scheduleMinutes: UInt32
  var notifyOn: String  // "all" | "errors_only" | "none"
  var rockboxCompat: Bool
  var dropSyncBehavior: DropSyncBehaviorWire

  enum CodingKeys: String, CodingKey {
    case enabled
    case autostartWithWindows = "autostart_with_windows"
    case firstSyncMode = "first_sync_mode"
    case subsequentSyncMode = "subsequent_sync_mode"
    case scheduleMinutes = "schedule_minutes"
    case notifyOn = "notify_on"
    case rockboxCompat = "rockbox_compat"
    case dropSyncBehavior = "drop_sync_behavior"
  }

  init(
    enabled: Bool,
    autostartWithWindows: Bool,
    firstSyncMode: String,
    subsequentSyncMode: String,
    scheduleMinutes: UInt32,
    notifyOn: String,
    rockboxCompat: Bool = false,
    dropSyncBehavior: DropSyncBehaviorWire = .immediate
  ) {
    self.enabled = enabled
    self.autostartWithWindows = autostartWithWindows
    self.firstSyncMode = firstSyncMode
    self.subsequentSyncMode = subsequentSyncMode
    self.scheduleMinutes = scheduleMinutes
    self.notifyOn = notifyOn
    self.rockboxCompat = rockboxCompat
    self.dropSyncBehavior = dropSyncBehavior
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    enabled = try container.decode(Bool.self, forKey: .enabled)
    autostartWithWindows = try container.decode(Bool.self, forKey: .autostartWithWindows)
    firstSyncMode = try container.decode(String.self, forKey: .firstSyncMode)
    subsequentSyncMode = try container.decode(String.self, forKey: .subsequentSyncMode)
    scheduleMinutes = try container.decode(UInt32.self, forKey: .scheduleMinutes)
    notifyOn = try container.decode(String.self, forKey: .notifyOn)
    rockboxCompat = try container.decode(Bool.self, forKey: .rockboxCompat)
    dropSyncBehavior = try container.decodeIfPresent(
      DropSyncBehaviorWire.self, forKey: .dropSyncBehavior) ?? .immediate
  }
}

/// The subset of the daemon's `SyncSummary` (persisted `HistoryEntry.summary`)
/// this app needs. `add`/`modify`/`remove`/`unchanged`/`skipped`/
/// `metadata_only` are present on the wire but decoded leniently —
/// JSONDecoder ignores unknown keys, and they aren't needed for display
/// beyond `outcome`. **Since daemon protocol 1.5.0**: the three fields below,
/// each `#[serde(default)]` on the Rust side so pre-1.5.0 `history.json`
/// entries (or a `summary` object from an older daemon) deserialize to `0`.
struct SyncSummaryInfo: Codable, Equatable, Sendable {
  var skippedForSpaceTracks: Int
  var skippedForSpaceBytes: UInt64
  var artworkFailedSources: Int

  enum CodingKeys: String, CodingKey {
    case skippedForSpaceTracks = "skipped_for_space_tracks"
    case skippedForSpaceBytes = "skipped_for_space_bytes"
    case artworkFailedSources = "artwork_failed_sources"
  }

  init(
    skippedForSpaceTracks: Int = 0, skippedForSpaceBytes: UInt64 = 0, artworkFailedSources: Int = 0
  ) {
    self.skippedForSpaceTracks = skippedForSpaceTracks
    self.skippedForSpaceBytes = skippedForSpaceBytes
    self.artworkFailedSources = artworkFailedSources
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    skippedForSpaceTracks =
      try container.decodeIfPresent(Int.self, forKey: .skippedForSpaceTracks) ?? 0
    skippedForSpaceBytes =
      try container.decodeIfPresent(UInt64.self, forKey: .skippedForSpaceBytes) ?? 0
    artworkFailedSources =
      try container.decodeIfPresent(Int.self, forKey: .artworkFailedSources) ?? 0
  }
}

struct HistoryEntry: Codable, Equatable, Sendable {
  var serial: String
  var sessionID: UInt64?
  var timestamp: String
  var durationSecs: UInt64
  var trigger: String
  var outcome: String
  /// Absent on the wire when the run never reached a summarizable state
  /// (e.g. aborted before planning). **Since 1.5.0** it also carries the
  /// skipped-for-space + artwork-failure rollups — see `SyncSummaryInfo`.
  var summary: SyncSummaryInfo?
  /// **Since daemon protocol 1.5.0.** Mirrors the subprocess `finish`
  /// event's `db_restored` (§4.11) for that run. Omitted on the wire (not
  /// `false`) when it didn't fire, matching the subprocess field's own
  /// old-client-compat convention — decode absence as `false`.
  var dbRestored: Bool

  enum CodingKeys: String, CodingKey {
    case serial
    case sessionID = "session_id"
    case timestamp
    case durationSecs = "duration_secs"
    case trigger
    case outcome
    case summary
    case dbRestored = "db_restored"
  }

  init(
    serial: String, sessionID: UInt64? = nil,
    timestamp: String, durationSecs: UInt64, trigger: String, outcome: String,
    summary: SyncSummaryInfo? = nil, dbRestored: Bool = false
  ) {
    self.serial = serial
    self.sessionID = sessionID
    self.timestamp = timestamp
    self.durationSecs = durationSecs
    self.trigger = trigger
    self.outcome = outcome
    self.summary = summary
    self.dbRestored = dbRestored
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    serial = try container.decode(String.self, forKey: .serial)
    sessionID = try container.decodeIfPresent(UInt64.self, forKey: .sessionID)
    timestamp = try container.decode(String.self, forKey: .timestamp)
    durationSecs = try container.decode(UInt64.self, forKey: .durationSecs)
    trigger = try container.decode(String.self, forKey: .trigger)
    outcome = try container.decode(String.self, forKey: .outcome)
    summary = try container.decodeIfPresent(SyncSummaryInfo.self, forKey: .summary)
    dbRestored = try container.decodeIfPresent(Bool.self, forKey: .dbRestored) ?? false
  }
}

struct StatusInfo: Equatable, Sendable {
  enum State: String, Codable, Sendable { case idle, syncing, scanning }

  /// Wire keys are `free_bytes`/`total_bytes` (Rust `StorageInfo`).
  /// This struct once decoded bare `free`/`total`. When the macOS storage
  /// implementation landed daemon-side, the mismatch made
  /// EVERY `status_update` with a connected iPod fail to decode, so the
  /// app silently dropped all status while a device was plugged in
  /// (phase stuck at .notConfigured, sync state never shown). Keys are
  /// pinned by `testDecodesStatusUpdateWithStorageFromLiveWireCapture`.
  struct Storage: Codable, Equatable, Sendable {
    var free: UInt64
    var total: UInt64

    enum CodingKeys: String, CodingKey {
      case free = "free_bytes"
      case total = "total_bytes"
    }
  }

  var state: State
  var configured: Bool
  var ipodConnected: Bool
  var lastSync: HistoryEntry?
  var nextScheduledUnixSecs: UInt64?
  var storage: Storage?
  var syncedCount: Int = 0  // X in "X of Y synced" — manifest track count
  var libraryCount: Int?  // Y — source-library track count; nil until known
  var acknowledgedRequestID: String? = nil
}

extension StatusInfo: Codable {
  enum CodingKeys: String, CodingKey {
    case state
    case configured
    case ipodConnected = "ipod_connected"
    case lastSync = "last_sync"
    case nextScheduledUnixSecs = "next_scheduled_unix_secs"
    case storage
    case syncedCount = "synced_count"
    case libraryCount = "library_count"
    case acknowledgedRequestID = "acknowledged_request_id"
  }
}

struct DeviceIdentityWire: Codable, Equatable, Sendable {
  var serial: String
  var modelLabel: String
  var name: String?

  enum CodingKeys: String, CodingKey {
    case serial
    case modelLabel = "model_label"
    case name
  }
}

enum DevicePhaseLabel: String, Codable, Equatable, Sendable {
  case disconnected, unconfigured, idle, syncing, paused, error
}

struct DeviceSnapshotWire: Codable, Equatable, Sendable {
  var identity: DeviceIdentityWire
  var configured: Bool
  var connected: Bool
  var mount: String?
  var phase: DevicePhaseLabel
  var sessionID: UInt64?
  var storage: StatusInfo.Storage?
  var syncedCount: Int
  var libraryCount: Int?
  var latestSuccessfulSync: HistoryEntry?
  var latestAttempt: HistoryEntry?
  var lastTerminalError: String?
  var selectionRevision: UInt64
  var settingsRevision: UInt64
  var subscriptionsRevision: UInt64

  enum CodingKeys: String, CodingKey {
    case identity, configured, connected, mount, phase, storage
    case sessionID = "session_id"
    case syncedCount = "synced_count"
    case libraryCount = "library_count"
    case latestSuccessfulSync = "latest_successful_sync"
    case latestAttempt = "latest_attempt"
    case lastTerminalError = "last_terminal_error"
    case selectionRevision = "selection_revision"
    case settingsRevision = "settings_revision"
    case subscriptionsRevision = "subscriptions_revision"
  }
}

struct DeviceInventorySnapshot: Codable, Equatable, Sendable {
  var revision: UInt64
  var devices: [DeviceSnapshotWire]
}

enum SourceAvailabilityState: String, Decodable, Equatable, Sendable {
  case available
  case remounting
  case authRequired = "auth_required"
  case unavailable
}

struct SourceAvailabilityInfo: Equatable, Sendable {
  var state: SourceAvailabilityState
  var sourceRoot: String?
  var acknowledgedRequestID: String?
}

enum DropDelivery: String, Decodable, Equatable, Sendable {
  case addedAndSyncing = "added_and_syncing"
  case addedForNextSync = "added_for_next_sync"
  case alreadyPresent = "already_present"
}

typealias DeviceSelectionDelivery = DropDelivery

struct DeviceSelectionAddedInfo: Equatable, Sendable {
  var acknowledgedRequestID: String
  var serial: String
  var matchedTracks: Int
  var missingTracks: Int
  var selectionChanged: Bool
  var selectionRevision: UInt64
  var selection: SelectionState
  var delivery: DropDelivery
}

struct PlaylistSelectionAppendedInfo: Equatable, Sendable {
  var acknowledgedRequestID: String
  var slug: String
  var appendedTracks: Int
  var playlistRevision: UInt64
  var playlist: ManualPlaylistWire
}

struct ManualPlaylistWire: Decodable, Equatable, Sendable {
  var slug: String
  var name: String
  var tracks: [String]
}

enum LibraryMutationTarget: Decodable, Equatable, Sendable {
  case deviceSelection(serial: String)
  case manualPlaylist(slug: String)

  private enum CodingKeys: String, CodingKey { case kind, serial, slug }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    switch try container.decode(String.self, forKey: .kind) {
    case "device_selection":
      self = .deviceSelection(serial: try container.decode(String.self, forKey: .serial))
    case "manual_playlist":
      self = .manualPlaylist(slug: try container.decode(String.self, forKey: .slug))
    case let kind:
      throw DecodingError.dataCorruptedError(
        forKey: .kind, in: container, debugDescription: "unknown mutation target \(kind)")
    }
  }
}

struct LibraryMutationRejectedInfo: Equatable, Sendable {
  var acknowledgedRequestID: String
  var target: LibraryMutationTarget
  var code: String
  var message: String
}



// MARK: - Sync progress presentation values

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

enum SyncStopReason: String, Decodable, Equatable, Sendable {
  case cancelled
  case paused
}
