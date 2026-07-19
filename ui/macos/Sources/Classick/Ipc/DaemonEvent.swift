import Foundation

// Wire contract: docs/ipc-protocol.md (inner `sync_event.line`, v1.0.0) and
// the daemon-pipe protocol described in AGENTS.md / the macOS app plan's
// "Global Constraints". Every command/event is a JSON object with a
// snake_case "type" discriminator. Field names here are verbatim copies of
// the Rust wire names — do not rename without a corresponding Rust change.

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

  enum CodingKeys: String, CodingKey {
    case enabled
    case autostartWithWindows = "autostart_with_windows"
    case firstSyncMode = "first_sync_mode"
    case subsequentSyncMode = "subsequent_sync_mode"
    case scheduleMinutes = "schedule_minutes"
    case notifyOn = "notify_on"
    case rockboxCompat = "rockbox_compat"
  }

  init(
    enabled: Bool,
    autostartWithWindows: Bool,
    firstSyncMode: String,
    subsequentSyncMode: String,
    scheduleMinutes: UInt32,
    notifyOn: String,
    rockboxCompat: Bool = false
  ) {
    self.enabled = enabled
    self.autostartWithWindows = autostartWithWindows
    self.firstSyncMode = firstSyncMode
    self.subsequentSyncMode = subsequentSyncMode
    self.scheduleMinutes = scheduleMinutes
    self.notifyOn = notifyOn
    self.rockboxCompat = rockboxCompat
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

// MARK: - DaemonEvent (received)

enum DaemonEvent: Decodable, Sendable {
  case hello(protocolVersion: String, coreVersion: String)
  case statusUpdate(StatusInfo)
  case configUpdate(
    source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?, configRevision: UInt64,
    acknowledgedRequestID: String?)
  case historyUpdate(entries: [HistoryEntry], acknowledgedRequestID: String)
  case deviceConnected(serial: String, modelLabel: String, drive: String, name: String?)
  case deviceDisconnected(serial: String)
  case syncRejected(reason: String, serial: String, acknowledgedRequestID: String)
  case syncEvent(line: String, serial: String?, sessionID: UInt64)
  case deviceInventorySnapshot(DeviceInventorySnapshot)
  case libraryUpdate(LibraryInfo)
  case selectionUpdate(
    mode: SelectionMode, rules: [SelectionRule], serial: String?, acknowledgedRequestID: String?)
  case selectionPreview(SelectionPreviewInfo)
  // MARK: Protocol 2.0.0 — correlated playlists and per-device replies
  case playlistsUpdate(
    [PlaylistSummary], playlistRevision: UInt64, acknowledgedRequestID: String?)
  case playlistDetail(PlaylistDetail)
  case deviceConfigUpdate(
    serial: String, selection: SelectionState, subscriptions: SubscriptionsWire,
    settings: DeviceSettingsWire, selectionRevision: UInt64, settingsRevision: UInt64,
    subscriptionsRevision: UInt64, acknowledgedRequestID: String)
  case devicePreview(DevicePreview)
  case sourceAvailability(SourceAvailabilityInfo)
  /// Reply to `resolve_tracks`. An empty `tracks` array is a valid reply
  /// (no rule matched anything in the index), not an error.
  case resolvedTracks(tracks: [String], acknowledgedRequestID: String)
  case commandFailed(acknowledgedRequestID: String, error: String)
  case deviceSelectionAdded(DeviceSelectionAddedInfo)
  case playlistSelectionAppended(PlaylistSelectionAppendedInfo)
  case libraryMutationRejected(LibraryMutationRejectedInfo)
  case unknown  // forward-compat: log + ignore

  private enum CodingKeys: String, CodingKey {
    case type
    case protocolVersion = "protocol_version"
    case coreVersion = "core_version"
    case state
    case configured
    case ipodConnected = "ipod_connected"
    case lastSync = "last_sync"
    case nextScheduledUnixSecs = "next_scheduled_unix_secs"
    case storage
    case source
    case daemon
    case ipod
    case entries
    case serial
    case modelLabel = "model_label"
    case drive
    case name
    case reason
    case line
    case syncedCount = "synced_count"
    case libraryCount = "library_count"
    case sourceRoot = "source_root"
    case scannedAtUnixSecs = "scanned_at_unix_secs"
    case artists
    case genres
    case totalTracks = "total_tracks"
    case totalBytes = "total_bytes"
    case mode
    case rules
    case selectedTracks = "selected_tracks"
    case selectedBytes = "selected_bytes"
    case adds
    case removes
    case playlists, playlist
    case slug
    case kind
    case tracks
    case error
    case selection
    case subscriptions
    case settings
    case playlistExtraTracks = "playlist_extra_tracks"
    case playlistExtraBytes = "playlist_extra_bytes"
    case projectedFreeBytes = "projected_free_bytes"
    case unresolvedSubscriptions = "unresolved_subscriptions"
    case revision
    case devices
    case sessionID = "session_id"
    case configRevision = "config_revision"
    case playlistRevision = "playlist_revision"
    case selectionRevision = "selection_revision"
    case settingsRevision = "settings_revision"
    case subscriptionsRevision = "subscriptions_revision"
    case acknowledgedRequestID = "acknowledged_request_id"
    case matchedTracks = "matched_tracks"
    case missingTracks = "missing_tracks"
    case selectionChanged = "selection_changed"
    case appendedTracks = "appended_tracks"
    case delivery, target, code, message
  }

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    func requirePresence(of keys: CodingKeys...) throws {
      for key in keys where !container.contains(key) {
        throw DecodingError.keyNotFound(
          key,
          DecodingError.Context(
            codingPath: decoder.codingPath,
            debugDescription: "Required nullable field '\(key.stringValue)' is missing"))
      }
    }

    let type = try container.decode(String.self, forKey: .type)
    switch type {
    case "hello":
      let protocolVersion = try container.decode(String.self, forKey: .protocolVersion)
      let coreVersion = try container.decode(String.self, forKey: .coreVersion)
      self = .hello(protocolVersion: protocolVersion, coreVersion: coreVersion)
    case "status_update":
      // Unknown state values MUST decode as .idle (protocol §Daemon
      // v1.4.0) — a hard decode failure here would drop the whole
      // status_update and freeze the menu on stale state.
      let stateRaw = try container.decode(String.self, forKey: .state)
      let state = StatusInfo.State(rawValue: stateRaw) ?? .idle
      let configured = try container.decode(Bool.self, forKey: .configured)
      let ipodConnected = try container.decode(Bool.self, forKey: .ipodConnected)
      let lastSync = try container.decodeIfPresent(HistoryEntry.self, forKey: .lastSync)
      let nextScheduledUnixSecs = try container.decodeIfPresent(
        UInt64.self, forKey: .nextScheduledUnixSecs)
      let storage = try container.decodeIfPresent(StatusInfo.Storage.self, forKey: .storage)
      let syncedCount = try container.decode(Int.self, forKey: .syncedCount)
      let libraryCount = try container.decodeIfPresent(Int.self, forKey: .libraryCount)
      let requestID = try container.decodeIfPresent(String.self, forKey: .acknowledgedRequestID)
      self = .statusUpdate(
        StatusInfo(
          state: state,
          configured: configured,
          ipodConnected: ipodConnected,
          lastSync: lastSync,
          nextScheduledUnixSecs: nextScheduledUnixSecs,
          storage: storage,
          syncedCount: syncedCount,
          libraryCount: libraryCount,
          acknowledgedRequestID: requestID))
    case "config_update":
      try requirePresence(of: .source, .daemon, .ipod)
      let source = try container.decodeIfPresent(String.self, forKey: .source)
      let daemon = try container.decodeIfPresent(DaemonSettings.self, forKey: .daemon)
      let ipod = try container.decodeIfPresent(IpodIdentity.self, forKey: .ipod)
      let revision = try container.decode(UInt64.self, forKey: .configRevision)
      let requestID = try container.decodeIfPresent(String.self, forKey: .acknowledgedRequestID)
      self = .configUpdate(
        source: source,
        daemon: daemon,
        ipod: ipod,
        configRevision: revision,
        acknowledgedRequestID: requestID)
    case "history_update":
      let entries = try container.decode([HistoryEntry].self, forKey: .entries)
      self = .historyUpdate(
        entries: entries,
        acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID))
    case "device_connected":
      let serial = try container.decode(String.self, forKey: .serial)
      let modelLabel = try container.decode(String.self, forKey: .modelLabel)
      let drive = try container.decode(String.self, forKey: .drive)
      let name = try container.decodeIfPresent(String.self, forKey: .name)
      self = .deviceConnected(serial: serial, modelLabel: modelLabel, drive: drive, name: name)
    case "device_disconnected":
      let serial = try container.decode(String.self, forKey: .serial)
      self = .deviceDisconnected(serial: serial)
    case "sync_rejected":
      let reason = try container.decode(String.self, forKey: .reason)
      self = .syncRejected(
        reason: reason,
        serial: try container.decode(String.self, forKey: .serial),
        acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID))
    case "sync_event":
      let line = try container.decode(String.self, forKey: .line)
      self = .syncEvent(
        line: line,
        serial: try container.decodeIfPresent(String.self, forKey: .serial),
        sessionID: try container.decode(UInt64.self, forKey: .sessionID))
    case "device_inventory_snapshot":
      self = .deviceInventorySnapshot(
        DeviceInventorySnapshot(
          revision: try container.decode(UInt64.self, forKey: .revision),
          devices: try container.decode([DeviceSnapshotWire].self, forKey: .devices)))
    case "library_update":
      try requirePresence(of: .sourceRoot, .scannedAtUnixSecs)
      self = .libraryUpdate(
        LibraryInfo(
          sourceRoot: try container.decodeIfPresent(String.self, forKey: .sourceRoot),
          scannedAtUnixSecs: try container.decodeIfPresent(UInt64.self, forKey: .scannedAtUnixSecs),
          artists: try container.decode([LibraryArtist].self, forKey: .artists),
          genres: try container.decode([LibraryGenre].self, forKey: .genres),
          totalTracks: try container.decode(Int.self, forKey: .totalTracks),
          totalBytes: try container.decode(UInt64.self, forKey: .totalBytes),
          acknowledgedRequestID: try container.decodeIfPresent(
            String.self, forKey: .acknowledgedRequestID)))
    case "source_availability":
      let state = try container.decode(SourceAvailabilityState.self, forKey: .state)
      let sourceRoot: String?
      switch state {
      case .available:
        sourceRoot = try container.decode(String.self, forKey: .sourceRoot)
      case .remounting, .authRequired, .unavailable:
        guard !container.contains(.sourceRoot) else {
          throw DecodingError.dataCorruptedError(
            forKey: .sourceRoot, in: container,
            debugDescription: "source_root must be omitted unless source is available")
        }
        sourceRoot = nil
      }
      self = .sourceAvailability(
        SourceAvailabilityInfo(
          state: state,
          sourceRoot: sourceRoot,
          acknowledgedRequestID: try container.decodeIfPresent(
            String.self, forKey: .acknowledgedRequestID)))
    case "selection_update":
      self = .selectionUpdate(
        mode: try container.decode(SelectionMode.self, forKey: .mode),
        rules: try container.decode([SelectionRule].self, forKey: .rules),
        serial: try container.decodeIfPresent(String.self, forKey: .serial),
        acknowledgedRequestID: try container.decodeIfPresent(
          String.self, forKey: .acknowledgedRequestID))
    case "selection_preview":
      self = .selectionPreview(
        SelectionPreviewInfo(
          selectedTracks: try container.decode(Int.self, forKey: .selectedTracks),
          selectedBytes: try container.decode(UInt64.self, forKey: .selectedBytes),
          adds: try container.decode(Int.self, forKey: .adds),
          removes: try container.decode(Int.self, forKey: .removes),
          serial: try container.decode(String.self, forKey: .serial),
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID)))
    case "playlists_update":
      let playlists = try container.decode([PlaylistSummary].self, forKey: .playlists)
      self = .playlistsUpdate(
        playlists,
        playlistRevision: try container.decode(UInt64.self, forKey: .playlistRevision),
        acknowledgedRequestID: try container.decodeIfPresent(
          String.self, forKey: .acknowledgedRequestID))
    case "playlist_detail":
      let slug = try container.decode(String.self, forKey: .slug)
      let name = try container.decodeIfPresent(String.self, forKey: .name)
      let kind = try container.decodeIfPresent(PlaylistKind.self, forKey: .kind)
      let tracks = try container.decodeIfPresent([String].self, forKey: .tracks)
      let rules = try container.decodeIfPresent(SmartRulesWire.self, forKey: .rules)
      let error = try container.decodeIfPresent(String.self, forKey: .error)
      self = .playlistDetail(
        PlaylistDetail(
          slug: slug,
          name: name,
          kind: kind,
          tracks: tracks,
          rules: rules,
          error: error,
          playlistRevision: try container.decode(UInt64.self, forKey: .playlistRevision),
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID)))
    case "device_config_update":
      let serial = try container.decode(String.self, forKey: .serial)
      let selection = try container.decode(SelectionState.self, forKey: .selection)
      let subscriptions = try container.decode(SubscriptionsWire.self, forKey: .subscriptions)
      let settings = try container.decode(DeviceSettingsWire.self, forKey: .settings)
      self = .deviceConfigUpdate(
        serial: serial,
        selection: selection,
        subscriptions: subscriptions,
        settings: settings,
        selectionRevision: try container.decode(UInt64.self, forKey: .selectionRevision),
        settingsRevision: try container.decode(UInt64.self, forKey: .settingsRevision),
        subscriptionsRevision: try container.decode(UInt64.self, forKey: .subscriptionsRevision),
        acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID))
    case "device_preview":
      try requirePresence(of: .projectedFreeBytes)
      self = .devicePreview(
        DevicePreview(
          serial: try container.decode(String.self, forKey: .serial),
          selectedTracks: try container.decode(Int.self, forKey: .selectedTracks),
          selectedBytes: try container.decode(UInt64.self, forKey: .selectedBytes),
          playlistExtraTracks: try container.decode(Int.self, forKey: .playlistExtraTracks),
          playlistExtraBytes: try container.decode(UInt64.self, forKey: .playlistExtraBytes),
          projectedFreeBytes: try container.decodeIfPresent(
            UInt64.self, forKey: .projectedFreeBytes),
          unresolvedSubscriptions: try container.decodeIfPresent(
            [String].self, forKey: .unresolvedSubscriptions),
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID)))
    case "resolved_tracks":
      let tracks = try container.decode([String].self, forKey: .tracks)
      self = .resolvedTracks(
        tracks: tracks,
        acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID))
    case "command_failed":
      self = .commandFailed(
        acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID),
        error: try container.decode(String.self, forKey: .error))
    case "device_selection_added":
      self = .deviceSelectionAdded(
        DeviceSelectionAddedInfo(
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID),
          serial: try container.decode(String.self, forKey: .serial),
          matchedTracks: try container.decode(Int.self, forKey: .matchedTracks),
          missingTracks: try container.decode(Int.self, forKey: .missingTracks),
          selectionChanged: try container.decode(Bool.self, forKey: .selectionChanged),
          selectionRevision: try container.decode(UInt64.self, forKey: .selectionRevision),
          selection: try container.decode(SelectionState.self, forKey: .selection),
          delivery: try container.decode(DropDelivery.self, forKey: .delivery)))
    case "playlist_selection_appended":
      self = .playlistSelectionAppended(
        PlaylistSelectionAppendedInfo(
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID),
          slug: try container.decode(String.self, forKey: .slug),
          appendedTracks: try container.decode(Int.self, forKey: .appendedTracks),
          playlistRevision: try container.decode(UInt64.self, forKey: .playlistRevision),
          playlist: try container.decode(ManualPlaylistWire.self, forKey: .playlist)))
    case "library_mutation_rejected":
      self = .libraryMutationRejected(
        LibraryMutationRejectedInfo(
          acknowledgedRequestID: try container.decode(String.self, forKey: .acknowledgedRequestID),
          target: try container.decode(LibraryMutationTarget.self, forKey: .target),
          code: try container.decode(String.self, forKey: .code),
          message: try container.decode(String.self, forKey: .message)))
    default:
      self = .unknown
    }
  }
}

