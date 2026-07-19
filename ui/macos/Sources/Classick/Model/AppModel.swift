import Foundation
import Observation

/// Derived UI phase for the menu-bar surface. `.noDevice`/`.notConfigured`
/// take precedence over sync state when deriving from `status_update`, but
/// direct sync progress (`sync_event` lines) always wins once a sync is
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
  var id: UInt64
  var message: String
  var options: [String]
}

/// A `resolved_tracks` reply tagged with the slug of the playlist editor
/// whose `resolve_tracks` request it answers — see
/// `AppModel.latestResolvedTracks`'s doc comment.
struct ResolvedTracksReply: Equatable, Sendable {
  var slug: String
  var tracks: [String]
}

/// The daemon's last-known persisted configuration, as pushed by
/// `config_update`. Settings/Setup UI reads this to seed its controls and
/// writes back via `save_config`; the daemon (not this app) remains the
/// store of record.
struct AppConfig: Equatable, Sendable {
  var source: String?
  var daemon: DaemonSettings?
  var ipod: IpodIdentity?
}

/// One device's resolved config (protocol v1.6.0's `device_config_update`),
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

enum DeviceConfigRequestIntent {
  case read
  case write
}

private struct PendingDeviceRequest {
  var serial: DeviceSerial
  var generation: UInt64
  var configIntent: DeviceConfigRequestIntent?
}

enum DeviceCommandGate {
  static func allows(
    serial: DeviceSerial,
    hasAuthoritativeInventory: Bool,
    devices: [DeviceSerial: DeviceViewState]
  ) -> Bool {
    hasAuthoritativeInventory && devices[serial] != nil
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
  private(set) var device: DeviceState?
  private(set) var phase: Phase = .noDevice
  private(set) var lastSync: HistoryEntry?
  private(set) var pendingPrompt: PendingPrompt?
  private(set) var storageText: String?
  private(set) var config: AppConfig?
  private(set) var syncedCount: Int = 0
  private(set) var libraryCount: Int?
  private(set) var history: [HistoryEntry] = []

  // Protocol 1.5.0: the most recently completed run's `finish` rollups,
  // for immediate post-sync display (Task 17). These are separate from
  // `lastSync`/`history`'s own `summary`/`dbRestored` (which the daemon
  // persists and rebroadcasts) because a live `finish` line arrives over
  // the forwarded sync_event stream before the daemon's own
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
  private(set) var devices: [DeviceSerial: DeviceViewState] = [:]
  var deviceConfigs: [String: DeviceConfigState] {
    devices.compactMapValues(\.config)
  }
  /// Sidebar navigation selection. Plain (not `private(set)`) — the
  /// sidebar view binds to this directly.
  var selectedDestination: SidebarDestination?
  private var nextDeviceRequestGeneration: UInt64 = 0
  private var deviceConfigRequests: [String: PendingDeviceRequest] = [:]
  private var latestDeviceConfigGeneration: [DeviceSerial: UInt64] = [:]
  private var devicePreviewRequests: [String: PendingDeviceRequest] = [:]
  private var latestDevicePreviewGeneration: [DeviceSerial: UInt64] = [:]

  private var lastInventoryRevision: UInt64?
  private var hasAuthoritativeInventory = false
  private var globalScanSessionID: UInt64?

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
    isApplicationActive: Bool, requestID: String
  ) -> DaemonCommand? {
    guard isApplicationActive, sourceNeedsAttention, pendingSourceRetryRequestID == nil else {
      return nil
    }
    pendingSourceRetryRequestID = requestID
    return .retrySourceMount(allowUI: true, requestID: requestID)
  }

  func canSendDeviceCommand(to serial: DeviceSerial) -> Bool {
    DeviceCommandGate.allows(
      serial: serial,
      hasAuthoritativeInventory: hasAuthoritativeInventory,
      devices: devices)
  }

