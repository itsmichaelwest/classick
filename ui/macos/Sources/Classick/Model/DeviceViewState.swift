import Foundation

typealias StorageWire = StatusInfo.Storage
typealias HistoryEntryWire = HistoryEntry

enum DevicePhase: Equatable, Sendable {
  case disconnected
  case unconfigured
  case idle
  case syncing
  case paused
  case error(String)
}

struct DeviceSyncProgress: Equatable, Sendable {
  var current: Int
  var total: Int
  var label: String
  var etaSecs: UInt64?
}

struct DeviceRunRollup: Equatable, Sendable {
  var success: Bool
  var skippedForSpace: SkippedForSpace?
  var artwork: ArtworkSummary?
  var dbRestored: Bool
}

struct DeviceFinalization: Equatable, Sendable {
  var reason: SyncStopReason
  var stagedAlbums: Int
  var stagedTracks: Int
}

struct DeviceConfigDeliveryState: Equatable, Sendable {
  var selection: WireV3ConfigComponent<SelectionState>
  var settings: WireV3ConfigComponent<DeviceSettingsWire>
  var subscriptions: WireV3ConfigComponent<SubscriptionsWire>
}

struct UnidentifiedDeviceViewState: Equatable, Sendable {
  var observationID: ObservationID
  var readiness: String
  var hardware: WireV3Hardware
}

struct DeviceViewState: Equatable, Sendable {
  var deviceID: DeviceID
  var identity: DeviceIdentityWire
  var readiness: String = "ready"
  var hardware: WireV3Hardware = .init(
    family: nil, generation: nil, modelCode: nil, colour: nil, firmware: nil,
    capacityBytes: nil)
  var profileStatus: String = "not_adopted"
  var configDelivery: DeviceConfigDeliveryState?
  var configured: Bool
  var connected: Bool
  var mountPath: String?
  var phase: DevicePhase
  var sessionID: UInt64?
  var storage: StorageWire?
  var syncedCount: Int
  var libraryCount: Int?
  var latestSuccessfulSync: HistoryEntryWire?
  var latestAttempt: HistoryEntryWire?
  var lastTerminalError: String?
  var config: DeviceConfigState?
  var preview: DevicePreview?
  var selectionRevision: UInt64
  var settingsRevision: UInt64
  var subscriptionsRevision: UInt64
  var syncProgress: DeviceSyncProgress?
  var finalization: DeviceFinalization?
  var lastRun: DeviceRunRollup?
}