struct DurableAcknowledgement: Equatable, Sendable {
  var requestID: String
  var revision: UInt64?
  var configState: ConfigCommitState?
  var target: DurableIntentKey? = nil
  var terminalFailure = false
}

struct ConfigCommitState: Equatable, Sendable {
  var source: String?
  var daemon: DaemonSettings?
  var ipod: IpodIdentity?
}

extension DaemonEvent {
  var durableAcknowledgement: DurableAcknowledgement? {
    switch self {
    case .configUpdate(let source, let daemon, let ipod, let revision, let requestID):
      requestID.map {
        DurableAcknowledgement(
          requestID: $0,
          revision: revision,
          configState: ConfigCommitState(source: source, daemon: daemon, ipod: ipod))
      }
    case .selectionUpdate(_, _, _, let requestID):
      requestID.map {
        DurableAcknowledgement(requestID: $0, revision: nil, configState: nil)
      }
    case .playlistsUpdate(_, let revision, let requestID):
      requestID.map {
        DurableAcknowledgement(requestID: $0, revision: revision, configState: nil)
      }
    case .deviceConfigUpdate(
      _, _, _, _, let selectionRevision, let settingsRevision, let subscriptionsRevision,
      let requestID):
      DurableAcknowledgement(
        requestID: requestID,
        revision: max(selectionRevision, max(settingsRevision, subscriptionsRevision)),
        configState: nil)
    case .deviceSelectionAdded(let reply):
      DurableAcknowledgement(
        requestID: reply.acknowledgedRequestID, revision: reply.selectionRevision,
        configState: nil, target: .deviceSelectionAddition(serial: reply.serial))
    case .playlistSelectionAppended(let reply):
      DurableAcknowledgement(
        requestID: reply.acknowledgedRequestID, revision: reply.playlistRevision,
        configState: nil, target: .playlistAppend(slug: reply.slug))
    case .libraryMutationRejected(let reply):
      DurableAcknowledgement(
        requestID: reply.acknowledgedRequestID, revision: nil, configState: nil,
        target: reply.target.durableIntentKey, terminalFailure: true)
    default:
      nil
    }
  }
}

extension LibraryMutationTarget {
  fileprivate var durableIntentKey: DurableIntentKey {
    switch self {
    case .deviceSelection(let serial): .deviceSelectionAddition(serial: serial)
    case .manualPlaylist(let slug): .playlistAppend(slug: slug)
    }
  }
}
