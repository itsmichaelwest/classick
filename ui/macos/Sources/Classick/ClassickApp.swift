import AppKit
import SwiftUI
import os

/// Owns the pieces that must persist for the whole app lifetime and outlive
/// any individual SwiftUI scene: the daemon connection, the daemon
/// subprocess, and the reducer that turns daemon events into UI state.
///
/// Driving daemon startup/shutdown from app-lifecycle callbacks (rather than
/// a `.task`/`.onDisappear` on a view inside the `MenuBarExtra`) sidesteps a
/// footgun with `.menuBarExtraStyle(.menu)`: that content view is only
/// materialized when the user actually opens the menu, so a `.task` there
/// would delay connecting to the daemon until the first click.
// `ObservableObject` (not `@Observable`) because `NSApplicationDelegateAdaptor`
// only re-renders dependent scenes on Combine-style publishes — that's the
// documented mechanism for making delegate state observable to SwiftUI.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    private let logger = Logger(subsystem: "com.classick.app", category: "AppDelegate")

    let model = AppModel()
    let daemonClient = DaemonClient()
    private let daemonProcess = DaemonProcess()
    #if canImport(Sparkle)
    private let updater = Updater()
    #endif
    private var eventTask: Task<Void, Never>?

    // Set by the most recent inner `summary` line of the sync in progress, so
    // that when the matching `finish` line arrives we know how many tracks
    // were added. Decoded straight from `sync_event.line` here (rather than
    // exposing it on AppModel) since notifications are a side effect of the
    // event stream, not UI state the reducer needs to track.
    private var pendingSyncAddCount = 0
    private let syncEventDecoder = JSONDecoder()

    /// Mirrors `daemonClient.lastFatalError` on the main actor. The actor's
    /// own property can't be read synchronously from a SwiftUI `body`, so
    /// this copy is refreshed whenever the event stream ends (which is
    /// exactly when a fatal handshake error, if any, was set).
    @Published private(set) var daemonFatalError: String?

    func applicationDidFinishLaunching(_ notification: Notification) {
        Notifier.requestAuth()
        daemonProcess.ensureRunning()

        eventTask = Task {
            // Register the event stream's continuation before starting the
            // connect/reconnect loop, so nothing yielded during the initial
            // handshake is dropped (mirrors DaemonClientTests' call order).
            let stream = await self.daemonClient.events()
            await self.daemonClient.start()
            for await event in stream {
                self.model.apply(event)
                self.observeForNotification(event)
                self.presentPromptIfNeeded()
            }
            self.daemonFatalError = await self.daemonClient.lastFatalError
        }
    }

    /// Sends the setup window's `save_config` (folder + auto-sync + the
    /// currently-detected iPod, if any) and clears any error banner from a
    /// prior failed handshake/save.
    func finishSetup(source: String, autoSync: Bool) {
        let daemon = DaemonSettings(
            enabled: autoSync,
            autostartWithWindows: false,
            firstSyncMode: "auto_apply",
            subsequentSyncMode: "auto_apply",
            scheduleMinutes: 0,
            notifyOn: "all")
        let ipod = model.device.map { IpodIdentity(serial: $0.serial, modelLabel: $0.model, name: $0.name) }
        Task { await daemonClient.send(.saveConfig(source: source, daemon: daemon, ipod: ipod)) }
    }

    /// Settings window's debounced edits. `ipod` is omitted (nil) so an
    /// existing iPod pairing isn't disturbed by unrelated setting changes —
    /// only "Remove this iPod" (`forgetIpod()`) touches that.
    func saveSettings(source: String?, daemon: DaemonSettings) {
        Task { await daemonClient.send(.saveConfig(source: source, daemon: daemon, ipod: nil)) }
    }

    func forgetIpod() {
        Task { await daemonClient.send(.forgetIpod) }
    }

    /// Surfaces `model.pendingPrompt` (set by the reducer from a relayed
    /// `sync_event` prompt/form line) as a blocking `NSAlert`, then replies
    /// with the chosen option and clears it so it isn't re-shown.
    private func presentPromptIfNeeded() {
        guard let prompt = model.pendingPrompt else { return }
        let choice = PromptAlert.present(prompt)
        model.clearPendingPrompt()
        Task { await daemonClient.send(.decidePrompt(id: prompt.id, choice: choice)) }
    }

    /// Sync Now / Cancel Sync menu actions. `DaemonClient.send` is async
    /// (actor-isolated socket write); the menu's `Button` actions are sync
    /// closures, so each hop through here spawns a detached-from-the-caller
    /// `Task` to bridge to the actor.
    func syncNow() {
        Task { await daemonClient.send(.triggerSync(source: .manual)) }
    }

    func cancelSync() {
        Task { await daemonClient.send(.cancelSync) }
    }

    /// "Retry" from the error phase. There's no dedicated retry command on
    /// the wire — a fresh `get_status` forces the daemon to push a current
    /// `status_update`, which recomputes phase out of `.error` if whatever
    /// caused it has cleared.
    func retry() {
        Task { await daemonClient.send(.getStatus) }
    }

    /// No-op under `swift test` (see `Updater.swift`) — SPM's `Package.swift`
    /// graph doesn't carry the Sparkle dependency.
    func checkForUpdates() {
        #if canImport(Sparkle)
        updater.checkForUpdates()
        #endif
    }

    /// Peeks at `sync_event` lines for the `summary`/`finish` pair so a
    /// completion notification can report how many tracks were added.
    /// AppModel's reducer already handles these lines for UI state; this is
    /// a side channel over the same wire data, not a change to the reducer.
    private func observeForNotification(_ event: DaemonEvent) {
        guard case let .syncEvent(line) = event,
              let data = line.data(using: .utf8),
              let inner = try? syncEventDecoder.decode(SyncEvent.self, from: data) else { return }
        switch inner {
        case .header:
            // A new sync just started streaming — clear out whatever count
            // is left over from the previous run so a `finish` that (for
            // whatever reason) arrives without an intervening `summary`
            // can't report a stale, previous-run count.
            pendingSyncAddCount = 0
        case let .summary(add, _, _, _, _, _):
            pendingSyncAddCount = add
        case let .finish(success):
            // Honor the user's notification-level preference (notify_on):
            // "all" fires always, "errors_only" only on failure, "none" never.
            if Notifier.shouldPostSyncFinished(
                notifyOn: model.config?.daemon?.notifyOn, success: success) {
                Notifier.syncFinished(success: success, added: pendingSyncAddCount)
            }
            pendingSyncAddCount = 0
        default:
            break
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        eventTask?.cancel()
        // Best-effort: actor-isolated socket teardown can't be awaited from
        // this synchronous delegate callback without risking a delay to
        // process termination, so this is fire-and-forget.
        Task { await daemonClient.stop() }
        daemonProcess.stop()
    }
}