  func canControlSync(to serial: DeviceSerial) -> Bool {
    canSendDeviceCommand(to: serial) && devices[serial]?.finalization == nil
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
  // `configuredSerial`/`hasSeenConfig` come from `config_update` (the
  // source of truth once we've seen one). Before the first `config_update`
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
    // `status_update` arrives before `config_update`).
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
  /// an empty `config_update` when nothing is persisted — so this reliably
  /// flips `true` on a fresh machine.
  var needsFirstRunSetup: Bool {
    hasSeenConfig && (config?.source?.isEmpty ?? true)
  }

  private let decoder = JSONDecoder()

  func apply(_ ev: DaemonEvent) {
    switch ev {
    case .hello:
      // Correlation and monotonic revisions are scoped to one connection.
      // A reply left behind by a dead socket must not affect the new epoch.
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
      devices.removeAll()
      device = nil
      phase = .noDevice
      lastSync = nil
      pendingPrompt = nil
      storageText = nil
      config = nil
      syncedCount = 0
      libraryCount = nil
      selectionPreview = nil
      lastRunSkippedForSpace = nil
      lastRunArtwork = nil
      lastRunDbRestored = false
      deviceStorage = nil
      isIpodConnected = false
      configuredSerial = nil
      hasSeenConfig = false
      statusConfigured = false
      sourceAvailability = nil
      pendingSourceRetryRequestID = nil

    case .unknown:
      break

    case .historyUpdate(let entries, _):
      history = entries

    case .libraryUpdate(let info):
      library = info

    case .sourceAvailability(let info):
      if let acknowledgedRequestID = info.acknowledgedRequestID {
        if let pendingSourceRetryRequestID {
          guard acknowledgedRequestID == pendingSourceRetryRequestID else { break }
          self.pendingSourceRetryRequestID = nil
        }
      } else if info.state != .remounting {
        pendingSourceRetryRequestID = nil
      }
      sourceAvailability = info
      if case .available = info.state, let sourceRoot = info.sourceRoot {
        library?.sourceRoot = sourceRoot
        config?.source = sourceRoot
      }

    case .selectionUpdate(let mode, let rules, _, _):
      selection = SelectionState(mode: mode, rules: rules)

    case .selectionPreview(let info):
      selectionPreview = info

    case .playlistsUpdate(let list, _):
      playlists = list
      playlistsUpdateRevision += 1

    case .playlistDetail(let detail):
      playlistDetail = detail

    case .deviceConfigUpdate(
      let serial, let selection, let subscriptions, let settings, let acknowledgedRequestID):
      guard shouldApplyDeviceConfigResponse(serial: serial, requestID: acknowledgedRequestID) else {
        break
      }
      guard var deviceState = devices[serial] else { break }
      var config = deviceState.config ?? .defaultState
      config.selection = selection
      config.subscriptions = subscriptions
      config.settings = settings
      deviceState.config = config
      devices[serial] = deviceState

    case .devicePreview(let preview):
      let serial = preview.serial
      guard
        shouldApplyDeviceResponse(
          serial: serial,
          requestID: preview.acknowledgedRequestID,
          requests: devicePreviewRequests,
          latestGeneration: latestDevicePreviewGeneration,
          allowsUnsolicitedResponse: false)
      else { break }
      guard var deviceState = devices[serial] else { break }
      deviceState.preview = preview
      if deviceState.config == nil {
        deviceState.config = .defaultState
      }
      deviceState.config?.preview = preview
      devices[serial] = deviceState

    case .configUpdate(let source, let daemon, let ipod, _, _):
      config = AppConfig(source: source, daemon: daemon, ipod: ipod)
      // The daemon considers itself configured once it has a persisted
      // iPod identity (daemon: `configured = configured_serial.is_some()`).
      // It emits `config_update` (not a pushed `status_update`) after a
      // `save_config`, so derive the flag here too or the menu would stay
      // stuck on "Set Up…" right after first-run setup. Track the serial
      // itself (not just presence) so a later device swap is caught by
      // `isConfiguredForCurrentDevice`.
      hasSeenConfig = true
      configuredSerial = ipod?.serial
      phase = computePhase(targetSyncing: phaseIsSyncing)

    case .deviceConnected(let serial, let modelLabel, let drive, let name):
      device = DeviceState(serial: serial, model: modelLabel, name: name, drive: drive)
      isIpodConnected = true
      let storage = storageFor(drive: drive)
      deviceStorage = storage
      storageText = Self.formatStorage(storage)
      phase = computePhase(targetSyncing: phaseIsSyncing)

    case .deviceDisconnected:
      device = nil
      isIpodConnected = false
      storageText = nil
      deviceStorage = nil
      phase = computePhase(targetSyncing: false)

    case .statusUpdate(let info):
      statusConfigured = info.configured
      isIpodConnected = info.ipodConnected
      lastSync = info.lastSync
      syncedCount = info.syncedCount
      libraryCount = info.libraryCount
      let wasScanning = isScanning
      isScanning = (info.state == .scanning)
      if !isScanning || !wasScanning {
        globalScanSessionID = nil
      }
      let targetSyncing: Bool
      switch info.state {
      case .syncing: targetSyncing = true
      case .idle, .scanning: targetSyncing = false
      }
      if info.state == .scanning {
        // Preserve in-flight scan progress across status rebroadcasts.
        if case .scanning = phase {} else { phase = .scanning(current: 0, total: 0) }
      } else if hasAuthoritativeInventory {
        refreshFocusedDeviceProjection()
      } else {
        phase = computePhase(targetSyncing: targetSyncing)
      }

    case .syncEvent(let line, let serial, let sessionID):
      applySyncEvent(line, serial: serial, sessionID: sessionID)

    case .syncRejected(let reason, _, _):
      phase = .error(Self.humanReadable(rejection: reason))

    case .resolvedTracks(let tracks, _):
      guard !pendingResolveTracks.isEmpty else { break }
      let slug = pendingResolveTracks.removeFirst()
      latestResolvedTracks = ResolvedTracksReply(slug: slug, tracks: tracks)
      resolvedTracksRevision += 1

    case .deviceInventorySnapshot(let snapshot):
      guard lastInventoryRevision.map({ snapshot.revision > $0 }) ?? true else { break }
      lastInventoryRevision = snapshot.revision
      hasAuthoritativeInventory = true
      devices = DeviceReducer.reduce(snapshot: snapshot, previous: devices)
      refreshFocusedDeviceProjection()
    }
  }

  /// Called once a surfaced `pendingPrompt` has been answered (its
  /// `decide_prompt` sent) so the same prompt isn't re-presented.
  func clearPendingPrompt() {
    pendingPrompt = nil
  }

  func willRequestDeviceConfig(
    serial: DeviceSerial, requestID: String, intent: DeviceConfigRequestIntent
  ) {
    registerDeviceRequest(
      serial: serial,
      requestID: requestID,
      configIntent: intent,
      requests: &deviceConfigRequests,
      latestGeneration: &latestDeviceConfigGeneration)
  }

  func willRequestDevicePreview(serial: DeviceSerial, requestID: String) {
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

  private var phaseIsSyncing: Bool {
    if case .syncing = phase { return true }
    return false
  }

  /// `noDevice`/`notConfigured` precedence used when deriving phase from
  /// connection/config state. Sync progress events (`sync_event` lines)
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

  private func applySyncEvent(_ line: String, serial: String?, sessionID: UInt64) {
    guard let data = line.data(using: .utf8),
      let event = try? decoder.decode(SyncEvent.self, from: data)
    else { return }

    if let serial {
      guard hasAuthoritativeInventory else { return }
      guard var state = devices[serial], state.sessionID == sessionID else { return }
      state = DeviceReducer.reduce(syncEvent: event, into: state)
      devices[serial] = state

      switch event {
      case .prompt(let id, let message, let options):
        pendingPrompt = PendingPrompt(id: id, message: message, options: options)
      case .form(let id, let label, let initial, let hint):
        pendingPrompt = PendingPrompt(
          id: id, message: hint ?? label, options: initial.map { [$0] } ?? [])
      default:
        break
      }

      refreshFocusedDeviceProjection()
      return
    }

    guard isScanning else { return }
    if let globalScanSessionID {
      guard globalScanSessionID == sessionID else { return }
    } else {
      globalScanSessionID = sessionID
    }
    applyLegacySyncEvent(event)
  }

  private func registerDeviceRequest(
    serial: DeviceSerial,
    requestID: String,
    configIntent: DeviceConfigRequestIntent?,
    requests: inout [String: PendingDeviceRequest],
    latestGeneration: inout [DeviceSerial: UInt64]
  ) {
    nextDeviceRequestGeneration += 1
    let generation = nextDeviceRequestGeneration
    requests[requestID] = PendingDeviceRequest(
      serial: serial, generation: generation, configIntent: configIntent)
    latestGeneration[serial] = generation
  }

  private func shouldApplyDeviceResponse(
    serial: DeviceSerial,
    requestID: String,
    requests: [String: PendingDeviceRequest],
    latestGeneration: [DeviceSerial: UInt64],
    allowsUnsolicitedResponse: Bool
  ) -> Bool {
    guard let request = requests[requestID] else { return allowsUnsolicitedResponse }
    return request.serial == serial && latestGeneration[serial] == request.generation
  }

  private func shouldApplyDeviceConfigResponse(serial: DeviceSerial, requestID: String) -> Bool {
    guard let request = deviceConfigRequests[requestID] else {
      invalidateLatestDeviceConfigRead(for: serial)
      return true
    }
    return request.serial == serial
      && latestDeviceConfigGeneration[serial] == request.generation
  }

  private func invalidateLatestDeviceConfigRead(for serial: DeviceSerial) {
    guard let latestGeneration = latestDeviceConfigGeneration[serial],
      deviceConfigRequests.values.contains(where: {
        $0.serial == serial && $0.generation == latestGeneration && $0.configIntent == .read
      })
    else { return }
    nextDeviceRequestGeneration += 1
    latestDeviceConfigGeneration[serial] = nextDeviceRequestGeneration
  }

  private func applyLegacySyncEvent(_ event: SyncEvent) {
    switch event {
    case .trackStart(let current, let total, let label, let etaSecs):
      if isScanning {
        phase = .scanning(current: current, total: total)
      } else {
        phase = .syncing(current: current, total: total, label: label, etaSecs: etaSecs)
      }
    case .finish(_, let skippedForSpace, let artwork, let dbRestored):
      if isScanning {
        // A scan's finish never carries these fields (they're
        // sync-only) — don't clobber the last real sync's rollup.
        phase = computePhase(targetSyncing: false)
      } else {
        lastRunSkippedForSpace = skippedForSpace
        lastRunArtwork = artwork
        lastRunDbRestored = dbRestored
        phase = .idle
        // Post-sync truth refresh: `deviceStorage` was statfs'd at
        // connect time and the preview's `projectedFreeBytes` was a
        // PRE-sync projection — without this, the capacity bar keeps
        // showing the old fill plus an orange "will use" overlay for
        // a sync that already happened.
        if let device {
          let storage = storageFor(drive: device.drive)
          deviceStorage = storage
          storageText = Self.formatStorage(storage)
          if var state = devices[device.serial] {
            state.preview?.projectedFreeBytes = nil
            state.config?.preview?.projectedFreeBytes = nil
            devices[device.serial] = state
          }
        }
      }
    case .prompt(let id, let message, let options):
      pendingPrompt = PendingPrompt(id: id, message: message, options: options)
    case .form(let id, let label, let initial, let hint):
      pendingPrompt = PendingPrompt(
        id: id, message: hint ?? label, options: initial.map { [$0] } ?? [])
    case .error(let message, _):
      phase = .error(message)
    case .paused:
      phase = .paused(synced: syncedCount, total: libraryCount)
    case .hello, .header, .summary, .trackDone, .finalizing, .cancelled, .log, .other:
      break
    }
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
      serial: serial,
      model: state.identity.modelLabel,
      name: state.identity.name,
      drive: state.mountPath ?? "")
    syncedCount = state.syncedCount
    libraryCount = state.libraryCount
    lastSync = state.latestSuccessfulSync
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
}

#if DEBUG
  extension AppModel {
    /// Preview-only seam for `deviceStorage`/`storageText`. Every other field
    /// SwiftUI previews need can be reached through a synthetic `DaemonEvent`
    /// fed to `apply(_:)` (see `PreviewFixtures.swift`), but a preview has no
    /// real iPod volume to resolve when exercising the mounted-volume fallback,
    /// and would otherwise show whichever disk happens to be at a given path
    /// on the machine running the canvas — not the deterministic, canned
    /// numbers a design review needs. `#if DEBUG`-gated (compiled out of
    /// Release entirely) rather than relaxing `deviceStorage`'s
    /// `private(set)` for the whole module.
    func seedPreviewStorage(free: Int64, total: Int64) {
      deviceStorage = (free, total)
      storageText = Self.formatStorage(deviceStorage)
    }
  }
#endif
