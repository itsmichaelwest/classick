import Foundation
import Observation

/// Derived UI phase for the menu-bar surface. `.noDevice`/`.notConfigured`
/// take precedence over sync state when deriving from `status_update`, but
/// direct protocol v3 sync progress always wins once a sync is
/// actually streaming — see AppModel.apply for the precedence rules.
enum Phase: Equatable, Sendable {
  case noDevice
  case notConfigured
  case idle
  case syncing(current: Int, total: Int, label: String, etaSecs: UInt64?)
  case scanning(current: Int, total: Int)
  case paused(synced: Int, total: Int?)
  case error(String)
}

struct DeviceState: Equatable, Sendable {
  var serial: String
  var model: String
  var name: String?
  var drive: String
}

struct PendingPrompt: Equatable, Sendable {
  enum Kind: Equatable, Sendable {
    case choice(options: [String])
    case form(initial: String?, hint: String?)
  }

  var route: WireV3Route
  var id: UInt64
  var message: String
  var kind: Kind
}

struct DeviceMountAction: Equatable, Sendable {
  var deviceID: DeviceID
  var inventoryRevision: UInt64
  var mountPath: String
}

/// A `resolved_tracks` reply tagged with the slug of the playlist editor
/// whose `resolve_tracks` request it answers — see
/// `AppModel.latestResolvedTracks`'s doc comment.
struct ResolvedTracksReply: Equatable, Sendable {
  var slug: String
  var tracks: [String]
}

/// The daemon's last-known persisted configuration, as reported by
/// `global_config`. Settings/Setup UI reads this to seed its controls; the daemon remains the
/// store of record.
struct AppConfig: Equatable, Sendable {
  var source: String?
  var daemon: DaemonSettings?
  var ipod: IpodIdentity?
}

/// One device's resolved protocol v3 config,
/// plus its most recently requested `device_preview` (if any). Keyed by
/// serial in `AppModel.deviceConfigs` so the app can hold config for more
/// than one iPod this daemon has ever seen, not just the connected one.
struct DeviceConfigState: Equatable, Sendable {
  var selection: SelectionState
  var subscriptions: SubscriptionsWire
  var settings: DeviceSettingsWire
  var preview: DevicePreview?

  static let defaultState = DeviceConfigState(
    selection: SelectionState(mode: .all, rules: []),
    subscriptions: SubscriptionsWire(playlists: []),
    settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
    preview: nil)
}

enum DeviceConfigRequestIntent: Equatable {
  case read
  case write
}

private struct PendingDeviceRequest {
  var serial: DeviceID
  var generation: UInt64
  var configIntent: DeviceConfigRequestIntent?
}

enum DeviceCommandGate {
  static func allows(
    serial: DeviceID,
    hasAuthoritativeInventory: Bool,
    devices: [DeviceID: DeviceViewState]
  ) -> Bool {
    hasAuthoritativeInventory
      && devices[serial].map { DeviceReadinessLogic.isReady($0.readiness) } == true
  }
}

enum SourceRecoveryPresentation {
  static let attentionTitle = "Music share needs attention"

  static func needsAttention(_ availability: SourceAvailabilityInfo?) -> Bool {
    guard let availability else { return false }
    switch availability.state {
    case .authRequired, .unavailable:
      return true
    case .available, .remounting:
      return false
    }
  }
}

struct SourceConnectIntent {
  private var awaitingActivation = false

  mutating func userRequestedConnect(isApplicationActive: Bool) -> Bool {
    if isApplicationActive {
      awaitingActivation = false
      return true
    }
    awaitingActivation = true
    return false
  }

  mutating func applicationDidBecomeActive() -> Bool {
    guard awaitingActivation else { return false }
    awaitingActivation = false
    return true
  }
}

@Observable
@MainActor
final class AppModel {
  let libraryDragLaunchNonce = UUID()
  private(set) var device: DeviceState?
  private(set) var phase: Phase = .noDevice
  private(set) var pendingPrompt: PendingPrompt?
  private(set) var storageText: String?
  private(set) var config: AppConfig?
  private(set) var syncedCount: Int = 0
  private(set) var libraryCount: Int?
  private(set) var history: [HistoryEntry] = []
  private var libraryDropState = LibraryDropState()
  private(set) var persistedDropAcknowledgements: [String] = []

  var dropOutcome: DropOutcome? {
    libraryDropState.outcome
  }

  // Protocol 1.5.0: the most recently completed run's `finish` rollups,
  // for immediate post-sync display (Task 17). These are separate from
  // `history`'s own `summary`/`dbRestored` (which the daemon
  // persists and rebroadcasts) because a live `finish` line arrives over
  // the routed progress stream before the daemon's own
  // history_update/status_update catches up.
  private(set) var lastRunSkippedForSpace: SkippedForSpace?
  private(set) var lastRunArtwork: ArtworkSummary?
  private(set) var lastRunDbRestored = false

  private(set) var library: LibraryInfo?
  private(set) var sourceAvailability: SourceAvailabilityInfo?
  private var pendingSourceRetryRequestID: String?
  private(set) var selection: SelectionState?
  private(set) var selectionPreview: SelectionPreviewInfo?

  // MARK: - Protocol 1.6.0: playlists, per-device config, device preview

