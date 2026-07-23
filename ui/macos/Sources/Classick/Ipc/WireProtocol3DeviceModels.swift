import Foundation

struct ObservationID: Codable, Hashable, Comparable, Sendable {
  let rawValue: UInt64

  init(_ rawValue: UInt64) throws {
    guard rawValue > 0 else { throw WireV3Error.invalid("observation_id must be positive") }
    self.rawValue = rawValue
  }

  init(from decoder: Decoder) throws {
    try self.init(decoder.singleValueContainer().decode(UInt64.self))
  }

  func encode(to encoder: Encoder) throws {
    var container = encoder.singleValueContainer()
    try container.encode(rawValue)
  }

  static func < (lhs: ObservationID, rhs: ObservationID) -> Bool {
    lhs.rawValue < rhs.rawValue
  }
}

struct WireV3HardwareFact<Value: Codable & Equatable & Sendable>: Codable, Equatable, Sendable {
  let value: Value
  let source: String
  let confidence: String
}

struct WireV3Hardware: Codable, Equatable, Sendable {
  let family: WireV3HardwareFact<String>?
  let generation: WireV3HardwareFact<String>?
  let modelCode: WireV3HardwareFact<String>?
  let colour: WireV3HardwareFact<String>?
  let firmware: WireV3HardwareFact<String>?
  let capacityBytes: WireV3HardwareFact<UInt64>?

  enum CodingKeys: String, CodingKey {
    case family, generation
    case modelCode = "model_code"
    case colour, firmware
    case capacityBytes = "capacity_bytes"
  }
}

struct WireV3Storage: Codable, Equatable, Sendable {
  let totalBytes: UInt64
  let freeBytes: UInt64
  let freshness: String

  enum CodingKeys: String, CodingKey {
    case totalBytes = "total_bytes"
    case freeBytes = "free_bytes"
    case freshness
  }
}

struct WireV3IdentifiedDevice: Codable, Equatable, Sendable {
  let deviceID: DeviceID
  let name: String?
  let readiness: String
  let hardware: WireV3Hardware
  let profileStatus: String
  let connected: Bool
  let mountPath: String?
  let phase: String
  let sessionID: UInt64?
  let storage: WireV3Storage?
  let syncedCount: Int
  let libraryCount: Int?
  let lastTerminalError: String?

  enum CodingKeys: String, CodingKey {
    case deviceID = "device_id"
    case name, readiness, hardware
    case profileStatus = "profile_status"
    case connected
    case mountPath = "mount_path"
    case phase
    case sessionID = "session_id"
    case storage
    case syncedCount = "synced_count"
    case libraryCount = "library_count"
    case lastTerminalError = "last_terminal_error"
  }
}

struct WireV3UnidentifiedDevice: Codable, Equatable, Sendable {
  let observationID: ObservationID
  let readiness: String
  let hardware: WireV3Hardware

  enum CodingKeys: String, CodingKey {
    case observationID = "observation_id"
    case readiness, hardware
  }
}

struct WireV3DeviceInventory: Codable, Equatable, Sendable {
  let requestID: UUID?
  let revision: UInt64
  let devices: [WireV3IdentifiedDevice]
  let unidentified: [WireV3UnidentifiedDevice]

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case revision, devices, unidentified
  }
}

struct WireV3Delivery: Codable, Equatable, Sendable {
  let state: String
  let lastFailure: String?

  enum CodingKeys: String, CodingKey {
    case state
    case lastFailure = "last_failure"
  }
}

struct WireV3ConfigComponent<Value: Codable & Equatable & Sendable>: Codable, Equatable,
  Sendable
{
  let revision: UInt64
  let mutationID: UUID
  let value: Value
  let delivery: WireV3Delivery

  enum CodingKeys: String, CodingKey {
    case revision
    case mutationID = "mutation_id"
    case value, delivery
  }
}

struct WireV3DeviceConfig: Codable, Equatable, Sendable {
  let requestID: UUID?
  let deviceID: DeviceID
  let selection: WireV3ConfigComponent<SelectionState>
  let settings: WireV3ConfigComponent<DeviceSettingsWire>
  let subscriptions: WireV3ConfigComponent<SubscriptionsWire>

  enum CodingKeys: String, CodingKey {
    case requestID = "request_id"
    case deviceID = "device_id"
    case selection, settings, subscriptions
  }
}
