import Foundation

enum WireV3SemanticValidator {
  static func validateInventory(_ object: [String: Any]) throws {
    _ = try positiveUInt(object, "revision")
    guard let devices = object["devices"] as? [[String: Any]],
      let unidentified = object["unidentified"] as? [[String: Any]]
    else { throw WireV3Error.invalid("inventory arrays are required") }
    var ids = Set<DeviceID>()
    var mounts = Set<String>()
    var observations = Set<UInt64>()
    for device in devices {
      let id = try deviceID(device)
      guard ids.insert(id).inserted else { throw WireV3Error.invalid("duplicate device") }
      let connected = device["connected"] as? Bool == true
      let mount = string(device, "mount_path")
      guard connected == (mount != nil) else {
        throw WireV3Error.invalid("mount does not match connection")
      }
      if let mount {
        guard mount.hasPrefix("/") else { throw WireV3Error.invalid("mount must be absolute") }
        guard mounts.insert(mount).inserted else { throw WireV3Error.invalid("duplicate mount") }
      }
      let phase = string(device, "phase")
      guard connected || phase == "disconnected" else {
        throw WireV3Error.invalid("disconnected device phase")
      }
      guard !connected || phase != "disconnected" else {
        throw WireV3Error.invalid("connected device phase")
      }
      guard string(device, "readiness") != "identity_unavailable" else {
        throw WireV3Error.invalid("identified device cannot be identity unavailable")
      }
      if phase == "syncing" || phase == "paused" {
        guard string(device, "readiness") == "ready" else {
          throw WireV3Error.invalid("unready device cannot have active phase")
        }
      }
      if phase == "syncing" {
        _ = try positiveUInt(device, "session_id")
        guard string(device, "profile_status") == "adopted" else {
          throw WireV3Error.invalid("syncing device must be adopted")
        }
      } else if device["session_id"] != nil {
        throw WireV3Error.invalid("session_id is only valid while syncing")
      }
      if let storage = device["storage"] as? [String: Any] {
        let total = try positiveUInt(storage, "total_bytes")
        guard let free = uint(storage, "free_bytes"), free <= total else {
          throw WireV3Error.invalid("invalid storage")
        }
        if !connected && string(storage, "freshness") != "cached" {
          throw WireV3Error.invalid("disconnected storage must be cached")
        }
      }
      try validateHardware(device["hardware"] as? [String: Any] ?? [:])
    }
    for candidate in unidentified {
      let observation = try positiveUInt(candidate, "observation_id")
      guard observations.insert(observation).inserted else {
        throw WireV3Error.invalid("duplicate observation")
      }
      guard string(candidate, "readiness") == "identity_unavailable" else {
        throw WireV3Error.invalid("unidentified readiness")
      }
      try validateHardware(candidate["hardware"] as? [String: Any] ?? [:])
    }
  }

  static func validateDeviceConfig(_ object: [String: Any]) throws {
    _ = try deviceID(object)
    var mutations = Set<String>()
    for key in ["selection", "settings", "subscriptions"] {
      guard let component = object[key] as? [String: Any],
        let mutation = string(component, "mutation_id"), !mutation.isEmpty,
        let delivery = component["delivery"] as? [String: Any]
      else { throw WireV3Error.invalid("missing config component") }
      guard mutations.insert(mutation).inserted else {
        throw WireV3Error.invalid("duplicate mutation")
      }
      if let failure = delivery["last_failure"] as? String, failure.isEmpty {
        throw WireV3Error.invalid("empty delivery failure")
      }
    }
  }

  static func validateHistory(_ object: [String: Any]) throws {
    guard let entries = object["entries"] as? [[String: Any]] else {
      throw WireV3Error.invalid("history entries are required")
    }
    for entry in entries where string(entry, "outcome") == "ok" && entry["error_message"] != nil {
      throw WireV3Error.invalid("successful history cannot have an error")
    }
  }

