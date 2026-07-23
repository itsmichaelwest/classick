import Foundation

enum WireV3Codec {
  static let protocolMajor = 3
  static let daemonCapabilities = ["device_inventory", "portable_profile", "typed_sync_progress"]

  private static let commandTypes: Set<String> = [
    "get_global_config", "set_source_location", "set_global_settings", "get_inventory",
    "subscribe_inventory", "unsubscribe_inventory", "adopt_device", "forget_device",
    "get_device_config", "set_selection", "set_settings", "set_subscriptions", "trigger_sync",
    "backfill_rockbox", "replace_library", "get_history", "get_library", "scan_library",
    "retry_source_mount", "preview_selection", "preview_device", "resolve_tracks",
    "add_selection_to_device", "list_playlists", "get_playlist", "save_playlist",
    "delete_playlist", "append_selection_to_playlist", "shutdown", "apply_review",
    "dry_run_review", "quit_review", "prompt_decision", "form_decision", "cancel_sync",
    "pause_sync",
  ]

  private static let eventTypes: Set<String> = [
    "global_config", "source_availability", "device_inventory",
    "inventory_subscription_changed", "device_config", "config_mutation_failed",
    "device_forgotten", "sync_accepted", "sync_rejected", "history", "library",
    "library_scan_started", "library_scan_progress", "library_scan_finished",
    "selection_preview", "device_preview", "resolved_tracks", "playlists", "playlist_detail",
    "playlist_saved", "device_selection_added", "playlist_selection_appended",
    "library_mutation_rejected", "daemon_shutdown_started", "run_header", "sync_summary",
    "review_requested", "prompt", "form", "track_start", "track_done", "finalizing",
    "sync_cancelled", "sync_paused", "sync_log", "sync_error", "sync_finished",
    "command_failed",
  ]

  static func decodeInitialHello(_ data: Data) throws -> WireV3Hello {
    let object = try jsonObject(data)
    guard string(object, "type") == "hello" else {
      throw WireV3Error.invalid("first wire message must be hello")
    }
    let hello = try JSONDecoder().decode(WireV3HelloEnvelope.self, from: data).hello
    try validateSemanticVersion(hello.protocolVersion, field: "protocol_version")
    try validateSemanticVersion(hello.softwareVersion, field: "software_version")
    guard !hello.capabilities.contains(where: { $0.isEmpty }),
      Set(hello.capabilities).count == hello.capabilities.count
    else { throw WireV3Error.invalid("capabilities must be unique and non-empty") }
    return WireV3Hello(
      protocolVersion: hello.protocolVersion, role: hello.role,
      softwareVersion: hello.softwareVersion, capabilities: hello.capabilities.sorted())
  }

  static func admitDaemonHello(_ data: Data) -> WireV3ConnectionCompatibility {
    do {
      let hello = try decodeInitialHello(data)
      guard semanticMajor(hello.protocolVersion) == protocolMajor else {
        return .incompatible("daemon protocol major is not 3")
      }
      guard hello.role == .daemon else { return .incompatible("peer role is not daemon") }
      let missing = daemonCapabilities.filter { !hello.capabilities.contains($0) }
      guard missing.isEmpty else {
        return .incompatible("daemon is missing capabilities: \(missing.joined(separator: ", "))")
      }
      return .compatible(hello)
    } catch {
      return .incompatible(String(describing: error))
    }
  }

