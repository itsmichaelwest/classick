import Foundation

private struct WireV3CanonicalUUID: Encodable {
  let value: UUID

  func encode(to encoder: Encoder) throws {
    var container = encoder.singleValueContainer()
    try container.encode(value.uuidString.lowercased())
  }
}

enum WireV3SyncTrigger: String, Codable, Sendable {
  case manual
  case scheduled
  case plugIn = "plug_in"
  case firstRun = "first_run"
  case drop
}

struct WireV3SelectionValue: Codable, Equatable, Sendable {
  let schemaVersion: UInt32
  let mode: SelectionMode
  let rules: [SelectionRule]

  enum CodingKeys: String, CodingKey {
    case schemaVersion = "schema_version"
    case mode, rules
  }

  init(_ value: SelectionState) {
    schemaVersion = 1
    mode = value.mode
    rules = value.rules
  }

  var value: SelectionState { SelectionState(mode: mode, rules: rules) }
}

struct WireV3SettingsValue: Codable, Equatable, Sendable {
  let schemaVersion: UInt32
  let autoSync: Bool
  let rockboxCompat: Bool

  enum CodingKeys: String, CodingKey {
    case schemaVersion = "schema_version"
    case autoSync = "auto_sync"
    case rockboxCompat = "rockbox_compat"
  }

  init(_ value: DeviceSettingsWire) {
    schemaVersion = 1
    autoSync = value.autoSync
    rockboxCompat = value.rockboxCompat
  }

  var value: DeviceSettingsWire {
    DeviceSettingsWire(autoSync: autoSync, rockboxCompat: rockboxCompat)
  }
}

struct WireV3SubscriptionsValue: Codable, Equatable, Sendable {
  let schemaVersion: UInt32
  let playlists: [String]

  enum CodingKeys: String, CodingKey {
    case schemaVersion = "schema_version"
    case playlists
  }

  init(_ value: SubscriptionsWire) {
    schemaVersion = 1
    playlists = value.playlists
  }

  var value: SubscriptionsWire { SubscriptionsWire(playlists: playlists) }
}

struct WireV3GlobalSettings: Codable, Equatable, Sendable {
  let firstSyncMode: String
  let subsequentSyncMode: String
  let scheduleMinutes: UInt32
  let notifyOn: String
  let dropSyncBehavior: DropSyncBehaviorWire

  enum CodingKeys: String, CodingKey {
    case firstSyncMode = "first_sync_mode"
    case subsequentSyncMode = "subsequent_sync_mode"
    case scheduleMinutes = "schedule_minutes"
    case notifyOn = "notify_on"
    case dropSyncBehavior = "drop_sync_behavior"
  }

  init(_ settings: DaemonSettings) {
    firstSyncMode = settings.firstSyncMode
    subsequentSyncMode = settings.subsequentSyncMode
    scheduleMinutes = settings.scheduleMinutes
    notifyOn = settings.notifyOn
    dropSyncBehavior = settings.dropSyncBehavior
  }

  var appValue: DaemonSettings {
    DaemonSettings(
      enabled: true, autostartWithWindows: false,
      firstSyncMode: firstSyncMode, subsequentSyncMode: subsequentSyncMode,
      scheduleMinutes: scheduleMinutes, notifyOn: notifyOn,
      rockboxCompat: false, dropSyncBehavior: dropSyncBehavior)
  }
}

