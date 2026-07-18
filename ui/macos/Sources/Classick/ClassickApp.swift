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
  /// Whether the subprocess stream currently in flight is a library scan
  /// (latched at `header` — see `observeForNotification`).
  private var currentStreamIsScan = false

  private let syncEventDecoder = JSONDecoder()

  private var targetSerial: String? {
    model.device?.serial ?? model.config?.ipod?.serial
  }

  /// Mirrors `daemonClient.lastFatalError` on the main actor. The actor's
  /// own property can't be read synchronously from a SwiftUI `body`, so
  /// this copy is refreshed whenever the event stream ends (which is
  /// exactly when a fatal handshake error, if any, was set).
  @Published private(set) var daemonFatalError: String?

  func applicationDidFinishLaunching(_ notification: Notification) {
    // Xcode renders an app target's previews by launching the app as the
    // preview host. Fixtures need none of the launch side effects.
    guard !ProcessInfo.isRunningInXcodePreviews else { return }

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
        // `hello` marks a completed (re)handshake. The initial
        // window-appear request batch races the first connect —
        // `DaemonClient.send` DROPS commands while disconnected —
        // so without this, `get_library`/`get_history`/
        // `list_playlists` are silently lost at launch and the
        // Library page sits on "needs scan" until the next
        // ScanCompleted broadcast, even though the daemon serves
        // the cached index from disk on request. Re-issuing here
        // covers both launch and every daemon reconnect; the
        // requests are idempotent reads, so overlapping with the
        // window-appear batch is harmless.
        if case .hello = event {
          self.requestLibraryAndSelection()
        }
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
      await daemonClient.send(.getLibrary(requestID: DaemonCommand.newRequestID()))
      await daemonClient.send(.getHistory(limit: 50, requestID: DaemonCommand.newRequestID()))
      // Protocol 1.6.0: the sidebar's Playlists section and the device
      // Music page's subscriptions checklist both read `model.playlists`,
      // populated only by a `playlists_update` reply/broadcast — nothing
      // requests the initial list otherwise.
      await daemonClient.send(.listPlaylists(requestID: DaemonCommand.newRequestID()))
    }
  }

  /// "Rescan Library" action for the main window's `LibraryView`.
  func rescan() {
    Task { await daemonClient.send(.scanLibrary(requestID: DaemonCommand.newRequestID())) }
  }

  /// TRUE disk eject for the sidebar's eject glyph: unmounts the iPod's
  /// volume via `NSWorkspace` (the native "safe to unplug" operation) —
  /// deliberately NOT `forgetIpod`, which unpairs the device from Classick
  /// and lives on the device Settings page. The daemon holds no volume
  /// handles while idle (the sync subprocess only runs during a sync, and
  /// the sidebar disables eject mid-sync), so the unmount normally
  /// succeeds; when it doesn't (Finder/another app holding files open),
  /// the system's error is surfaced in an alert rather than swallowed.
  func ejectIpod() {
    guard let drive = model.device?.drive else { return }
    let url = URL(fileURLWithPath: drive)
    do {
      try NSWorkspace.shared.unmountAndEjectDevice(at: url)
    } catch {
      let alert = NSAlert()
      alert.messageText = "Couldn't Eject iPod"
      alert.informativeText = error.localizedDescription
      alert.alertStyle = .warning
      alert.runModal()
    }
  }

  /// Live preview of a candidate selection (mode + rules) as the user edits
  /// it in the main window's `LibraryView`.
  func previewSelection(mode: SelectionMode, rules: [SelectionRule]) {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .previewSelection(
          mode: mode,
          rules: rules,
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Persist a selection. The persistent `LibraryView` auto-saves on every
  /// debounced edit; the always-visible Sync Now button in the device row
  /// (rather than a modal "Sync now?" prompt on every save) is the
  /// affordance for applying a selection change to the connected iPod.
  func saveSelectionDirect(mode: SelectionMode, rules: [SelectionRule]) {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .saveDeviceConfig(
          serial: serial,
          selection: SelectionState(mode: mode, rules: rules),
          subscriptions: nil,
          settings: nil,
          requestID: DaemonCommand.newRequestID()))
    }
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
    let preserveCustomSelection =
      existingIpod?.serial == model.device?.serial
      ? (existingIpod?.customSelection ?? false)
      : false
    let ipod = Self.setupIpodIdentity(
      device: model.device, preservingCustomSelection: preserveCustomSelection)
    Task {
      await daemonClient.send(
        .saveConfig(
          source: source,
          daemon: daemon,
          ipod: ipod,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Settings window's debounced edits. `ipod` is omitted (nil) so an
  /// existing iPod pairing isn't disturbed by unrelated setting changes —
  /// only "Remove this iPod" (`forgetIpod()`) touches that.
  func saveSettings(source: String?, daemon: DaemonSettings) {
    Task {
      await daemonClient.send(
        .saveConfig(
          source: source,
          daemon: daemon,
          ipod: nil,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  func forgetIpod() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .forgetIpod(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// "Replace Library…" confirmation sheet's confirm action. The UI
  /// (`DeviceSettingsPage`) is responsible for obtaining the user's typed
  /// confirmation before calling this — mirrors `replace_library`'s own
  /// contract on the wire (see `DaemonCommand.replaceLibrary`'s doc
  /// comment): the daemon does not prompt.
  func replaceLibrary() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .replaceLibrary(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// "Update existing library for Rockbox" button in Settings — asks the
  /// daemon to re-embed tags/art into already-synced tracks so an iPod
  /// running Rockbox (which doesn't read the iTunesDB) can display them.
  func backfillRockbox() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .backfillRockbox(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Surfaces `model.pendingPrompt` (set by the reducer from a relayed
  /// `sync_event` prompt/form line) as a blocking `NSAlert`, then replies
  /// with the chosen option and clears it so it isn't re-shown.
  private func presentPromptIfNeeded() {
    guard let prompt = model.pendingPrompt else { return }
    guard let serial = targetSerial else { return }
    let choice = PromptAlert.present(prompt)
    model.clearPendingPrompt()
    Task {
      await daemonClient.send(
        .decidePrompt(
          id: prompt.id,
          choice: choice,
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Sync Now / Cancel Sync menu actions. `DaemonClient.send` is async
  /// (actor-isolated socket write); the menu's `Button` actions are sync
  /// closures, so each hop through here spawns a detached-from-the-caller
  /// `Task` to bridge to the actor.
  func syncNow() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .triggerSync(
          source: .manual,
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  func cancelSync() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .cancelSync(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Pause / Resume menu actions. Pause requests a graceful drain +
  /// checkpoint on the daemon side; resume is just a normal sync trigger —
  /// the sync is diff-based, so it continues where it left off.
  func pause() {
    guard let serial = targetSerial else { return }
    Task {
      await daemonClient.send(
        .pause(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  func resume() {
    syncNow()
  }

  /// "Retry" from the error phase. There's no dedicated retry command on
  /// the wire — a fresh `get_status` forces the daemon to push a current
  /// `status_update`, which recomputes phase out of `.error` if whatever
  /// caused it has cleared.
  func retry() {
    Task { await daemonClient.send(.getStatus(requestID: DaemonCommand.newRequestID())) }
  }

  /// Sidebar's "+ New Playlist" flow (Task 3). The daemon replies with an
  /// unsolicited `playlists_update` (not a direct reply to this command),
  /// which is what `Sidebar`'s `destinationForNewlyCreatedPlaylist` flow
  /// watches for to pick up the newly assigned slug.
  func savePlaylist(_ payload: PlaylistPayload) {
    Task {
      await daemonClient.send(
        .savePlaylist(
          payload,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Playlist editor pages (Task 7): fetches one playlist's full content
  /// the moment the user navigates to its page — `model.playlistDetail`
  /// is otherwise only ever populated by an unsolicited reply.
  func getPlaylist(slug: String) {
    Task {
      await daemonClient.send(
        .getPlaylist(
          slug: slug,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Toolbar menu's "Delete Playlist" (with confirmation, handled by the
  /// caller). The daemon replies via an unsolicited `playlists_update`
  /// broadcast, same as `savePlaylist`.
  func deletePlaylist(slug: String) {
    Task {
      await daemonClient.send(
        .deletePlaylist(
          slug: slug,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Add Songs picker (Task 7, protocol 1.7.0): expands checked
  /// artist/album/genre rules into real track paths server-side — see
  /// `AppModel.willRequestResolveTracks`'s doc comment for why this
  /// bookkeeping precedes the send (mirrors `previewDevice`). `slug` is the
  /// requesting playlist editor's own slug (no wire change — purely
  /// client-side correlation bookkeeping, see `ResolvedTracksReply`).
  func resolveTracks(slug: String, rules: [SelectionRule]) {
    model.willRequestResolveTracks(slug: slug)
    Task {
      await daemonClient.send(
        .resolveTracks(
          rules: rules,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Device Music page (Task 5): fetches the device's current selection +
  /// subscriptions + settings, plus a fresh capacity preview, the moment
  /// the user navigates to its Music page. Device config isn't part of
  /// `requestLibraryAndSelection`'s window-appear batch since it's scoped
  /// to whichever device page is showing, not global app state.
  func loadDeviceConfig(serial: String) {
    Task {
      await daemonClient.send(
        .getDeviceConfig(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
    previewDevice(serial: serial)
  }

  /// Live capacity/skip preview for a candidate device selection/
  /// subscription edit. `device_preview` carries no serial on the wire, so
  /// `willRequestDevicePreview` bookkeeping (mirrors `previewSelection`
  /// above) lets the reducer attach the reply to the right device.
  func previewDevice(serial: String) {
    model.willRequestDevicePreview(serial: serial)
    Task {
      await daemonClient.send(
        .previewDevice(
          serial: serial,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Device Music page's debounced auto-save (Task 5). `settings` is
  /// always omitted (nil = "don't change") — this page only ever edits
  /// `selection`/`subscriptions`; per-device settings are the device
  /// Settings page's job (Task 6).
  func saveDeviceConfig(
    serial: String, selection: SelectionState?, subscriptions: SubscriptionsWire?
  ) {
    Task {
      await daemonClient.send(
        .saveDeviceConfig(
          serial: serial,
          selection: selection,
          subscriptions: subscriptions,
          settings: nil,
          requestID: DaemonCommand.newRequestID()))
    }
  }

  /// Device Settings page's debounced auto-save (Task 6): the mirror image
  /// of `saveDeviceConfig` above — `selection`/`subscriptions` are always
  /// omitted (nil = "don't change") via `DeviceSettingsLogic.saveSettingsCommand`,
  /// so a toggle edit here can never disturb the Music page's sync intent.
  func saveDeviceSettings(serial: String, settings: DeviceSettingsWire) {
    Task {
      await daemonClient.send(
        DeviceSettingsLogic.saveSettingsCommand(
          serial: serial,
          settings: settings,
          requestID: DaemonCommand.newRequestID()))
    }
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
    guard case .syncEvent(let line, _, _) = event,
      let data = line.data(using: .utf8),
      let inner = try? syncEventDecoder.decode(SyncEvent.self, from: data)
    else { return }
    switch inner {
    case .header:
      // A new sync just started streaming — clear out whatever count
      // is left over from the previous run so a `finish` that (for
      // whatever reason) arrives without an intervening `summary`
      // can't report a stale, previous-run count.
      pendingSyncAddCount = 0
      // Latch "this stream is a scan" at stream START, when the
      // daemon's scanning status_update has definitively preceded
      // the subprocess's first line — reading `isScanning` at
      // `finish` time instead raced the trailing idle status_update
      // (sweep finding #7: if idle landed first, a scan fired a
      // bogus "Sync complete" banner).
      currentStreamIsScan = model.isScanning
    case .summary(let add, _, _, _, _, _):
      pendingSyncAddCount = add
    case .finish(let success, _, _, _):
      // Honor the user's notification-level preference (notify_on):
      // "all" fires always, "errors_only" only on failure, "none" never.
      if Notifier.shouldPostSyncFinished(
        notifyOn: model.config?.daemon?.notifyOn, success: success,
        isScanning: currentStreamIsScan)
      {
        Notifier.syncFinished(success: success, added: pendingSyncAddCount)
      }
      pendingSyncAddCount = 0
      currentStreamIsScan = false
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
  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool
  {
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
        onSaveSelection: { mode, rules in appDelegate.saveSelectionDirect(mode: mode, rules: rules)
        },
        onScan: appDelegate.rescan,
        onForgetIpod: appDelegate.forgetIpod,
        onEjectIpod: appDelegate.ejectIpod,
        onBackfill: appDelegate.backfillRockbox,
        onSetUp: appDelegate.presentSetup,
        onReplaceLibrary: appDelegate.replaceLibrary,
        onAppearRequests: appDelegate.requestLibraryAndSelection,
        onSavePlaylist: { payload in appDelegate.savePlaylist(payload) },
        onGetPlaylist: { slug in appDelegate.getPlaylist(slug: slug) },
        onDeletePlaylist: { slug in appDelegate.deletePlaylist(slug: slug) },
        onResolveTracks: { slug, rules in appDelegate.resolveTracks(slug: slug, rules: rules) },
        onLoadDeviceConfig: { serial in appDelegate.loadDeviceConfig(serial: serial) },
        onPreviewDevice: { serial in appDelegate.previewDevice(serial: serial) },
        onSaveDeviceConfig: { serial, selection, subscriptions in
          appDelegate.saveDeviceConfig(
            serial: serial, selection: selection, subscriptions: subscriptions)
        },
        onSaveDeviceSettings: { serial, settings in
          appDelegate.saveDeviceSettings(serial: serial, settings: settings)
        }
      )
    }
    .windowResizability(.contentMinSize)

    // `isInserted: false` in previews — the preview host runs this App
    // body for real, and without the gate every canvas refresh planted
    // a Classick item in the developer's actual menu bar.
    MenuBarExtra(
      "Classick", systemImage: menuBarSystemImage(for: appDelegate.model.phase),
      isInserted: .constant(!ProcessInfo.isRunningInXcodePreviews)
    ) {
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
        onSave: appDelegate.saveSettings
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
