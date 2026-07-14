import SwiftUI

/// The `MenuBarExtra` menu body, rendered from `model.phase`. Actions
/// ("Set Up Classick…", "Sync Now", etc.) are injected as closures so this
/// view stays a pure function of `AppModel` — later tasks (setup window,
/// settings window, live sync control) wire the closures to real commands
/// without touching this file's layout.
struct MenuContent: View {
    var model: AppModel
    var daemonFatalError: String?

    var onSetUp: () -> Void = { print("TODO: open setup window") }
    var onOpenSettings: () -> Void = { print("TODO: open settings window") }
    var onSyncNow: () -> Void = { print("TODO: send(.triggerSync(source: .manual))") }
    var onChooseMusic: () -> Void = { print("TODO: open Choose Music window") }
    var onCancelSync: () -> Void = { print("TODO: send(.cancelSync)") }
    var onPause: () -> Void = { print("TODO: send(.pause)") }
    var onResume: () -> Void = { print("TODO: send(.triggerSync(source: .manual))") }
    var onRetry: () -> Void = { print("TODO: retry after error") }
    var onCheckForUpdates: () -> Void = { print("TODO: check for updates") }

    var body: some View {
        if let daemonFatalError {
            Text(daemonFatalError)
            Divider()
        }

        phaseContent

        Divider()
        Button("Check for Updates…", action: onCheckForUpdates)
        Button("Settings…", action: onOpenSettings)
        Button("Quit Classick") { NSApplication.shared.terminate(nil) }
    }

    @ViewBuilder
    private var phaseContent: some View {
        switch model.phase {
        case .noDevice:
            Text("No iPod connected")
                .disabled(true)

        case .notConfigured:
            Button("Set Up Classick…", action: onSetUp)

        case .idle:
            if let device = model.device {
                Text(device.name ?? device.model)
            }
            if let syncedSummary {
                Text(syncedSummary)
            }
            if let storageText = model.storageText {
                Text(storageText)
            }
            if let lastSync = model.lastSync {
                Text("Last sync: \(formatLastSync(lastSync.timestamp))")
            }
            if model.selection.map({ $0.mode != .all }) ?? false {
                Text("Selection active — \(model.libraryCount ?? 0) tracks")
            }
            Divider()
            Button("Sync Now", action: onSyncNow)
            Button("Choose Music…", action: onChooseMusic)

        case let .syncing(current, total, label):
            Text("Syncing… \(current) of \(total)")
            if !label.isEmpty {
                Text(label)
            }
            Button("Pause", action: onPause)
            Button("Cancel Sync", action: onCancelSync)

        case let .scanning(current, total):
            Text("Scanning library… \(current) of \(total)")

        case let .paused(synced, total):
            Text("Paused — \(pausedSummary(synced: synced, total: total)) synced")
            Button("Resume", action: onResume)

        case let .error(message):
            Text(message)
            Button("Retry", action: onRetry)
        }
    }

    /// "X of Y synced" when the library size is known, else just "X synced".
    /// Hidden entirely when there's nothing to report yet (no counts seen).
    private var syncedSummary: String? {
        guard model.libraryCount != nil || model.syncedCount > 0 else { return nil }
        return pausedSummary(synced: model.syncedCount, total: model.libraryCount) + " synced"
    }

    private func pausedSummary(synced: Int, total: Int?) -> String {
        if let total {
            return "\(synced) of \(total)"
        }
        return "\(synced)"
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