  private(set) var playlists: [PlaylistSummary] = []
  private(set) var playlistRevision: UInt64 = 0
  private(set) var playlistAcknowledgedRequestID: String?
  /// Bumped on every `playlists_update` event, regardless of whether
  /// `playlists`'s content actually changed. The sidebar's "+" in-flight
  /// guard (review finding #2) needs to observe EVERY reply — including
  /// one that's content-identical to the prior list, e.g. the daemon's
  /// error path re-sending the unchanged list — so it can clear pending
  /// state and re-enable the button. A plain `onChange(of: playlists)`
  /// wouldn't fire for a content-identical reply since SwiftUI's
  /// `onChange` only fires when the Equatable value differs.
  private(set) var playlistsUpdateRevision = 0
  /// Reply to the most recent `get_playlist` — for the playlist editor
  /// (Task 7). Not scoped by slug: like `selectionPreview`, there's only
  /// ever one in-flight editor request at a time.
  private(set) var playlistDetail: PlaylistDetail?
  private(set) var configRevision: UInt64 = 0
  private(set) var configAcknowledgedRequestID: String?
  private(set) var deviceConfigAcknowledgedRequestIDs: [DeviceID: String] = [:]
  private(set) var deviceConfigDrafts: [DeviceID: DeviceConfigEditingState] = [:]
  private(set) var devices: [DeviceID: DeviceViewState] = [:]
  private(set) var unidentifiedDevices: [ObservationID: UnidentifiedDeviceViewState] = [:]
  var deviceConfigs: [DeviceID: DeviceConfigState] {
    devices.compactMapValues(\.config)
  }
  /// Sidebar navigation selection. Plain (not `private(set)`) — the
  /// sidebar view binds to this directly.
  var selectedDestination: SidebarDestination?
  private var nextDeviceRequestGeneration: UInt64 = 0
  private var deviceConfigRequests: [String: PendingDeviceRequest] = [:]
  private var latestDeviceConfigGeneration: [DeviceID: UInt64] = [:]
  private var devicePreviewRequests: [String: PendingDeviceRequest] = [:]
  private var latestDevicePreviewGeneration: [DeviceID: UInt64] = [:]

  private var lastInventoryRevision: UInt64?
  private var hasAuthoritativeInventory = false
  private var globalScanSessionID: UInt64?
  private var terminalStateConsumer = TerminalStateConsumer()

  /// Device commands are unsafe until the current daemon connection has
  /// supplied its first authoritative inventory snapshot.
  var deviceActionsAvailable: Bool {
    hasAuthoritativeInventory
  }

  var sourceNeedsAttention: Bool {
    SourceRecoveryPresentation.needsAttention(sourceAvailability)
  }

  var sourceRetryPending: Bool {
    pendingSourceRetryRequestID != nil
  }

  func prepareSourceMountRetry(
    isApplicationActive: Bool, requestID: UUID
  ) -> WireV3Command? {
    guard isApplicationActive, sourceNeedsAttention, pendingSourceRetryRequestID == nil else {
      return nil
    }
    pendingSourceRetryRequestID = requestID.uuidString.lowercased()
    return .retrySourceMount(requestID: requestID, allowUI: true)
  }

  func canSendDeviceCommand(to serial: DeviceID) -> Bool {
    DeviceCommandGate.allows(
      serial: serial,
      hasAuthoritativeInventory: hasAuthoritativeInventory,
      devices: devices)
  }

  func canControlSync(to serial: DeviceID) -> Bool {
    canSendDeviceCommand(to: serial)
      && devices[serial]?.connected == true
      && devices[serial]?.finalization == nil
  }

  func captureMountAction(for serial: DeviceID) -> DeviceMountAction? {
    guard let inventoryRevision = lastInventoryRevision,
      let state = devices[serial], state.connected,
      let mountPath = state.mountPath
    else { return nil }
    return DeviceMountAction(
      deviceID: serial, inventoryRevision: inventoryRevision, mountPath: mountPath)
  }

  func isCurrentMountAction(_ action: DeviceMountAction) -> Bool {
    guard action.inventoryRevision == lastInventoryRevision,
      let state = devices[action.deviceID]
    else { return false }
    return state.connected && state.mountPath == action.mountPath
  }

  @discardableResult
  func completeMountAction(_ action: DeviceMountAction) -> Bool {
    guard isCurrentMountAction(action), var state = devices[action.deviceID] else {
      return false
    }
    state.connected = false
    state.mountPath = nil
    state.phase = .disconnected
    state.sessionID = nil
    state.storage = nil
    state.syncProgress = nil
    state.finalization = nil
    devices[action.deviceID] = state
    refreshFocusedDeviceProjection()
    return true
  }

  func editableDeviceConfig(for serial: DeviceID) -> DeviceConfigState? {
    if let draft = deviceConfigDrafts[serial] {
      var value = draft.value
      value.preview = devices[serial]?.preview
      return value
    }
    return devices[serial]?.config
  }