  static func decode(_ data: Data, direction: WireV3Direction) throws -> WireV3DecodedMessage {
    let object = try jsonObject(data)
    guard let type = string(object, "type"), !type.isEmpty else {
      throw WireV3Error.invalid("wire message requires a non-empty string type")
    }
    guard type != "hello" else { throw WireV3Error.invalid("hello is only valid first") }

    switch direction {
    case .daemonToDesktopEvents:
      if commandTypes.contains(type) { throw WireV3Error.invalid("command on event stream") }
      guard eventTypes.contains(type) else { return .ignoredUnknownEvent(type: type) }
      try validate(object, type: type)
      let decoder = JSONDecoder()
      switch type {
      case "global_config": return .event(.globalConfig(try decoder.decode(WireV3GlobalConfigEvent.self, from: data)))
      case "source_availability": return .event(.sourceAvailability(try decoder.decode(WireV3SourceAvailabilityEvent.self, from: data)))
      case "device_inventory": return .event(.deviceInventory(try decoder.decode(WireV3DeviceInventory.self, from: data)))
      case "inventory_subscription_changed": return .event(.inventorySubscriptionChanged(try decoder.decode(WireV3InventorySubscriptionEvent.self, from: data)))
      case "device_config": return .event(.deviceConfig(try decoder.decode(WireV3DeviceConfig.self, from: data)))
      case "config_mutation_failed": return .event(.configMutationFailed(try decoder.decode(WireV3ConfigMutationFailedEvent.self, from: data)))
      case "device_forgotten": return .event(.deviceForgotten(try decoder.decode(WireV3DeviceForgottenEvent.self, from: data)))
      case "sync_accepted": return .event(.syncAccepted(try decoder.decode(WireV3SyncAcceptedEvent.self, from: data)))
      case "sync_rejected": return .event(.syncRejected(try decoder.decode(WireV3SyncRejectedEvent.self, from: data)))
      case "history": return .event(.history(try decoder.decode(WireV3HistoryEvent.self, from: data)))
      case "library": return .event(.library(try decoder.decode(WireV3LibraryEvent.self, from: data)))
      case "selection_preview": return .event(.selectionPreview(try decoder.decode(WireV3SelectionPreviewEvent.self, from: data)))
      case "device_preview": return .event(.devicePreview(try decoder.decode(WireV3DevicePreviewEvent.self, from: data)))
      case "resolved_tracks": return .event(.resolvedTracks(try decoder.decode(WireV3ResolvedTracksEvent.self, from: data)))
      case "playlists": return .event(.playlists(try decoder.decode(WireV3PlaylistsEvent.self, from: data)))
      case "playlist_detail": return .event(.playlistDetail(try decoder.decode(WireV3PlaylistDetailEvent.self, from: data)))
      case "playlist_saved": return .event(.playlistSaved(try decoder.decode(WireV3PlaylistSavedEvent.self, from: data)))
      case "device_selection_added": return .event(.deviceSelectionAdded(try decoder.decode(WireV3DeviceSelectionAddedEvent.self, from: data)))
      case "playlist_selection_appended": return .event(.playlistSelectionAppended(try decoder.decode(WireV3PlaylistSelectionAppendedEvent.self, from: data)))
      case "library_mutation_rejected": return .event(.libraryMutationRejected(try decoder.decode(WireV3LibraryMutationRejectedEvent.self, from: data)))
      case "daemon_shutdown_started": return .event(.daemonShutdownStarted(try decoder.decode(WireV3RequestEvent.self, from: data)))
      case "command_failed": return .event(.commandFailed(try decoder.decode(WireV3CommandFailedEvent.self, from: data)))
      default: break
      }
      if let kind = WireV3LibraryScanEventKind(rawValue: type) {
        return .event(.libraryScan(try libraryScanEvent(object, kind: kind)))
      }
      if let kind = WireV3ProgressEventKind(rawValue: type) {
        return .event(.progress(try progressEvent(object, kind: kind)))
      }
      throw WireV3Error.invalid("known event lacks a typed decoder")
    case .desktopToDaemonCommands:
      guard commandTypes.contains(type) else {
        throw WireV3Error.invalid(
          eventTypes.contains(type) ? "event on command stream" : "unknown command")
      }
      guard object["observation_id"] == nil else {
        throw WireV3Error.invalid("observation_id is forbidden on commands")
      }
      try validate(object, type: type)
      if let kind = WireV3ProgressCommandKind(rawValue: type) {
        return .command(.progress(try progressCommand(object, kind: kind)))
      }
      return .command(.known(type: type))
    case .workerToDaemonEvents(let expected):
      guard eventTypes.contains(type), let kind = WireV3ProgressEventKind(rawValue: type),
        type != "command_failed"
      else { throw WireV3Error.invalid("message is not valid worker output") }
      try validate(object, type: type)
      let event = try progressEvent(object, kind: kind)
      guard event.route == expected else { throw WireV3Error.invalid("worker route mismatch") }
      return .event(.progress(event))
    case .daemonToWorkerCommands(let expected, let pending):
      guard commandTypes.contains(type), let kind = WireV3ProgressCommandKind(rawValue: type) else {
        throw WireV3Error.invalid("message is not a worker command")
      }
      try validate(object, type: type)
      let command = try progressCommand(object, kind: kind)
      guard command.route == expected else { throw WireV3Error.invalid("worker route mismatch") }
      try validate(command, object: object, pending: pending)
      return .command(.progress(command))
    }
  }

