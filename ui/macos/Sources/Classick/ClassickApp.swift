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
    private let setupWindowController = SetupWindowController()

    /// First-run setup is auto-presented exactly once per launch, the moment
    /// the daemon confirms the user is unconfigured. This latches that so a
    /// later `config_update` (or reconnect churn) can't re-open it.
    private var didAutoPresentSetup = false
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
                self.autoPresentSetupIfNeeded()
            }
            self.daemonFatalError = await self.daemonClient.lastFatalError
        }
    }

    /// Shows first-run setup. Wired to the "Set Up Classick…" menu row and
    /// reused by `autoPresentSetupIfNeeded`.
    func presentSetup() {
        setupWindowController.show(model: model, onDone: finishSetup)
    }

    /// Requests a fresh library + selection + sync-history snapshot from the
    /// daemon. Used by `MainWindow`'s `.task` on first appear so the persistent
    /// `LibraryView` and History tab both have data on first view
    /// (`AppModel.history` is otherwise only ever populated by an unsolicited
    /// `history_update`).
    func requestLibraryAndSelection() {
        Task {
            await daemonClient.send(.getLibrary)
            await daemonClient.send(.getSelection)
            await daemonClient.send(.getHistory(limit: 50))
        }
    }

    /// "Rescan Library" action for the main window's `LibraryView`.
    func rescan() {
        Task { await daemonClient.send(.scanLibrary) }
    }

    /// Live preview of a candidate selection (mode + rules) as the user edits
    /// it in the main window's `LibraryView`.
    func previewSelection(mode: SelectionMode, rules: [SelectionRule]) {
        Task { await daemonClient.send(.previewSelection(mode: mode, rules: rules)) }
    }

    /// Persist a selection. The persistent `LibraryView` auto-saves on every
    /// debounced edit; the always-visible Sync Now button in the device row
    /// (rather than a modal "Sync now?" prompt on every save) is the
    /// affordance for applying a selection change to the connected iPod.
    func saveSelectionDirect(mode: SelectionMode, rules: [SelectionRule]) {
        Task { await daemonClient.send(.saveSelection(mode: mode, rules: rules)) }
    }

    /// Auto-presents setup the first time the daemon confirms (post-handshake)
    /// that the user has no configured music library. Gated on
    /// `needsFirstRunSetup`, which stays false until the `get_config` reply
    /// lands, so this can't fire during the startup race — and latched by
    /// `didAutoPresentSetup` so it happens at most once per launch.
    private func autoPresentSetupIfNeeded() {
        guard !didAutoPresentSetup, model.needsFirstRunSetup else { return }
        didAutoPresentSetup = true
        presentSetup()
    }

    /// Sends the setup window's `save_config` (folder + auto-sync + the
    /// currently-detected iPod, if any) and clears any error banner from a
    /// prior failed handshake/save.
    func finishSetup(source: String, autoSync: Bool) {
        let daemon = Self.setupDaemonSettings(
            autoSync: autoSync,
            preservingRockboxCompat: model.config?.daemon?.rockboxCompat ?? false)
        // Only preserve `customSelection` when the previously-persisted
        // identity is for the SAME serial that's connected now — a freshly
        // paired/swapped-in device has no prior per-device selection choice
        // to carry over, so it correctly starts at the shared-selection
        // default.
        let existingIpod = model.config?.ipod
        let preserveCustomSelection = existingIpod?.serial == model.device?.serial
            ? (existingIpod?.customSelection ?? false)
            : false
        let ipod = Self.setupIpodIdentity(device: model.device, preservingCustomSelection: preserveCustomSelection)
        Task { await daemonClient.send(.saveConfig(source: source, daemon: daemon, ipod: ipod)) }
    }

    /// The `IpodIdentity` the first-run/re-run wizard persists. SaveConfig
    /// replaces the whole `ipod` blob, so `customSelection` — like
    /// `rockboxCompat` above — must be threaded through from the caller
    /// rather than silently reset to `false`. Static + pure so the
    /// preservation is unit-testable (mirrors `setupDaemonSettings`).
    static func setupIpodIdentity(device: DeviceState?, preservingCustomSelection customSelection: Bool) -> IpodIdentity? {
        guard let device else { return nil }
        return IpodIdentity(serial: device.serial, modelLabel: device.model, name: device.name, customSelection: customSelection)
    }

    /// The `DaemonSettings` the first-run wizard persists. SaveConfig replaces
    /// the whole daemon blob, so any field not carried here gets reset to its
    /// default when setup re-runs — hence `rockboxCompat` is threaded through
    /// from the current config rather than silently flipped back off. Static +
    /// pure so the preservation is unit-testable.
    static func setupDaemonSettings(autoSync: Bool, preservingRockboxCompat rockboxCompat: Bool) -> DaemonSettings {
        DaemonSettings(
            enabled: autoSync,
            autostartWithWindows: false,
            firstSyncMode: "auto_apply",
            subsequentSyncMode: "auto_apply",
            scheduleMinutes: 0,
            notifyOn: "all",
            rockboxCompat: rockboxCompat)
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

    /// Device view's Selection picker (Task 17). SaveConfig replaces the
    /// whole `ipod` blob, so this must be built from the *current* persisted
    /// identity with only `customSelection` flipped — never a bare
    /// `IpodIdentity(serial:modelLabel:...)` construction, which would drop
    /// `name`/`model_label` and re-trigger the 0.2.1 wizard-clobber lesson
    /// `IpodIdentity.customSelection`'s doc comment warns about. No-ops if
    /// there's no persisted identity yet (nothing to flip).
    func saveIpodSelection(customSelection: Bool) {
        guard let ipod = Self.withCustomSelection(customSelection, from: model.config?.ipod) else { return }
        Task { await daemonClient.send(.saveConfig(source: nil, daemon: nil, ipod: ipod)) }
    }

    /// Pure identity-preserving update used by `saveIpodSelection`. Static so
    /// the preservation is unit-testable, mirroring `setupIpodIdentity` /
    /// `setupDaemonSettings` above.
    static func withCustomSelection(_ customSelection: Bool, from existing: IpodIdentity?) -> IpodIdentity? {
        guard let existing else { return nil }
        return IpodIdentity(serial: existing.serial, modelLabel: existing.modelLabel, name: existing.name, customSelection: customSelection)
    }

    /// "Replace Library…" confirmation sheet's confirm action (Task 17). The
    /// UI (`DeviceView`) is responsible for obtaining the user's typed
    /// confirmation before calling this — mirrors `replace_library`'s own
    /// contract on the wire (see `DaemonCommand.replaceLibrary`'s doc
    /// comment): the daemon does not prompt.
    func replaceLibrary() {
        Task { await daemonClient.send(.replaceLibrary) }
    }

    /// "Update existing library for Rockbox" button in Settings — asks the
    /// daemon to re-embed tags/art into already-synced tracks so an iPod
    /// running Rockbox (which doesn't read the iTunesDB) can display them.
    func backfillRockbox() {
        Task { await daemonClient.send(.backfillRockbox) }
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

    /// Pause / Resume menu actions. Pause requests a graceful drain +
    /// checkpoint on the daemon side; resume is just a normal sync trigger —
    /// the sync is diff-based, so it continues where it left off.
    func pause() {
        Task { await daemonClient.send(.pause) }
    }

    func resume() {
        Task { await daemonClient.send(.triggerSync(source: .manual)) }
    }

    /// "Retry" from the error phase. There's no dedicated retry command on
    /// the wire — a fresh `get_status` forces the daemon to push a current
    /// `status_update`, which recomputes phase out of `.error` if whatever
    /// caused it has cleared.
    func retry() {
        Task { await daemonClient.send(.getStatus) }
    }

    /// Sidebar's "+ New Playlist" flow (Task 3). The daemon replies with an
    /// unsolicited `playlists_update` (not a direct reply to this command),
    /// which is what `Sidebar`'s `destinationForNewlyCreatedPlaylist` flow
    /// watches for to pick up the newly assigned slug.
    func savePlaylist(_ payload: PlaylistPayload) {
        Task { await daemonClient.send(.savePlaylist(payload)) }
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
        case let .finish(success, _, _, _):
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

    /// Hybrid app: closing the main window leaves the app running in the Dock
    /// + menu bar so the daemon keeps syncing. Quit is explicit (⌘Q).
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    /// Re-open the main window when the Dock icon is clicked with no window
    /// visible. Returning `true` lets AppKit perform its default reopen
    /// behavior (restoring the last-closed `WindowGroup` window once Task 5
    /// adds one); `false` would suppress that and require reopening manually.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            NSApp.activate(ignoringOtherApps: true)
            // The WindowGroup restores its window on activation; if none exists,
            // openWindow (wired in Task 5) recreates it. AppKit reopens the
            // last closed WindowGroup window automatically here.
        }
        return true
    }
}