  static func validateLibrary(_ object: [String: Any]) throws {
    if object["source_root"] == nil {
      let tracks = uint(object, "total_tracks") ?? 0
      let bytes = uint(object, "total_bytes") ?? 0
      guard tracks == 0, bytes == 0, object["scanned_at_unix_secs"] == nil else {
        throw WireV3Error.invalid("unconfigured library cannot contain content")
      }
    }
  }

  static func validatePlaylistDetail(_ object: [String: Any]) throws {
    guard let result = object["result"] as? [String: Any] else { return }
    if string(result, "state") == "found", let playlist = result["playlist"] as? [String: Any] {
      guard string(object, "slug") == string(playlist, "slug") else {
        throw WireV3Error.invalid("playlist slug mismatch")
      }
    }
  }

  static func validatePlaylistLimit(_ object: [String: Any]) throws {
    guard let playlist = object["playlist"] as? [String: Any],
      let rules = playlist["rules"] as? [String: Any],
      let limit = rules["limit"] as? [String: Any]
    else { return }
    if let tracks = uint(limit, "tracks"), tracks == 0 { throw WireV3Error.invalid("zero limit") }
    if let bytes = uint(limit, "bytes"), bytes == 0 { throw WireV3Error.invalid("zero limit") }
  }

  static func validateSortedStrings(_ object: [String: Any], key: String) throws {
    guard let values = object[key] as? [String], values == values.sorted() else {
      throw WireV3Error.invalid("\(key) must be sorted")
    }
  }

  static func validateUniqueStrings(_ object: [String: Any], at path: [String]) throws {
    var current: Any = object
    for key in path {
      guard let dictionary = current as? [String: Any], let next = dictionary[key] else {
        throw WireV3Error.invalid("missing \(path.joined(separator: "."))")
      }
      current = next
    }
    guard let values = current as? [String], Set(values).count == values.count else {
      throw WireV3Error.invalid("values must be unique")
    }
  }

  static func validateSourceRoot(_ value: String) throws {
    guard !value.contains("://") || URL(string: value)?.user == nil else {
      throw WireV3Error.invalid("source root must not include credentials")
    }
    guard
      value.hasPrefix("/") || value.hasPrefix("\\\\")
        || value.range(of: #"^[A-Za-z]:[\\/]"#, options: .regularExpression) != nil
    else { throw WireV3Error.invalid("source root must be absolute") }
  }

  private static func validateHardware(_ hardware: [String: Any]) throws {
    for (name, value) in hardware {
      guard let fact = value as? [String: Any], let source = string(fact, "source"),
        let confidence = string(fact, "confidence")
      else { throw WireV3Error.invalid("invalid hardware fact") }
      if let text = fact["value"] as? String, text.isEmpty {
        throw WireV3Error.invalid("empty hardware fact")
      }
      if name == "capacity_bytes", uint(fact, "value") == 0 {
        throw WireV3Error.invalid("zero capacity")
      }
      if source == "inferred" && confidence == "certain" {
        throw WireV3Error.invalid("inferred fact cannot be certain")
      }
    }
  }

  private static func deviceID(_ object: [String: Any]) throws -> DeviceID {
    guard let raw = string(object, "device_id") else {
      throw WireV3Error.invalid("device_id is required")
    }
    return try DeviceID(raw)
  }

  private static func positiveUInt(_ object: [String: Any], _ key: String) throws -> UInt64 {
    guard let value = uint(object, key), value > 0 else {
      throw WireV3Error.invalid("\(key) must be positive")
    }
    return value
  }

  private static func uint(_ object: [String: Any], _ key: String) -> UInt64? {
    guard let number = object[key] as? NSNumber,
      CFGetTypeID(number) != CFBooleanGetTypeID(), number.int64Value >= 0,
      number.doubleValue == Double(number.uint64Value)
    else { return nil }
    return number.uint64Value
  }

  private static func string(_ object: [String: Any], _ key: String) -> String? {
    object[key] as? String
  }
}