  private static func validate(
    _ command: WireV3ProgressCommand, object: [String: Any], pending: WireV3PendingInteraction
  ) throws {
    switch (command.kind, pending) {
    case (.applyReview, .review), (.dryRunReview, .review), (.quitReview, .review),
      (.cancelSync, _), (.pauseSync, _):
      return
    case (.promptDecision, .prompt(let id, let optionCount)):
      guard command.promptID == id, let choice = uint(object, "choice"), choice < optionCount else {
        throw WireV3Error.invalid("prompt decision does not match pending prompt")
      }
    case (.formDecision, .form(let id)):
      guard command.promptID == id else {
        throw WireV3Error.invalid("form decision does not match pending form")
      }
    default:
      throw WireV3Error.invalid("command does not match pending worker interaction")
    }
  }

  private static func progressEvent(
    _ object: [String: Any], kind: WireV3ProgressEventKind
  ) throws -> WireV3ProgressEvent {
    WireV3ProgressEvent(
      kind: kind, route: try route(object),
      promptID: object["prompt_id"] == nil ? nil : try positiveUInt(object, "prompt_id"),
      source: string(object, "source"), ipod: string(object, "ipod"),
      manifest: string(object, "manifest"), summary: decode(object["summary"]),
      noDelete: object["no_delete"] as? Bool, message: string(object, "message"),
      options: object["options"] as? [String], initial: string(object, "initial"),
      hint: string(object, "hint"),
      current: uint(object, "current").flatMap(Int.init(exactly:)),
      total: uint(object, "total").flatMap(Int.init(exactly:)),
      label: string(object, "label"),
      etaSecs: uint(object, "eta_secs"),
      result: string(object, "result"),
      finalizationReason: string(object, "reason").flatMap(SyncStopReason.init(rawValue:)),
      stagedAlbums: uint(object, "staged_albums").flatMap(Int.init(exactly:)),
      stagedTracks: uint(object, "staged_tracks").flatMap(Int.init(exactly:)),
      success: object["success"] as? Bool,
      skippedForSpace: decode(object["skipped_for_space"]),
      artwork: decode(object["artwork"]),
      dbRestored: object["db_restored"] as? Bool,
      recoveryHints: object["recovery_hints"] as? [String])
  }

  private static func libraryScanEvent(
    _ object: [String: Any], kind: WireV3LibraryScanEventKind
  ) throws -> WireV3LibraryScanEvent {
    WireV3LibraryScanEvent(
      kind: kind,
      requestID: object["request_id"] == nil || object["request_id"] is NSNull
        ? nil : try requestID(object),
      sessionID: try positiveUInt(object, "session_id"),
      filesScanned: uint(object, "files_scanned"),
      tracksIndexed: uint(object, "tracks_indexed"),
      success: object["success"] as? Bool,
      message: string(object, "message"))
  }

  private static func decode<Value: Decodable>(_ value: Any?) -> Value? {
    guard let value, JSONSerialization.isValidJSONObject(value),
      let data = try? JSONSerialization.data(withJSONObject: value)
    else { return nil }
    return try? JSONDecoder().decode(Value.self, from: data)
  }

  private static func progressCommand(
    _ object: [String: Any], kind: WireV3ProgressCommandKind
  ) throws -> WireV3ProgressCommand {
    WireV3ProgressCommand(
      kind: kind, route: try route(object), requestID: try requestID(object),
      promptID: object["prompt_id"] == nil ? nil : try positiveUInt(object, "prompt_id"))
  }

