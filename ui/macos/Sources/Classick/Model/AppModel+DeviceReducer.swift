import Foundation

enum DeviceReducer {
  static func reduce(
    inventory: WireV3DeviceInventory,
    previous: [DeviceID: DeviceViewState]
  ) -> [DeviceID: DeviceViewState] {
    Dictionary(
      uniqueKeysWithValues: inventory.devices.map { wire in
        let prior = previous[wire.deviceID]
        let sameSession = prior?.sessionID == wire.sessionID
        let preserveFinishedRun = wire.sessionID == nil && prior?.lastRun != nil
        let identity = DeviceIdentityWire(
          serial: wire.deviceID.rawValue,
          modelLabel: wire.hardware.family?.value ?? prior?.identity.modelLabel ?? "iPod",
          name: wire.name)

        return (
          wire.deviceID,
          DeviceViewState(
            deviceID: wire.deviceID,
            identity: identity,
            readiness: wire.readiness,
            hardware: wire.hardware,
            profileStatus: wire.profileStatus,
            configDelivery: prior?.configDelivery,
            configured: wire.profileStatus == "adopted"
              || wire.profileStatus == "recovery_required",
            connected: wire.connected,
            mountPath: wire.mountPath,
            phase: phase(from: wire.phase, error: wire.lastTerminalError, previous: prior?.phase),
            sessionID: wire.sessionID,
            storage: wire.storage.map { StorageWire(free: $0.freeBytes, total: $0.totalBytes) },
            syncedCount: wire.syncedCount,
            libraryCount: wire.libraryCount,
            latestSuccessfulSync: prior?.latestSuccessfulSync,
            latestAttempt: prior?.latestAttempt,
            lastTerminalError: wire.lastTerminalError,
            config: prior?.config,
            preview: prior?.preview,
            selectionRevision: prior?.selectionRevision ?? 0,
            settingsRevision: prior?.settingsRevision ?? 0,
            subscriptionsRevision: prior?.subscriptionsRevision ?? 0,
            syncProgress: sameSession && wire.phase == "syncing" ? prior?.syncProgress : nil,
            finalization: sameSession ? prior?.finalization : nil,
            lastRun: sameSession || preserveFinishedRun ? prior?.lastRun : nil)
        )
      })
  }

  static func reduce(progress: WireV3ProgressEvent, into state: DeviceViewState) -> DeviceViewState
  {
    guard state.deviceID == progress.route.deviceID,
      state.sessionID == progress.route.sessionID
    else { return state }

    var state = state
    switch progress.kind {
    case .trackStart:
      guard let current = progress.current, let total = progress.total,
        let label = progress.label
      else { return state }
      state.syncProgress = DeviceSyncProgress(
        current: current, total: total, label: label, etaSecs: progress.etaSecs)
    case .finalizing:
      guard let reason = progress.finalizationReason,
        let stagedAlbums = progress.stagedAlbums,
        let stagedTracks = progress.stagedTracks
      else { return state }
      state.finalization = DeviceFinalization(
        reason: reason, stagedAlbums: stagedAlbums, stagedTracks: stagedTracks)
    case .syncFinished:
      guard let success = progress.success else { return state }
      state.lastRun = DeviceRunRollup(
        success: success,
        skippedForSpace: progress.skippedForSpace,
        artwork: progress.artwork,
        dbRestored: progress.dbRestored ?? false)
    case .runHeader, .syncSummary, .reviewRequested, .prompt, .form, .trackDone,
      .syncCancelled, .syncPaused, .syncLog, .syncError:
      break
    }
    return state
  }

  private static func phase(
    from wire: String, error: String?, previous: DevicePhase?
  ) -> DevicePhase {
    switch wire {
    case "disconnected": .disconnected
    case "unconfigured": .unconfigured
    case "idle": .idle
    case "syncing": .syncing
    case "paused": .paused
    case "error": .error(error ?? "Sync failed")
    default: previous ?? .disconnected
    }
  }
}

extension AppModel {
  var focusedDeviceSerial: DeviceID? {
    let active = devices.values.filter { $0.sessionID != nil }
    if active.count == 1 {
      return active[0].deviceID
    }
    if active.count > 1 {
      return nil
    }

    if case .device(let serial, _) = selectedDestination, devices[serial] != nil {
      return serial
    }

    let connected = devices.values.filter(\.connected)
    return connected.count == 1 ? connected[0].deviceID : nil
  }
}
