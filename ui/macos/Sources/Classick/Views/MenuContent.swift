import SwiftUI

/// The `MenuBarExtra` menu body, rendered from `model.phase`. Actions
/// ("Set Up Classick…", "Sync Now", etc.) are injected as closures so this
/// view stays a pure function of `AppModel` — later tasks (setup window,
/// settings window, live sync control) wire the closures to real commands
/// without touching this file's layout.
struct MenuContent: View {
  var model: AppModel
  var daemonFatalError: String?

  var onSetUp: (DeviceID) -> Void = { _ in print("TODO: open setup window") }
  var onOpenMain: () -> Void = { print("TODO: open main window") }
  var onOpenSettings: () -> Void = { print("TODO: open settings window") }
  var onSyncNow: (DeviceID) -> Void = { _ in print("TODO: send(.triggerSync(source: .manual))")
  }
  var onRescan: () -> Void = { print("TODO: send(.scanLibrary)") }
  var onConnectSource: () -> Void = { print("TODO: send(.retrySourceMount)") }
  var onCancelSync: (DeviceID) -> Void = { _ in print("TODO: send(.cancelSync)") }
  var onPause: (DeviceID) -> Void = { _ in print("TODO: send(.pause)") }
  var onResume: (DeviceID) -> Void = { _ in print("TODO: send(.triggerSync(source: .manual))") }
  var onRetry: (DeviceID) -> Void = { _ in print("TODO: retry after error") }
  var onCheckForUpdates: () -> Void = { print("TODO: check for updates") }

  var body: some View {
    if let daemonFatalError {
      Text(daemonFatalError)
      Divider()
    }

    Button("Open Classick", action: onOpenMain)
    if model.sourceNeedsAttention {
      Divider()
      Text(SourceRecoveryPresentation.attentionTitle)
      Button("Connect", action: onConnectSource)
        .disabled(model.sourceRetryPending)
    }
    Divider()
    phaseContent
    Divider()
    Button("Rescan Library", action: onRescan)
    Button("Settings…", action: onOpenSettings)
    Button("Check for Updates…", action: onCheckForUpdates)
    Button("Quit Classick") { NSApplication.shared.terminate(nil) }
  }

  @ViewBuilder
  private var phaseContent: some View {
    if case .scanning(let current, let total) = model.phase {
      Text("Scanning library… \(current) of \(total)")
    } else if let serial = MenuContentLogic.actionTarget(
      focusedSerial: model.focusedDeviceSerial, devices: model.devices),
      let state = model.devices[serial]
    {
      deviceContent(serial: serial, state: state)
    } else if model.devices.values.filter(\.connected).count > 1 {
      Text("Select an iPod in Classick")
        .disabled(true)
    } else {
      Text("No iPod connected")
        .disabled(true)
    }
  }

  @ViewBuilder
  private func deviceContent(serial: DeviceID, state: DeviceViewState) -> some View {
    switch DeviceSurfaceLogic.phase(for: state, globalPhase: model.phase) {
    case .noDevice:
      Text("No iPod connected")
        .disabled(true)

    case .notConfigured:
      Button("Set Up Classick…") { onSetUp(serial) }

    case .idle:
      Text(state.identity.name ?? state.identity.modelLabel)
      if let storageText = DeviceSurfaceLogic.storageText(state) {
        Text(storageText)
      }
      if let lastSync = model.latestSuccessfulSync(for: serial) {
        Text("Last sync: \(formatLastSync(lastSync.timestamp))")
      }
      Divider()
      Button("Sync Now") { onSyncNow(serial) }

    case .syncing(let current, let total, let label, _):
      Text("Syncing… \(current) of \(total)")
      if !label.isEmpty {
        Text(label)
      }
      Button("Pause") { onPause(serial) }
      Button("Cancel Sync") { onCancelSync(serial) }

    case .scanning(let current, let total):
      Text("Scanning library… \(current) of \(total)")

    case .paused(let synced, let total):
      Text("Paused — \(pausedSummary(synced: synced, total: total)) synced")
      Button("Resume") { onResume(serial) }

    case .error(let message):
      Text(message)
      Button("Retry") { onRetry(serial) }
    }
  }

  private func pausedSummary(synced: Int, total: Int?) -> String {
    if let total {
      return "\(synced) of \(total)"
    }
    return "\(synced)"
  }
}

enum MenuContentLogic {
  static func actionTarget(
    focusedSerial: DeviceID?, devices: [DeviceID: DeviceViewState]
  ) -> DeviceID? {
    guard let focusedSerial, let state = devices[focusedSerial], state.connected else {
      return nil
    }
    return focusedSerial
  }
}

/// The daemon sends `HistoryEntry.timestamp` as an ISO-8601/RFC-3339 string
/// (e.g. "2026-05-24T10:00:00Z"). Render it in the user's locale/timezone
/// instead of dumping the raw UTC string into the menu. Falls back to the raw
/// value if it somehow doesn't parse.
private func formatLastSync(_ iso: String) -> String {
  let parser = ISO8601DateFormatter()
  guard let date = parser.date(from: iso) else { return iso }
  return date.formatted(date: .abbreviated, time: .shortened)
}

#if DEBUG
  #Preview("Idle") {
    MenuContent(model: PreviewFixtures.connectedSyncedModel())
      .frame(width: 280)
  }

  #Preview("Syncing") {
    MenuContent(model: PreviewFixtures.syncingModel())
      .frame(width: 280)
  }

  #Preview("Paused") {
    MenuContent(model: PreviewFixtures.pausedModel())
      .frame(width: 280)
  }

  #Preview("No device") {
    MenuContent(model: PreviewFixtures.noDeviceModel())
      .frame(width: 280)
  }

  #Preview("Not configured") {
    MenuContent(model: PreviewFixtures.notConfiguredModel())
      .frame(width: 280)
  }

  #Preview("Error") {
    MenuContent(model: PreviewFixtures.errorModel())
      .frame(width: 280)
  }

  #Preview("Music share needs attention") {
    MenuContent(model: PreviewFixtures.sourceAttentionModel())
      .frame(width: 280)
  }
#endif