enum WireV3Command: Encodable, Sendable {
  case getGlobalConfig(requestID: UUID)
  case setSourceLocation(requestID: UUID, sourceRoot: String?)
  case setGlobalSettings(requestID: UUID, settings: WireV3GlobalSettings)
  case getInventory(requestID: UUID)
  case subscribeInventory(requestID: UUID)
  case unsubscribeInventory(requestID: UUID)
  case adoptDevice(
    deviceID: DeviceID, requestID: UUID,
    selectionMutationID: UUID, selection: WireV3SelectionValue,
    settingsMutationID: UUID, settings: WireV3SettingsValue,
    subscriptionsMutationID: UUID, subscriptions: WireV3SubscriptionsValue)
  case forgetDevice(deviceID: DeviceID, requestID: UUID)
  case getDeviceConfig(deviceID: DeviceID, requestID: UUID)
  case setSelection(
    deviceID: DeviceID, requestID: UUID, mutationID: UUID, selection: WireV3SelectionValue)
  case setSettings(
    deviceID: DeviceID, requestID: UUID, mutationID: UUID, settings: WireV3SettingsValue)
  case setSubscriptions(
    deviceID: DeviceID, requestID: UUID, mutationID: UUID,
    subscriptions: WireV3SubscriptionsValue)
  case triggerSync(deviceID: DeviceID, requestID: UUID, trigger: WireV3SyncTrigger)
  case backfillRockbox(deviceID: DeviceID, requestID: UUID)
  case replaceLibrary(deviceID: DeviceID, requestID: UUID)
  case getHistory(requestID: UUID, limit: UInt32)
  case getLibrary(requestID: UUID)
  case scanLibrary(requestID: UUID)
  case retrySourceMount(requestID: UUID, allowUI: Bool)
  case previewSelection(deviceID: DeviceID, requestID: UUID, selection: WireV3SelectionValue)
  case previewDevice(deviceID: DeviceID, requestID: UUID)
  case resolveTracks(requestID: UUID, rules: [SelectionRule])
  case addSelectionToDevice(
    deviceID: DeviceID, requestID: UUID, mutationID: UUID, rules: [SelectionRule])
  case listPlaylists(requestID: UUID)
  case getPlaylist(requestID: UUID, slug: String)
  case savePlaylist(requestID: UUID, playlist: PlaylistPayload)
  case deletePlaylist(requestID: UUID, slug: String)
  case appendSelectionToPlaylist(requestID: UUID, slug: String, rules: [SelectionRule])
  case shutdown(requestID: UUID)
  case applyReview(route: WireV3Route, requestID: UUID, noDelete: Bool)
  case dryRunReview(route: WireV3Route, requestID: UUID)
  case quitReview(route: WireV3Route, requestID: UUID)
  case promptDecision(route: WireV3Route, requestID: UUID, promptID: UInt64, choice: UInt32)
  case formDecision(route: WireV3Route, requestID: UUID, promptID: UInt64, value: String?)
  case cancelSync(route: WireV3Route, requestID: UUID)
  case pauseSync(route: WireV3Route, requestID: UUID)

  static func newRequestID() -> UUID { UUID() }

  var requestID: UUID {
    switch self {
    case .getGlobalConfig(let id), .setSourceLocation(let id, _),
      .setGlobalSettings(let id, _), .getInventory(let id), .subscribeInventory(let id),
      .unsubscribeInventory(let id), .getHistory(let id, _), .getLibrary(let id),
      .scanLibrary(let id), .retrySourceMount(let id, _), .resolveTracks(let id, _),
      .listPlaylists(let id), .getPlaylist(let id, _), .savePlaylist(let id, _),
      .deletePlaylist(let id, _), .appendSelectionToPlaylist(let id, _, _), .shutdown(let id):
      id
    case .adoptDevice(_, let id, _, _, _, _, _, _), .forgetDevice(_, let id),
      .getDeviceConfig(_, let id), .setSelection(_, let id, _, _),
      .setSettings(_, let id, _, _), .setSubscriptions(_, let id, _, _),
      .triggerSync(_, let id, _), .backfillRockbox(_, let id), .replaceLibrary(_, let id),
      .previewSelection(_, let id, _), .previewDevice(_, let id),
      .addSelectionToDevice(_, let id, _, _):
      id
    case .applyReview(_, let id, _), .dryRunReview(_, let id), .quitReview(_, let id),
      .promptDecision(_, let id, _, _), .formDecision(_, let id, _, _),
      .cancelSync(_, let id), .pauseSync(_, let id):
      id
    }
  }