  private static func validate(_ object: [String: Any], type: String) throws {
    if object["device_id"] != nil, !(object["device_id"] is NSNull) { _ = try deviceID(object) }
    if object["session_id"] != nil, !(object["session_id"] is NSNull) {
      _ = try positiveUInt(object, "session_id")
    }
    if object["request_id"] != nil, !(object["request_id"] is NSNull) { _ = try requestID(object) }
    if object["prompt_id"] != nil, !(object["prompt_id"] is NSNull) {
      _ = try positiveUInt(object, "prompt_id")
    }

    if WireV3ProgressEventKind(rawValue: type) != nil { _ = try route(object) }
    if WireV3ProgressCommandKind(rawValue: type) != nil {
      _ = try route(object)
      _ = try requestID(object)
    }

    switch type {
    case "track_start":
      let current = try positiveUInt(object, "current")
      let total = try positiveUInt(object, "total")
      guard current <= total else { throw WireV3Error.invalid("track position exceeds total") }
    case "prompt":
      guard let options = object["options"] as? [Any], !options.isEmpty else {
        throw WireV3Error.invalid("prompt options must not be empty")
      }
    case "prompt_decision":
      guard uint(object, "choice") != nil else { throw WireV3Error.invalid("choice is required") }
    case "get_history":
      _ = try positiveUInt(object, "limit")
    case "set_source_location":
      if let root = object["source_root"] as? String {
        try WireV3SemanticValidator.validateSourceRoot(root)
      }
    case "source_availability":
      let available = string(object, "state") == "available"
      guard available == (object["source_root"] != nil) else {
        throw WireV3Error.invalid("source_root presence does not match availability")
      }
    case "library_scan_started":
      _ = try positiveUInt(object, "session_id")
    case "library_scan_progress":
      _ = try positiveUInt(object, "session_id")
      guard let files = uint(object, "files_scanned"),
        let tracks = uint(object, "tracks_indexed"), tracks <= files
      else { throw WireV3Error.invalid("invalid library scan progress") }
    case "library_scan_finished":
      _ = try positiveUInt(object, "session_id")
      guard object["success"] is Bool else {
        throw WireV3Error.invalid("finished scan requires success")
      }
      if object["success"] as? Bool == false {
        guard let message = string(object, "message"), !message.isEmpty else {
          throw WireV3Error.invalid("failed scan requires a message")
        }
      }
    case "history": try WireV3SemanticValidator.validateHistory(object)
    case "library": try WireV3SemanticValidator.validateLibrary(object)
    case "resolved_tracks": try WireV3SemanticValidator.validateSortedStrings(object, key: "tracks")
    case "playlist_detail": try WireV3SemanticValidator.validatePlaylistDetail(object)
    case "save_playlist": try WireV3SemanticValidator.validatePlaylistLimit(object)
    case "append_selection_to_playlist", "add_selection_to_device":
      guard let rules = object["rules"] as? [Any], !rules.isEmpty else {
        throw WireV3Error.invalid("mutation rules must not be empty")
      }
    case "shutdown": _ = try requestID(object)
    case "set_subscriptions":
      try WireV3SemanticValidator.validateUniqueStrings(
        object, at: ["subscriptions", "playlists"])
    case "device_config": try WireV3SemanticValidator.validateDeviceConfig(object)
    case "device_inventory": try WireV3SemanticValidator.validateInventory(object)
    default: break
    }
  }

  private static func route(_ object: [String: Any]) throws -> WireV3Route {
    WireV3Route(deviceID: try deviceID(object), sessionID: try positiveUInt(object, "session_id"))
  }

  private static func deviceID(_ object: [String: Any]) throws -> DeviceID {
    guard let raw = string(object, "device_id") else {
      throw WireV3Error.invalid("device_id is required")
    }
    return try DeviceID(raw)
  }

  private static func requestID(_ object: [String: Any]) throws -> UUID {
    guard let raw = string(object, "request_id"), raw == raw.lowercased(),
      let value = UUID(uuidString: raw), value.uuidString.lowercased() == raw,
      raw != "00000000-0000-0000-0000-000000000000"
    else { throw WireV3Error.invalid("request_id must be a nonnil lowercase UUID") }
    return value
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

  private static func jsonObject(_ data: Data) throws -> [String: Any] {
    guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
      throw WireV3Error.invalid("wire message must be a JSON object")
    }
    return object
  }

  private static func semanticMajor(_ value: String) -> Int? {
    Int(value.split(separator: ".", maxSplits: 1).first ?? "")
  }

  private static func validateSemanticVersion(_ value: String, field: String) throws {
    let pattern =
      #"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"#
    guard value.range(of: pattern, options: .regularExpression) != nil else {
      throw WireV3Error.invalid("\(field) is not valid semantic versioning")
    }
    let publicVersion = value.split(separator: "+", maxSplits: 1)[0]
    if let separator = publicVersion.firstIndex(of: "-") {
      let prerelease = publicVersion[publicVersion.index(after: separator)...]
      let hasLeadingZeroNumericIdentifier = prerelease.split(separator: ".").contains {
        $0.count > 1 && $0.first == "0" && $0.allSatisfy(\.isNumber)
      }
      guard !hasLeadingZeroNumericIdentifier else {
        throw WireV3Error.invalid("\(field) is not valid semantic versioning")
      }
    }
  }
}

private struct WireV3HelloEnvelope: Decodable {
  let hello: WireV3Hello

  init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    guard try container.decode(String.self, forKey: .type) == "hello" else {
      throw WireV3Error.invalid("first wire message must be hello")
    }
    hello = try WireV3Hello(from: decoder)
  }

  private enum CodingKeys: String, CodingKey { case type }
}