  func editDeviceSelection(_ selection: SelectionState, for serial: DeviceID) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.selection.edit(selection)
    deviceConfigDrafts[serial] = draft
  }

  func editDeviceSettings(_ settings: DeviceSettingsWire, for serial: DeviceID) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.settings.edit(settings)
    deviceConfigDrafts[serial] = draft
  }

  func editDeviceSubscriptions(_ subscriptions: SubscriptionsWire, for serial: DeviceID) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.subscriptions.edit(subscriptions)
    deviceConfigDrafts[serial] = draft
  }

  func pendingDeviceSelection(for serial: DeviceID) -> SelectionState? {
    guard let draft = deviceConfigDrafts[serial], draft.selection.hasUnsubmittedChanges else {
      return nil
    }
    return draft.selection.value
  }

  func pendingDeviceSettings(for serial: DeviceID) -> DeviceSettingsWire? {
    guard let draft = deviceConfigDrafts[serial], draft.settings.hasUnsubmittedChanges else {
      return nil
    }
    return draft.settings.value
  }

  func pendingDeviceSubscriptions(for serial: DeviceID) -> SubscriptionsWire? {
    guard let draft = deviceConfigDrafts[serial], draft.subscriptions.hasUnsubmittedChanges else {
      return nil
    }
    return draft.subscriptions.value
  }

  func markDeviceSelectionSubmitted(for serial: DeviceID, receipt: DeviceMutationReceipt) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.selection.markSubmitted(requestID: receipt.requestID, mutationID: receipt.mutationID)
    deviceConfigDrafts[serial] = draft
  }

  func markDeviceSettingsSubmitted(for serial: DeviceID, receipt: DeviceMutationReceipt) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.settings.markSubmitted(requestID: receipt.requestID, mutationID: receipt.mutationID)
    deviceConfigDrafts[serial] = draft
  }

  func markDeviceSubscriptionsSubmitted(for serial: DeviceID, receipt: DeviceMutationReceipt) {
    guard var draft = deviceConfigEditor(for: serial) else { return }
    draft.subscriptions.markSubmitted(requestID: receipt.requestID, mutationID: receipt.mutationID)
    deviceConfigDrafts[serial] = draft
  }

  func deviceConfigStatus(
    for serial: DeviceID, component: DeviceConfigComponent
  ) -> DeviceConfigComponentStatus {
    guard let draft = deviceConfigDrafts[serial] else { return .saved }
    return draft.status(for: component, delivery: devices[serial]?.configDelivery)
  }

  private func deviceConfigEditor(for serial: DeviceID) -> DeviceConfigEditingState? {
    if let draft = deviceConfigDrafts[serial] { return draft }
    guard let state = devices[serial] else { return nil }
    return DeviceConfigEditingState(config: state.config ?? .defaultState, state: state)
  }

  // MARK: - Protocol 1.7.0: Add Songs picker track resolution

  /// Most recent `resolved_tracks` reply, tagged with the slug of the
  /// playlist editor that requested it (resolve-reply correlation
  /// hardening). `pendingResolveTracks` is a FIFO queue because the daemon
  /// services one connection's commands in send order, so replies retain
  /// that ordering. Each reply dequeues and is tagged with the front slug.
  ///
  /// This is a single slot, not per-slug storage: only one Add can be in
  /// flight at a time in practice (the sheet disables its Add button while
  /// resolving, and `AddSongsPicker` is a window-modal `.sheet()`, so no
  /// other editor can send a competing request while one is pending).
  /// Tagging the reply is what makes that safe even if that modality
  /// invariant weakens later — each `ManualPlaylistEditor` only consumes a
  /// reply whose `slug` matches its own (see
  /// `ManualPlaylistLogic.shouldConsumeResolvedTracks`), so a stale reply
  /// meant for a different playlist can never be misattributed.
  private(set) var latestResolvedTracks: ResolvedTracksReply?
  /// Bumped on every `resolved_tracks` reply, including an empty one — the
  /// same "revision, not content-equality" pattern as
  /// `playlistsUpdateRevision`, since an empty reply (no rule matched
  /// anything) is a valid outcome the Add Songs sheet must still notice to
  /// stop showing "Adding…".
  private(set) var resolvedTracksRevision = 0
  /// FIFO queue of playlist slugs awaiting a `resolved_tracks` reply — see
  /// `latestResolvedTracks`'s doc comment. A reply with nothing pending is
  /// simply dropped rather than guessed at.
  private var pendingResolveTracks: [String] = []

  /// Whether the daemon's `status_update.state` currently reports a
  /// library scan. Readable outside: the notification observer consults it
  /// to suppress the "Sync complete" banner for `--scan-library` finishes
  /// (which stream the same sync-wire `finish` a real sync does).
  private(set) var isScanning = false
  /// Raw device capacity for the Choose Music footer's capacity bar
  /// (storageText is display-only). Set beside storageText in the
  /// deviceConnected arm from the same `storageFor(drive:)` call;
  /// cleared on deviceDisconnected.
  private(set) var deviceStorage: (free: Int64, total: Int64)?

  // Tracked separately from `device` because `status_update` carries its
  // own `ipod_connected`/`configured` flags independent of the
  // `device_connected`/`device_disconnected` events.
  private var isIpodConnected = false

  // "Configured" is device-aware: the daemon's persisted iPod identity must
  // match the *currently connected* device's serial, not just "some iPod
  // was ever paired". Without this check, swapping in a different,
  // unpaired iPod while a paired one's config is still cached would show
  // "Sync Now" instead of "Set Up Classick…".
  //
  // `hasSeenConfig` comes from `global_config`, the source of truth once
  // we've seen one. Before the first response
  // arrives, `statusConfigured` — the daemon's own device-agnostic
  // `status_update.configured` flag — is used as a fallback so the menu
  // doesn't flash "Set Up Classick…" during the startup handshake.
  private var configuredSerial: String?
  private var hasSeenConfig = false
  private var statusConfigured = false

  private var isConfiguredForCurrentDevice: Bool {
    // Until the config reply lands (`hasSeenConfig`), we don't yet know
    // *which* iPod is paired, so we can't device-match. Trust the daemon's
    // device-agnostic `status_update.configured` flag in that window — this
    // avoids flashing "Set Up Classick…" during the startup handshake AND
    // on every reconnect of an already-configured device (where
    // inventory arrives before global configuration).
    guard hasSeenConfig else { return statusConfigured }
    // Config known but the `device_connected` event hasn't arrived yet:
    // fall back to "is anything paired at all".
    guard let device else { return configuredSerial != nil }
    // Both known: the paired serial must match the connected device, so a
    // swapped-in unpaired iPod correctly shows "Set Up Classick…".
    return device.serial == configuredSerial
  }

  /// The user has never completed first-run setup: the daemon has reported
  /// its persisted config (post-handshake, so `hasSeenConfig`) and it carries
  /// no music-library source. Stays `false` until the config reply lands, so
  /// first-run auto-presentation waits for the handshake instead of firing
  /// during the startup race. The daemon always answers `get_config` — with
  /// a `global_config` response when nothing is persisted — so this reliably
  /// flips `true` on a fresh machine.
  var needsFirstRunSetup: Bool {
    hasSeenConfig && (config?.source?.isEmpty ?? true)
  }

  func apply(_ event: WireV3Event) {
    switch event {
    case .hello:
      resetForProtocolEpoch()
    case .globalConfig(let wire):
      guard wire.revision >= configRevision else { return }
      config = AppConfig(source: wire.sourceRoot, daemon: wire.settings.appValue, ipod: nil)
      configRevision = wire.revision
      configAcknowledgedRequestID = wire.requestID?.uuidString.lowercased()
      hasSeenConfig = true
    case .sourceAvailability(let wire):
      applySourceAvailability(wire)
    case .inventorySubscriptionChanged:
      break
    case .deviceInventory(let inventory):
      guard lastInventoryRevision.map({ inventory.revision > $0 }) ?? true else { return }
      lastInventoryRevision = inventory.revision
      hasAuthoritativeInventory = true
      let previous = devices
      devices = DeviceReducer.reduce(inventory: inventory, previous: previous)
      terminalStateConsumer.reconcile(devices: &devices, previous: previous)
      unidentifiedDevices = Dictionary(uniqueKeysWithValues: inventory.unidentified.map {
        ($0.observationID, UnidentifiedDeviceViewState(
          observationID: $0.observationID, readiness: $0.readiness, hardware: $0.hardware))
      })
      refreshFocusedDeviceProjection()
    case .deviceConfig(let wire):
      applyDeviceConfig(wire)
    case .configMutationFailed(let failure):
      applyConfigMutationFailure(failure)
    case .deviceForgotten(let forgotten):
      devices.removeValue(forKey: forgotten.deviceID)
      deviceConfigAcknowledgedRequestIDs.removeValue(forKey: forgotten.deviceID)
      deviceConfigDrafts.removeValue(forKey: forgotten.deviceID)
      refreshFocusedDeviceProjection()
    case .syncAccepted(let accepted):
      guard var state = devices[accepted.deviceID] else { return }
      state.phase = .syncing
      state.sessionID = accepted.sessionID
      state.syncProgress = nil
      state.finalization = nil
      devices[accepted.deviceID] = state
      refreshFocusedDeviceProjection()
    case .syncRejected(let rejected):
      guard var state = devices[rejected.deviceID] else { return }
      state.phase = .error(rejected.message)
      devices[rejected.deviceID] = state
      refreshFocusedDeviceProjection()
    case .history(let update):
      history = update.entries.map(\.appValue)
      for (deviceID, var state) in devices {
        let entries = update.entries.filter { $0.deviceID == deviceID }
        state.latestAttempt = entries.last?.appValue
        state.latestSuccessfulSync = entries.last(where: { $0.outcome == "ok" })?.appValue
        devices[deviceID] = state
      }
    case .library(let update):
      library = LibraryInfo(
        sourceRoot: update.sourceRoot, scannedAtUnixSecs: update.scannedAtUnixSecs,
        artists: update.artists, genres: update.genres, totalTracks: update.totalTracks,
        totalBytes: update.totalBytes,
        acknowledgedRequestID: update.requestID?.uuidString.lowercased())
    case .libraryScan(let scan):
      applyLibraryScan(scan)
    case .selectionPreview(let preview):
      selectionPreview = SelectionPreviewInfo(
        selectedTracks: preview.selectedTracks, selectedBytes: preview.selectedBytes,
        adds: preview.adds, removes: preview.removes, serial: preview.deviceID.rawValue,
        acknowledgedRequestID: preview.requestID.uuidString.lowercased())
    case .devicePreview(let preview):
      applyDevicePreview(preview)
    case .resolvedTracks(let resolved):
      guard !pendingResolveTracks.isEmpty else { return }
      latestResolvedTracks = ResolvedTracksReply(
        slug: pendingResolveTracks.removeFirst(), tracks: resolved.tracks)
      resolvedTracksRevision += 1
    case .playlists(let update):
      guard update.revision >= playlistRevision else { return }
      playlists = update.playlists
      playlistRevision = update.revision
      playlistAcknowledgedRequestID = update.requestID?.uuidString.lowercased()
      playlistsUpdateRevision += 1
      if let slug = playlistDetail?.slug, !playlists.contains(where: { $0.slug == slug }) {
        playlistDetail = nil
      }
    case .playlistDetail(let update):
      guard update.revision >= playlistRevision else { return }
      playlistDetail = Self.playlistDetail(from: update)
    case .playlistSaved(let saved):
      guard saved.revision >= playlistRevision else { return }
      playlistRevision = saved.revision
      playlistAcknowledgedRequestID = saved.requestID.uuidString.lowercased()
      playlistDetail = Self.playlistDetail(
        from: saved.playlist, revision: saved.revision, requestID: saved.requestID)
    case .deviceSelectionAdded(let reply):
      applyDeviceSelectionAdded(reply)
    case .playlistSelectionAppended(let reply):
      applyPlaylistSelectionAppended(reply)
    case .libraryMutationRejected(let rejection):
      let requestID = rejection.requestID.uuidString.lowercased()
      let target: LibraryMutationTarget
      switch rejection.target {
      case .deviceSelection(let id): target = .deviceSelection(serial: id.rawValue)
      case .manualPlaylist(let slug): target = .manualPlaylist(slug: slug)
      }
      let completed = libraryDropState.reject(
        requestID: requestID, target: target, message: rejection.message)
      recordPersistedDropAcknowledgement(requestID, if: completed)
    case .daemonShutdownStarted:
      break
    case .commandFailed(let failure):
      if pendingSourceRetryRequestID == failure.requestID.uuidString.lowercased() {
        pendingSourceRetryRequestID = nil
      }
      phase = .error(failure.message)
    case .progress(let progress):
      applyProgress(progress)
    }
  }

  private func applySourceAvailability(_ wire: WireV3SourceAvailabilityEvent) {
    let requestID = wire.requestID?.uuidString.lowercased()
    if let requestID, let pendingSourceRetryRequestID {
      guard requestID == pendingSourceRetryRequestID else { return }
      self.pendingSourceRetryRequestID = nil
    } else if wire.requestID == nil, wire.state != .remounting {
      pendingSourceRetryRequestID = nil
    }
    sourceAvailability = SourceAvailabilityInfo(
      state: wire.state, sourceRoot: wire.sourceRoot, acknowledgedRequestID: requestID)
    if wire.state == .available, let sourceRoot = wire.sourceRoot {
      library?.sourceRoot = sourceRoot
      config?.source = sourceRoot
    }
  }

  private func applyDeviceConfig(_ wire: WireV3DeviceConfig) {
    let requestID = wire.requestID?.uuidString.lowercased()
    guard shouldApplyProtocol3DeviceConfigResponse(serial: wire.deviceID, requestID: requestID),
      var state = devices[wire.deviceID]
    else { return }
    var config = state.config ?? .defaultState
    var delivery = state.configDelivery
    var editor = deviceConfigDrafts[wire.deviceID]
      ?? DeviceConfigEditingState(config: config, state: state)

    if wire.selection.revision >= state.selectionRevision {
      config.selection = wire.selection.value
      state.selectionRevision = wire.selection.revision
      editor.selection.reconcile(
        canonical: wire.selection.value, revision: wire.selection.revision,
        acknowledgedRequestID: requestID,
        acknowledgedMutationID: wire.selection.mutationID.uuidString.lowercased())
    }
    if wire.settings.revision >= state.settingsRevision {
      config.settings = wire.settings.value
      state.settingsRevision = wire.settings.revision
      editor.settings.reconcile(
        canonical: wire.settings.value, revision: wire.settings.revision,
        acknowledgedRequestID: requestID,
        acknowledgedMutationID: wire.settings.mutationID.uuidString.lowercased())
    }
    if wire.subscriptions.revision >= state.subscriptionsRevision {
      config.subscriptions = wire.subscriptions.value
      state.subscriptionsRevision = wire.subscriptions.revision
      editor.subscriptions.reconcile(
        canonical: wire.subscriptions.value, revision: wire.subscriptions.revision,
        acknowledgedRequestID: requestID,
        acknowledgedMutationID: wire.subscriptions.mutationID.uuidString.lowercased())
    }
    if var currentDelivery = delivery {
      if wire.selection.revision >= currentDelivery.selection.revision {
        currentDelivery.selection = wire.selection
      }
      if wire.settings.revision >= currentDelivery.settings.revision {
        currentDelivery.settings = wire.settings
      }
      if wire.subscriptions.revision >= currentDelivery.subscriptions.revision {
        currentDelivery.subscriptions = wire.subscriptions
      }
      delivery = currentDelivery
    } else {
      delivery = DeviceConfigDeliveryState(
        selection: wire.selection, settings: wire.settings, subscriptions: wire.subscriptions)
    }
    config.preview = state.preview
    state.config = config
    state.configDelivery = delivery
    devices[wire.deviceID] = state
    deviceConfigDrafts[wire.deviceID] = editor
    if let requestID { deviceConfigAcknowledgedRequestIDs[wire.deviceID] = requestID }
  }

  private func applyConfigMutationFailure(_ failure: WireV3ConfigMutationFailedEvent) {
    guard var editor = deviceConfigDrafts[failure.deviceID] else { return }
    let requestID = failure.requestID.uuidString.lowercased()
    let mutationID = failure.mutationID.uuidString.lowercased()
    let component = DeviceConfigComponent(rawValue: failure.component)
    if failure.stage == "host_acceptance" {
      switch component {
      case .selection: editor.selection.reject(requestID: requestID, mutationID: mutationID, message: failure.message)
      case .settings: editor.settings.reject(requestID: requestID, mutationID: mutationID, message: failure.message)
      case .subscriptions: editor.subscriptions.reject(requestID: requestID, mutationID: mutationID, message: failure.message)
      case nil: return
      }
      deviceConfigDrafts[failure.deviceID] = editor
      return
    }
    guard failure.stage == "device_delivery", var state = devices[failure.deviceID],
      var delivery = state.configDelivery
    else { return }
    let failed = WireV3Delivery(state: "pending_device", lastFailure: failure.message)
    switch component {
    case .selection
      where delivery.selection.delivery.state == "pending_device"
        && editor.selection.acceptsDeliveryFailure(requestID: requestID, mutationID: mutationID):
      delivery.selection = .init(
        revision: delivery.selection.revision, mutationID: delivery.selection.mutationID,
        value: delivery.selection.value, delivery: failed)
    case .settings
      where delivery.settings.delivery.state == "pending_device"
        && editor.settings.acceptsDeliveryFailure(requestID: requestID, mutationID: mutationID):
      delivery.settings = .init(
        revision: delivery.settings.revision, mutationID: delivery.settings.mutationID,
        value: delivery.settings.value, delivery: failed)
    case .subscriptions
      where delivery.subscriptions.delivery.state == "pending_device"
        && editor.subscriptions.acceptsDeliveryFailure(
          requestID: requestID, mutationID: mutationID):
      delivery.subscriptions = .init(
        revision: delivery.subscriptions.revision, mutationID: delivery.subscriptions.mutationID,
        value: delivery.subscriptions.value, delivery: failed)
    default: return
    }
    state.configDelivery = delivery
    devices[failure.deviceID] = state
  }

  private func applyLibraryScan(_ scan: WireV3LibraryScanEvent) {
    switch scan.kind {
    case .started:
      isScanning = true
      globalScanSessionID = scan.sessionID
      phase = .scanning(current: 0, total: 0)
    case .progress:
      guard isScanning, globalScanSessionID == scan.sessionID else { return }
      phase = .scanning(
        current: Int(scan.tracksIndexed ?? 0), total: Int(scan.filesScanned ?? 0))
    case .finished:
      guard globalScanSessionID == scan.sessionID else { return }
      isScanning = false
      globalScanSessionID = nil
      phase = computePhase(targetSyncing: false)
    }
  }

  private func applyDevicePreview(_ preview: WireV3DevicePreviewEvent) {
    let requestID = preview.requestID.uuidString.lowercased()
    guard shouldApplyDeviceResponse(
      serial: preview.deviceID, requestID: requestID, requests: devicePreviewRequests,
      latestGeneration: latestDevicePreviewGeneration, allowsUnsolicitedResponse: false),
      var state = devices[preview.deviceID]
    else { return }
    let value = DevicePreview(
      serial: preview.deviceID.rawValue, selectedTracks: preview.selectedTracks,
      selectedBytes: preview.selectedBytes, playlistExtraTracks: preview.playlistExtraTracks,
      playlistExtraBytes: preview.playlistExtraBytes,
      projectedFreeBytes: preview.projectedFreeBytes,
      unresolvedSubscriptions: preview.unresolvedSubscriptions,
      acknowledgedRequestID: requestID)
    state.preview = value
    if state.config == nil { state.config = .defaultState }
    state.config?.preview = value
    devices[preview.deviceID] = state
  }

  private func applyDeviceSelectionAdded(_ reply: WireV3DeviceSelectionAddedEvent) {
    guard var state = devices[reply.deviceID], reply.selectionRevision >= state.selectionRevision
    else { return }
    var deviceConfig = state.config ?? .defaultState
    deviceConfig.selection = reply.selection.value
    state.config = deviceConfig
    state.selectionRevision = reply.selectionRevision
    devices[reply.deviceID] = state
    let requestID = reply.requestID.uuidString.lowercased()
    deviceConfigAcknowledgedRequestIDs[reply.deviceID] = requestID
    let completed = libraryDropState.completeDevice(
      requestID: requestID, serial: reply.deviceID, delivery: reply.dropDelivery)
    recordPersistedDropAcknowledgement(requestID, if: completed)
  }

  private func applyPlaylistSelectionAppended(_ reply: WireV3PlaylistSelectionAppendedEvent) {
    guard reply.revision >= playlistRevision else { return }
    playlistRevision = reply.revision
    let requestID = reply.requestID.uuidString.lowercased()
    playlistAcknowledgedRequestID = requestID
    playlistDetail = Self.playlistDetail(
      from: reply.playlist, revision: reply.revision, requestID: reply.requestID)
    let completed = libraryDropState.completePlaylist(
      requestID: requestID, slug: reply.slug, appendedTracks: reply.appendedTracks)
    recordPersistedDropAcknowledgement(requestID, if: completed)
  }

  private func applyProgress(_ progress: WireV3ProgressEvent) {
    guard var state = devices[progress.route.deviceID],
      state.sessionID == progress.route.sessionID
    else { return }
    state = DeviceReducer.reduce(progress: progress, into: state)
    switch progress.kind {
    case .prompt:
      if let id = progress.promptID, let message = progress.message, let options = progress.options {
        pendingPrompt = PendingPrompt(
          route: progress.route, id: id, message: message, kind: .choice(options: options))
      }
    case .form:
      if let id = progress.promptID, let label = progress.label {
        pendingPrompt = PendingPrompt(
          route: progress.route, id: id, message: label,
          kind: .form(initial: progress.initial, hint: progress.hint))
      }
    case .syncError:
      if let message = progress.message { state.phase = .error(message) }
    case .syncPaused:
      state.phase = .paused
    default:
      break
    }
    devices[progress.route.deviceID] = state
    refreshFocusedDeviceProjection()
  }

  private func resetForProtocolEpoch() {
    deviceConfigDrafts = deviceConfigDrafts.mapValues { draft in
      var draft = draft
      draft.prepareForProtocolReconnect()
      return draft
    }
    nextDeviceRequestGeneration = 0
    deviceConfigRequests.removeAll()
    latestDeviceConfigGeneration.removeAll()
    devicePreviewRequests.removeAll()
    latestDevicePreviewGeneration.removeAll()
    pendingResolveTracks.removeAll()
    lastInventoryRevision = nil
    hasAuthoritativeInventory = false
    globalScanSessionID = nil
    isScanning = false
    terminalStateConsumer.reset()
    devices.removeAll()
    unidentifiedDevices.removeAll()
    device = nil
    phase = .noDevice
    pendingPrompt = nil
    storageText = nil
    config = nil
    configuredSerial = nil
    hasSeenConfig = false
    statusConfigured = false
    isIpodConnected = false
    syncedCount = 0
    libraryCount = nil
    selectionPreview = nil
    lastRunSkippedForSpace = nil
    lastRunArtwork = nil
    lastRunDbRestored = false
    deviceStorage = nil
    sourceAvailability = nil
    pendingSourceRetryRequestID = nil
    playlistRevision = 0
    playlistAcknowledgedRequestID = nil
    configRevision = 0
    configAcknowledgedRequestID = nil
    deviceConfigAcknowledgedRequestIDs.removeAll()
  }

  private static func playlistDetail(from event: WireV3PlaylistDetailEvent) -> PlaylistDetail {
    switch event.result {
    case .found(let playlist):
      playlistDetail(from: playlist, revision: event.revision, requestID: event.requestID)
    case .unavailable(let message):
      PlaylistDetail(
        slug: event.slug, name: nil, kind: nil, tracks: nil, rules: nil, error: message,
        playlistRevision: event.revision,
        acknowledgedRequestID: event.requestID.uuidString.lowercased())
    }
  }

  private static func playlistDetail(
    from playlist: WireV3StoredPlaylist, revision: UInt64, requestID: UUID
  ) -> PlaylistDetail {
    let detail = playlist.detail
    return PlaylistDetail(
      slug: detail.slug, name: detail.name, kind: detail.kind, tracks: detail.tracks,
      rules: detail.rules, error: nil, playlistRevision: revision,
      acknowledgedRequestID: requestID.uuidString.lowercased())
  }

  /// Called once a surfaced `pendingPrompt` has been answered (its
  /// `decide_prompt` sent) so the same prompt isn't re-presented.
  func clearPendingPrompt(route: WireV3Route, promptID: UInt64) {
    guard pendingPrompt?.route == route, pendingPrompt?.id == promptID else { return }
    pendingPrompt = nil
  }

  func markLibraryDropAdding(requestID: UUID, target: LibraryDropTarget) {
    libraryDropState.markAdding(requestID: requestID, target: target)
  }

  func isLibraryDropAdding(target: LibraryDropTarget) -> Bool {
    libraryDropState.isAdding(target: target)
  }

  func isLibraryDropAdding(requestID: UUID) -> Bool {
    libraryDropState.isAdding(requestID: requestID)
  }

  func rejectLibraryDropLocally(
    requestID: UUID, target: LibraryDropTarget, message: String
  ) {
    libraryDropState.rejectLocally(requestID: requestID, target: target, message: message)
  }

  private func recordPersistedDropAcknowledgement(_ requestID: String, if completed: Bool) {
    guard completed, !persistedDropAcknowledgements.contains(requestID) else { return }
    persistedDropAcknowledgements.append(requestID)
  }

  /// Clears the currently presented failure after the user dismisses Details.
  /// The attempt identity remains suppressed so an unchanged daemon snapshot
  /// cannot immediately resurrect the same error.
  func dismissTerminalError(for serial: DeviceID) {
    terminalStateConsumer.dismiss(serial: serial, devices: &devices)
    refreshFocusedDeviceProjection()
  }

  func willRequestDeviceConfig(
    serial: DeviceID, requestID: String, intent: DeviceConfigRequestIntent
  ) {
    registerDeviceRequest(
      serial: serial,
      requestID: requestID,
      configIntent: intent,
      requests: &deviceConfigRequests,
      latestGeneration: &latestDeviceConfigGeneration)
  }

  func willRequestDevicePreview(serial: DeviceID, requestID: String) {
    registerDeviceRequest(
      serial: serial,
      requestID: requestID,
      configIntent: nil,
      requests: &devicePreviewRequests,
      latestGeneration: &latestDevicePreviewGeneration)
  }

  /// Call immediately before sending `.resolveTracks(rules:)` — see
  /// `pendingResolveTracks`'s doc comment. `slug` is the requesting
  /// playlist editor's own slug, tagged onto the eventual reply so it can
  /// tell its own reply apart from another editor's.
  func willRequestResolveTracks(slug: String) {
    pendingResolveTracks.append(slug)
  }

  /// `noDevice`/`notConfigured` precedence used when deriving phase from
  /// connection/config state. Routed sync progress events
  /// bypass this and set `.syncing`/`.idle` directly.
  private func computePhase(targetSyncing: Bool) -> Phase {
    guard isIpodConnected else { return .noDevice }
    guard isConfiguredForCurrentDevice else { return .notConfigured }
    guard targetSyncing else {
      // A paused sync is a resting state, not plain idle: the sync
      // subprocess has already emitted `paused` and exited, so the daemon
      // now broadcasts `idle`. Without this, that trailing idle status
      // would wipe `.paused` and the menu would silently drop the Resume
      // affordance. Hold `.paused` (refreshing its X/Y from the latest
      // status) until the user resumes (targetSyncing → `.syncing`
      // below), the device disconnects (guard above), or the app restarts
      // (phase starts at `.noDevice`, so a cold idle status shows the
      // normal "X synced" count, never a phantom pause).
      if case .paused = phase { return .paused(synced: syncedCount, total: libraryCount) }
      return .idle
    }
    if case .syncing = phase { return phase }
    return .syncing(current: 0, total: 0, label: "", etaSecs: nil)
  }

  private func shouldApplyProtocol3DeviceConfigResponse(
    serial: DeviceID, requestID: String?
  ) -> Bool {
    guard let requestID else {
      invalidateLatestDeviceConfigRead(for: serial)
      return true
    }
    guard let request = deviceConfigRequests[requestID] else { return false }
    guard request.serial == serial else { return false }
    if request.configIntent == .write { return true }
    return latestDeviceConfigGeneration[serial] == request.generation
  }

  private func refreshFocusedDeviceProjection() {
    guard let serial = focusedDeviceSerial, let state = devices[serial] else {
      device = nil
      phase = .noDevice
      storageText = nil
      deviceStorage = nil
      return
    }

    device = DeviceState(
      serial: serial.rawValue,
      model: state.identity.modelLabel,
      name: state.identity.name,
      drive: state.mountPath ?? "")
    syncedCount = state.syncedCount
    libraryCount = state.libraryCount
    if let storage = state.storage {
      deviceStorage = (
        free: Int64(clamping: storage.free),
        total: Int64(clamping: storage.total)
      )
      storageText = Self.formatStorage(deviceStorage)
    } else {
      deviceStorage = nil
      storageText = nil
    }

    switch state.phase {
    case .disconnected:
      phase = .noDevice
    case .unconfigured:
      phase = .notConfigured
    case .idle:
      phase = .idle
    case .syncing:
      let progress = state.syncProgress
      phase = .syncing(
        current: progress?.current ?? 0,
        total: progress?.total ?? 0,
        label: progress?.label ?? "",
        etaSecs: progress?.etaSecs)
    case .paused:
      phase = .paused(synced: state.syncedCount, total: state.libraryCount)
    case .error(let message):
      phase = .error(message)
    }

    if let rollup = state.lastRun {
      lastRunSkippedForSpace = rollup.skippedForSpace
      lastRunArtwork = rollup.artwork
      lastRunDbRestored = rollup.dbRestored
    }
  }

  private static func formatStorage(_ pair: (free: Int64, total: Int64)?) -> String? {
    guard let pair else { return nil }
    let freeGB = pair.free / 1_000_000_000
    let totalGB = pair.total / 1_000_000_000
    return "\(freeGB) / \(totalGB) GB"
  }

  private static func humanReadable(rejection reason: String) -> String {
    switch reason {
    case "already_syncing": return "A sync is already in progress."
    case "no_ipod": return "No iPod is connected."
    case "not_configured": return "Classick isn't configured yet."
    case "too_many_failures": return "Sync disabled after repeated failures."
    default: return reason
    }
  }

  private func registerDeviceRequest(
    serial: DeviceID, requestID: String, configIntent: DeviceConfigRequestIntent?,
    requests: inout [String: PendingDeviceRequest],
    latestGeneration: inout [DeviceID: UInt64]
  ) {
    nextDeviceRequestGeneration += 1
    let generation = nextDeviceRequestGeneration
    requests[requestID] = PendingDeviceRequest(
      serial: serial, generation: generation, configIntent: configIntent)
    latestGeneration[serial] = generation
  }

  private func shouldApplyDeviceResponse(
    serial: DeviceID, requestID: String, requests: [String: PendingDeviceRequest],
    latestGeneration: [DeviceID: UInt64], allowsUnsolicitedResponse: Bool
  ) -> Bool {
    guard let request = requests[requestID] else { return allowsUnsolicitedResponse }
    return request.serial == serial && latestGeneration[serial] == request.generation
  }

  private func invalidateLatestDeviceConfigRead(for serial: DeviceID) {
    guard let latestGeneration = latestDeviceConfigGeneration[serial],
      deviceConfigRequests.values.contains(where: {
        $0.serial == serial && $0.generation == latestGeneration && $0.configIntent == .read
      })
    else { return }
    nextDeviceRequestGeneration += 1
    latestDeviceConfigGeneration[serial] = nextDeviceRequestGeneration
  }
}

#if DEBUG
  extension AppModel {
    func seedPreviewStorage(free: Int64, total: Int64) {
      deviceStorage = (free, total)
      storageText = Self.formatStorage(deviceStorage)
    }
  }
#endif
