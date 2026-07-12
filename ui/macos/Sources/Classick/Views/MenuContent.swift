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
    var onCancelSync: () -> Void = { print("TODO: send(.cancelSync)") }
    var onRetry: () -> Void = { print("TODO: retry after error") }

    var body: some View {
        if let daemonFatalError {
            Text(daemonFatalError)
            Divider()
        }

        phaseContent

        Divider()
        Button("Settings…", action: onOpenSettings)
            .keyboardShortcut(",")
        Button("Quit Classick") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
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
            if let storageText = model.storageText {
                Text(storageText)
            }
            if let lastSync = model.lastSync {
                Text("Last sync: \(lastSync.timestamp)")
            }
            Divider()
            Button("Sync Now", action: onSyncNow)
                .keyboardShortcut("s")

        case let .syncing(current, total, label):
            Text("Syncing… \(current) of \(total)")
            if !label.isEmpty {
                Text(label)
            }
            Button("Cancel Sync", action: onCancelSync)

        case let .error(message):
            Text(message)
            Button("Retry", action: onRetry)
        }
    }
}
