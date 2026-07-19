import Foundation

enum DeviceReducer {
  static func reduce(
    snapshot: DeviceInventorySnapshot,
    previous: [DeviceSerial: DeviceViewState]
  ) -> [DeviceSerial: DeviceViewState] {
    Dictionary(
      uniqueKeysWithValues: snapshot.devices.map { wire in
        let serial = wire.identity.serial
        let prior = previous[serial]
        let sameSession = prior?.sessionID == wire.sessionID
        let preserveFinishedRun = wire.sessionID == nil && prior?.lastRun != nil

        return (
          serial,
          DeviceViewState(
            identity: wire.identity,
            configured: wire.configured,
            connected: wire.connected,
            mountPath: wire.mount,
            phase: phase(from: wire),
            sessionID: wire.sessionID,
            storage: wire.storage,
            syncedCount: wire.syncedCount,
            libraryCount: wire.libraryCount,
            latestSuccessfulSync: wire.latestSuccessfulSync,
            latestAttempt: wire.latestAttempt,
            lastTerminalError: wire.lastTerminalError,
            config: prior?.config,
            preview: prior?.preview,
            selectionRevision: wire.selectionRevision,
            settingsRevision: wire.settingsRevision,
            subscriptionsRevision: wire.subscriptionsRevision,
            syncProgress: sameSession && wire.phase == .syncing ? prior?.syncProgress : nil,
            finalization: sameSession ? prior?.finalization : nil,
            lastRun: sameSession || preserveFinishedRun ? prior?.lastRun : nil)
        )
      })
  }

  static func reduce(syncEvent: SyncEvent, into state: DeviceViewState) -> DeviceViewState {
    var state = state
    switch syncEvent {
    case .trackStart(let current, let total, let label, let etaSecs):
      state.syncProgress = DeviceSyncProgress(
        current: current, total: total, label: label, etaSecs: etaSecs)
    case .finalizing(let reason, let stagedAlbums, let stagedTracks):
      state.finalization = DeviceFinalization(
        reason: reason, stagedAlbums: stagedAlbums, stagedTracks: stagedTracks)
    case .finish(let success, let skippedForSpace, let artwork, let dbRestored):
      state.lastRun = DeviceRunRollup(
        success: success,
        skippedForSpace: skippedForSpace,
        artwork: artwork,
        dbRestored: dbRestored)
    case .hello, .header, .summary, .trackDone, .cancelled, .log, .prompt, .form, .error, .paused,
      .other:
      break
    }
    return state
  }

  private static func phase(from wire: DeviceSnapshotWire) -> DevicePhase {
    switch wire.phase {
    case .disconnected: .disconnected
    case .unconfigured: .unconfigured
    case .idle: .idle
    case .syncing: .syncing
    case .paused: .paused
    case .error: .error(wire.lastTerminalError ?? "Sync failed")
    }
  }
}

extension AppModel {
  var focusedDeviceSerial: DeviceSerial? {
    let active = devices.values.filter { $0.sessionID != nil }
    if active.count == 1 {
      return active[0].identity.serial
    }
    if active.count > 1 {
      return nil
    }

    if case .device(let serial, _) = selectedDestination, devices[serial] != nil {
      return serial
    }

    let connected = devices.values.filter(\.connected)
    return connected.count == 1 ? connected[0].identity.serial : nil
  }
}