  private enum CodingKeys: String, CodingKey {
    case type, requestID = "request_id", deviceID = "device_id", sessionID = "session_id"
    case sourceRoot = "source_root", settings, selection, subscriptions
    case selectionMutationID = "selection_mutation_id"
    case settingsMutationID = "settings_mutation_id"
    case subscriptionsMutationID = "subscriptions_mutation_id"
    case mutationID = "mutation_id", trigger, limit, allowUI = "allow_ui"
    case rules, slug, playlist, noDelete = "no_delete", promptID = "prompt_id", choice, value
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: CodingKeys.self)
    func encodeID(_ value: UUID, forKey key: CodingKeys) throws {
      try c.encode(WireV3CanonicalUUID(value: value), forKey: key)
    }
    func base(_ type: String, _ requestID: UUID) throws {
      try c.encode(type, forKey: .type)
      try encodeID(requestID, forKey: .requestID)
    }
    func deviceBase(_ type: String, _ deviceID: DeviceID, _ requestID: UUID) throws {
      try base(type, requestID)
      try c.encode(deviceID, forKey: .deviceID)
    }
    func routeBase(_ type: String, _ route: WireV3Route, _ requestID: UUID) throws {
      try deviceBase(type, route.deviceID, requestID)
      try c.encode(route.sessionID, forKey: .sessionID)
    }
    switch self {
    case .getGlobalConfig(let id): try base("get_global_config", id)
    case .setSourceLocation(let id, let root):
      try base("set_source_location", id)
      if let root { try c.encode(root, forKey: .sourceRoot) } else { try c.encodeNil(forKey: .sourceRoot) }
    case .setGlobalSettings(let id, let settings):
      try base("set_global_settings", id); try c.encode(settings, forKey: .settings)
    case .getInventory(let id): try base("get_inventory", id)
    case .subscribeInventory(let id): try base("subscribe_inventory", id)
    case .unsubscribeInventory(let id): try base("unsubscribe_inventory", id)
    case .adoptDevice(let device, let id, let selectionID, let selection, let settingsID,
      let settings, let subscriptionsID, let subscriptions):
      try deviceBase("adopt_device", device, id)
      try encodeID(selectionID, forKey: .selectionMutationID)
      try c.encode(selection, forKey: .selection)
      try encodeID(settingsID, forKey: .settingsMutationID)
      try c.encode(settings, forKey: .settings)
      try encodeID(subscriptionsID, forKey: .subscriptionsMutationID)
      try c.encode(subscriptions, forKey: .subscriptions)
    case .forgetDevice(let device, let id): try deviceBase("forget_device", device, id)
    case .getDeviceConfig(let device, let id): try deviceBase("get_device_config", device, id)
    case .setSelection(let device, let id, let mutation, let selection):
      try deviceBase("set_selection", device, id); try encodeID(mutation, forKey: .mutationID)
      try c.encode(selection, forKey: .selection)
    case .setSettings(let device, let id, let mutation, let settings):
      try deviceBase("set_settings", device, id); try encodeID(mutation, forKey: .mutationID)
      try c.encode(settings, forKey: .settings)
    case .setSubscriptions(let device, let id, let mutation, let subscriptions):
      try deviceBase("set_subscriptions", device, id)
      try encodeID(mutation, forKey: .mutationID)
      try c.encode(subscriptions, forKey: .subscriptions)
    case .triggerSync(let device, let id, let trigger):
      try deviceBase("trigger_sync", device, id); try c.encode(trigger, forKey: .trigger)
    case .backfillRockbox(let device, let id): try deviceBase("backfill_rockbox", device, id)
    case .replaceLibrary(let device, let id): try deviceBase("replace_library", device, id)
    case .getHistory(let id, let limit): try base("get_history", id); try c.encode(limit, forKey: .limit)
    case .getLibrary(let id): try base("get_library", id)
    case .scanLibrary(let id): try base("scan_library", id)
    case .retrySourceMount(let id, let allowUI):
      try base("retry_source_mount", id); try c.encode(allowUI, forKey: .allowUI)
    case .previewSelection(let device, let id, let selection):
      try deviceBase("preview_selection", device, id); try c.encode(selection, forKey: .selection)
    case .previewDevice(let device, let id): try deviceBase("preview_device", device, id)
    case .resolveTracks(let id, let rules):
      try base("resolve_tracks", id); try c.encode(rules, forKey: .rules)
    case .addSelectionToDevice(let device, let id, let mutation, let rules):
      try deviceBase("add_selection_to_device", device, id)
      try encodeID(mutation, forKey: .mutationID); try c.encode(rules, forKey: .rules)
    case .listPlaylists(let id): try base("list_playlists", id)
    case .getPlaylist(let id, let slug):
      try base("get_playlist", id); try c.encode(slug, forKey: .slug)
    case .savePlaylist(let id, let playlist):
      try base("save_playlist", id); try c.encode(playlist, forKey: .playlist)
    case .deletePlaylist(let id, let slug):
      try base("delete_playlist", id); try c.encode(slug, forKey: .slug)
    case .appendSelectionToPlaylist(let id, let slug, let rules):
      try base("append_selection_to_playlist", id); try c.encode(slug, forKey: .slug)
      try c.encode(rules, forKey: .rules)
    case .shutdown(let id): try base("shutdown", id)
    case .applyReview(let route, let id, let noDelete):
      try routeBase("apply_review", route, id); try c.encode(noDelete, forKey: .noDelete)
    case .dryRunReview(let route, let id): try routeBase("dry_run_review", route, id)
    case .quitReview(let route, let id): try routeBase("quit_review", route, id)
    case .promptDecision(let route, let id, let promptID, let choice):
      try routeBase("prompt_decision", route, id); try c.encode(promptID, forKey: .promptID)
      try c.encode(choice, forKey: .choice)
    case .formDecision(let route, let id, let promptID, let value):
      try routeBase("form_decision", route, id); try c.encode(promptID, forKey: .promptID)
      try c.encode(value, forKey: .value)
    case .cancelSync(let route, let id): try routeBase("cancel_sync", route, id)
    case .pauseSync(let route, let id): try routeBase("pause_sync", route, id)
    }
  }
}
