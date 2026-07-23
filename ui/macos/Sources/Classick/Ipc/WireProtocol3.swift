import Foundation

enum WireV3Error: Error, Equatable, CustomStringConvertible {
  case invalid(String)

  var description: String {
    switch self {
    case .invalid(let message): message
    }
  }
}

struct DeviceID: Codable, Hashable, Comparable, Sendable, CustomStringConvertible,
  ExpressibleByStringLiteral
{
  let rawValue: String

  init(_ rawValue: String) throws {
    guard rawValue.count == 16,
      rawValue.unicodeScalars.allSatisfy({
        (48...57).contains($0.value) || (65...70).contains($0.value)
      })
    else { throw WireV3Error.invalid("device_id must be 16 canonical uppercase hex characters") }
    self.rawValue = rawValue
  }

  init(stringLiteral value: String) {
    let hexadecimal =
      value.hasPrefix("0x") || value.hasPrefix("0X") ? String(value.dropFirst(2)) : value
    let canonical =
      hexadecimal.count < 16
      ? String(repeating: "0", count: 16 - hexadecimal.count) + hexadecimal.uppercased()
      : hexadecimal.uppercased()
    precondition(
      canonical.count == 16
        && canonical.unicodeScalars.allSatisfy({
          (48...57).contains($0.value) || (65...70).contains($0.value)
        }), "invalid DeviceID literal")
    rawValue = canonical
  }

  init(from decoder: Decoder) throws {
    try self.init(decoder.singleValueContainer().decode(String.self))
  }

  func encode(to encoder: Encoder) throws {
    var container = encoder.singleValueContainer()
    try container.encode(rawValue)
  }

  var description: String { rawValue }

  static func < (lhs: DeviceID, rhs: DeviceID) -> Bool {
    lhs.rawValue < rhs.rawValue
  }

}

struct WireV3Route: Equatable, Sendable {
  let deviceID: DeviceID
  let sessionID: UInt64
}

enum WireV3LibraryScanEventKind: String, Sendable {
  case started = "library_scan_started"
  case progress = "library_scan_progress"
  case finished = "library_scan_finished"
}

struct WireV3LibraryScanEvent: Equatable, Sendable {
  let kind: WireV3LibraryScanEventKind
  let requestID: UUID?
  let sessionID: UInt64
  let filesScanned: UInt64?
  let tracksIndexed: UInt64?
  let success: Bool?
  let message: String?
}

enum WireV3EndpointRole: String, Codable, Sendable {
  case desktop, daemon, worker
}

struct WireV3Hello: Codable, Equatable, Sendable {
  let protocolVersion: String
  let role: WireV3EndpointRole
  let softwareVersion: String
  let capabilities: [String]

  enum CodingKeys: String, CodingKey {
    case protocolVersion = "protocol_version"
    case role
    case softwareVersion = "software_version"
    case capabilities
  }
}

enum WireV3Direction: Sendable {
  case daemonToDesktopEvents
  case desktopToDaemonCommands
  case workerToDaemonEvents(expected: WireV3Route)
  case daemonToWorkerCommands(expected: WireV3Route, pending: WireV3PendingInteraction)
}

enum WireV3PendingInteraction: Equatable, Sendable {
  case none
  case review
  case prompt(id: UInt64, optionCount: UInt64)
  case form(id: UInt64)
}

enum WireV3ProgressEventKind: String, Sendable {
  case runHeader = "run_header"
  case syncSummary = "sync_summary"
  case reviewRequested = "review_requested"
  case prompt, form
  case trackStart = "track_start"
  case trackDone = "track_done"
  case finalizing
  case syncCancelled = "sync_cancelled"
  case syncPaused = "sync_paused"
  case syncLog = "sync_log"
  case syncError = "sync_error"
  case syncFinished = "sync_finished"
}

struct WireV3ProgressEvent: Equatable, Sendable {
  let kind: WireV3ProgressEventKind
  let route: WireV3Route
  let promptID: UInt64?
  let source: String?
  let ipod: String?
  let manifest: String?
  let summary: WireV3ActionPlanSummary?
  let noDelete: Bool?
  let message: String?
  let options: [String]?
  let initial: String?
  let hint: String?
  let current: Int?
  let total: Int?
  let label: String?
  let etaSecs: UInt64?
  let result: String?
  let finalizationReason: SyncStopReason?
  let stagedAlbums: Int?
  let stagedTracks: Int?
  let success: Bool?
  let skippedForSpace: SkippedForSpace?
  let artwork: ArtworkSummary?
  let dbRestored: Bool?
  let recoveryHints: [String]?
}

struct WireV3ActionPlanSummary: Codable, Equatable, Sendable {
  let add: UInt64
  let modify: UInt64
  let metadataOnly: UInt64
  let remove: UInt64
  let unchanged: UInt64
  let totalPlanned: UInt64

  enum CodingKeys: String, CodingKey {
    case add, modify
    case metadataOnly = "metadata_only"
    case remove, unchanged
    case totalPlanned = "total_planned"
  }
}

enum WireV3ProgressCommandKind: String, Sendable {
  case applyReview = "apply_review"
  case dryRunReview = "dry_run_review"
  case quitReview = "quit_review"
  case promptDecision = "prompt_decision"
  case formDecision = "form_decision"
  case cancelSync = "cancel_sync"
  case pauseSync = "pause_sync"
}

struct WireV3ProgressCommand: Equatable, Sendable {
  let kind: WireV3ProgressCommandKind
  let route: WireV3Route
  let requestID: UUID
  let promptID: UInt64?
}

enum WireV3Event: Equatable, Sendable {
  case hello(WireV3Hello)
  case globalConfig(WireV3GlobalConfigEvent)
  case sourceAvailability(WireV3SourceAvailabilityEvent)
  case deviceInventory(WireV3DeviceInventory)
  case inventorySubscriptionChanged(WireV3InventorySubscriptionEvent)
  case deviceConfig(WireV3DeviceConfig)
  case configMutationFailed(WireV3ConfigMutationFailedEvent)
  case deviceForgotten(WireV3DeviceForgottenEvent)
  case syncAccepted(WireV3SyncAcceptedEvent)
  case syncRejected(WireV3SyncRejectedEvent)
  case history(WireV3HistoryEvent)
  case library(WireV3LibraryEvent)
  case libraryScan(WireV3LibraryScanEvent)
  case selectionPreview(WireV3SelectionPreviewEvent)
  case devicePreview(WireV3DevicePreviewEvent)
  case resolvedTracks(WireV3ResolvedTracksEvent)
  case playlists(WireV3PlaylistsEvent)
  case playlistDetail(WireV3PlaylistDetailEvent)
  case playlistSaved(WireV3PlaylistSavedEvent)
  case deviceSelectionAdded(WireV3DeviceSelectionAddedEvent)
  case playlistSelectionAppended(WireV3PlaylistSelectionAppendedEvent)
  case libraryMutationRejected(WireV3LibraryMutationRejectedEvent)
  case daemonShutdownStarted(WireV3RequestEvent)
  case progress(WireV3ProgressEvent)
  case commandFailed(WireV3CommandFailedEvent)
}

enum WireV3DecodedCommand: Equatable, Sendable {
  case progress(WireV3ProgressCommand)
  case known(type: String)
}

enum WireV3DecodedMessage: Equatable, Sendable {
  case event(WireV3Event)
  case command(WireV3DecodedCommand)
  case ignoredUnknownEvent(type: String)
}

enum WireV3ConnectionCompatibility: Equatable, Sendable {
  case compatible(WireV3Hello)
  case incompatible(String)
}
