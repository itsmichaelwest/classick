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
            }
            self.daemonFatalError = await self.daemonClient.lastFatalError
        }
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

    /// Peeks at `sync_event` lines for the `summary`/`finish` pair so a
    /// completion notification can report how many tracks were added.
    /// AppModel's reducer already handles these lines for UI state; this is
    /// a side channel over the same wire data, not a change to the reducer.
    private func observeForNotification(_ event: DaemonEvent) {
        guard case let .syncEvent(line) = event,
              let data = line.data(using: .utf8),
              let inner = try? syncEventDecoder.decode(SyncEvent.self, from: data) else { return }
        switch inner {
        case let .summary(add, _, _, _, _, _):
            pendingSyncAddCount = add
        case let .finish(success):
            Notifier.syncFinished(success: success, added: pendingSyncAddCount)
            pendingSyncAddCount = 0
        default:
            break
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        eventTask?.cancel()
        daemonProcess.stop()
    }
}

@main
struct ClassickApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra("Classick", systemImage: menuBarSystemImage(for: appDelegate.model.phase)) {
            MenuContent(
                model: appDelegate.model,
                daemonFatalError: appDelegate.daemonFatalError,
                onSyncNow: appDelegate.syncNow,
                onCancelSync: appDelegate.cancelSync
            )
        }
        .menuBarExtraStyle(.menu)
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