@main
struct ClassickApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    // `openWindow`/`openSettings` are ordinary `EnvironmentValues` and are
    // readable from the `App` conformer itself (not just from `View` bodies)
    // — the standard way to trigger scenes from `MenuBarExtra` actions.
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some Scene {
        MenuBarExtra("Classick", systemImage: menuBarSystemImage(for: appDelegate.model.phase)) {
            MenuContent(
                model: appDelegate.model,
                daemonFatalError: appDelegate.daemonFatalError,
                onSetUp: openSetupWindow,
                onOpenSettings: openSettingsWindow,
                onSyncNow: appDelegate.syncNow,
                onCancelSync: appDelegate.cancelSync,
                onRetry: appDelegate.retry,
                onCheckForUpdates: appDelegate.checkForUpdates
            )
        }
        .menuBarExtraStyle(.menu)

        // A regular, single-instance window (not a MenuBarExtra submenu) so
        // `.fileImporter` and normal window chrome work as expected.
        Window("Set Up Classick", id: "setup") {
            SetupWindow(model: appDelegate.model, onDone: appDelegate.finishSetup)
        }
        .windowResizability(.contentSize)

        Settings {
            SettingsView(
                model: appDelegate.model,
                onSave: appDelegate.saveSettings,
                onForgetIpod: appDelegate.forgetIpod
            )
        }
    }

    /// Opening any window from this `LSUIElement` (accessory, no Dock icon)
    /// app requires activating it first — otherwise the window can open
    /// behind whatever app currently has focus.
    private func openSetupWindow() {
        NSApp.activate(ignoringOtherApps: true)
        openWindow(id: "setup")
    }

    private func openSettingsWindow() {
        NSApp.activate(ignoringOtherApps: true)
        openSettings()
    }
}

private func menuBarSystemImage(for phase: Phase) -> String {
    switch phase {
    case .noDevice, .notConfigured, .idle:
        return "ipod"
    case .syncing:
        return "arrow.triangle.2.circlepath"
    case .error:
        return "exclamationmark.triangle"
    }
}