@main
struct ClassickApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    // `openSettings` is an ordinary `EnvironmentValue` readable from the `App`
    // conformer itself — the standard way to trigger the Settings scene from a
    // `MenuBarExtra` action. (Setup is NOT a SwiftUI scene; it's an
    // AppKit-hosted window owned by the delegate — see `SetupWindowController`.)
    @Environment(\.openSettings) private var openSettings
    @Environment(\.openWindow) private var openWindow

    var body: some Scene {
        Window("Classick", id: "main") {
            MainWindow(
                model: appDelegate.model,
                onSyncNow: appDelegate.syncNow,
                onPause: appDelegate.pause,
                onCancelSync: appDelegate.cancelSync,
                onResume: appDelegate.resume,
                onRetry: appDelegate.retry,
                onPreview: { mode, rules in appDelegate.previewSelection(mode: mode, rules: rules) },
                onSaveSelection: { mode, rules in appDelegate.saveSelectionDirect(mode: mode, rules: rules) },
                onScan: appDelegate.rescan,
                onSaveSettings: appDelegate.saveSettings,
                onForgetIpod: appDelegate.forgetIpod,
                onBackfill: appDelegate.backfillRockbox,
                onSetUp: appDelegate.presentSetup,
                onSaveIpodSelection: { custom in appDelegate.saveIpodSelection(customSelection: custom) },
                onReplaceLibrary: appDelegate.replaceLibrary,
                onAppearRequests: appDelegate.requestLibraryAndSelection,
                onSavePlaylist: { payload in appDelegate.savePlaylist(payload) }
            )
        }
        .windowResizability(.contentMinSize)

        MenuBarExtra("Classick", systemImage: menuBarSystemImage(for: appDelegate.model.phase)) {
            MenuContent(
                model: appDelegate.model,
                daemonFatalError: appDelegate.daemonFatalError,
                onSetUp: appDelegate.presentSetup,
                onOpenMain: openMainWindow,
                onOpenSettings: openSettingsWindow,
                onSyncNow: appDelegate.syncNow,
                onRescan: appDelegate.rescan,
                onCancelSync: appDelegate.cancelSync,
                onPause: appDelegate.pause,
                onResume: appDelegate.resume,
                onRetry: appDelegate.retry,
                onCheckForUpdates: appDelegate.checkForUpdates
            )
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView(
                model: appDelegate.model,
                onSave: appDelegate.saveSettings,
                onForgetIpod: appDelegate.forgetIpod,
                onBackfill: appDelegate.backfillRockbox
            )
        }
    }

    /// Activate before opening Settings so the window comes to the front
    /// rather than opening behind the current app.
    private func openSettingsWindow() {
        NSApp.activate(ignoringOtherApps: true)
        openSettings()
    }

    /// "Open Classick" menu action — brings the main singleton `Window` to
    /// the front, (re)creating it via `openWindow` if it was closed.
    private func openMainWindow() {
        NSApp.activate(ignoringOtherApps: true)
        openWindow(id: "main")
    }
}

private func menuBarSystemImage(for phase: Phase) -> String {
    switch phase {
    case .noDevice, .notConfigured, .idle:
        return "ipod"
    case .syncing:
        return "arrow.triangle.2.circlepath"
    case .scanning:
        return "magnifyingglass"
    case .paused:
        return "pause.circle"
    case .error:
        return "exclamationmark.triangle"
    }
}
