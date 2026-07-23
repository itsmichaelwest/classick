import AppKit
import SwiftUI
import os

enum DeviceActionCommand {
  static func sync(serial: DeviceID, requestID: UUID) -> WireV3Command {
    .triggerSync(deviceID: serial, requestID: requestID, trigger: .manual)
  }

  static func cancel(route: WireV3Route, requestID: UUID) -> WireV3Command {
    .cancelSync(route: route, requestID: requestID)
  }

  static func pause(route: WireV3Route, requestID: UUID) -> WireV3Command {
    .pauseSync(route: route, requestID: requestID)
  }

  static func forget(serial: DeviceID, requestID: UUID) -> WireV3Command {
    .forgetDevice(deviceID: serial, requestID: requestID)
  }

  static func replaceLibrary(serial: DeviceID, requestID: UUID) -> WireV3Command {
    .replaceLibrary(deviceID: serial, requestID: requestID)
  }

  static func backfillRockbox(serial: DeviceID, requestID: UUID) -> WireV3Command {
    .backfillRockbox(deviceID: serial, requestID: requestID)
  }
}

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
  private var libraryDropSubmissionCoordinator: LibraryDropSubmissionCoordinator!
  private let libraryDropAnnouncementCoordinator = LibraryDropAnnouncementCoordinator()
  private let daemonProcess = DaemonProcess()
  private let daemonShutdownCoordinator = DaemonShutdownCoordinator()
  private let setupWindowController = SetupWindowController()

  /// First-run setup is auto-presented exactly once per launch, the moment
  /// the daemon confirms the user is unconfigured. This latches that so a
  /// later `global_config` (or reconnect churn) can't re-open it.
  private var didAutoPresentSetup = false
  #if canImport(Sparkle)
    private let updater = Updater()
  #endif
  private var eventTask: Task<Void, Never>?
  private var syncNotificationCoordinator = SyncNotificationCoordinator()
  private var sourceConnectIntent = SourceConnectIntent()

  /// Mirrors `daemonClient.lastFatalError` on the main actor. The actor's
  /// own property can't be read synchronously from a SwiftUI `body`, so
  /// this copy is refreshed whenever the event stream ends (which is
  /// exactly when a fatal handshake error, if any, was set).
  @Published private(set) var daemonFatalError: String?

  override init() {
    super.init()
    libraryDropSubmissionCoordinator = LibraryDropSubmissionCoordinator(
      send: { [daemonClient] command in await daemonClient.send(command) },
      rejectLocally: { [model] requestID, target, message in
        model.rejectLibraryDropLocally(requestID: requestID, target: target, message: message)
      })
  }

  func applicationDidFinishLaunching(_ notification: Notification) {
    // Xcode renders an app target's previews by launching the app as the
    // preview host. Fixtures need none of the launch side effects.
    guard !ProcessInfo.isRunningInXcodePreviews else { return }

    Notifier.requestAuth()
    eventTask = Task {
      await self.daemonProcess.ensureRunning()
      // Register the event stream's continuation before starting the
      // connect/reconnect loop, so nothing yielded during the initial
      // handshake is dropped (mirrors DaemonClientTests' call order).
      let stream = await self.daemonClient.events()
      await self.daemonClient.start()
      for await event in stream {
        self.model.apply(event)
        if case .hello = event { self.requestLibraryAndSelection() }
        if let requestID = self.model.persistedDropAcknowledgements.last,
          let outcome = self.model.dropOutcome
        {
          self.libraryDropAnnouncementCoordinator.announce(
            requestID: requestID, outcome: outcome)
        }
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
        self.postNotifications(for: event)
        self.presentPromptIfNeeded()
        self.autoPresentSetupIfNeeded()
      }
      self.daemonFatalError = await self.daemonClient.lastFatalError
    }
  }

  func applicationDidBecomeActive(_ notification: Notification) {
    guard sourceConnectIntent.applicationDidBecomeActive() else { return }
    sendSourceMountRetry()
  }

  func submitLibraryDrop(
    target: LibraryDropTarget, rules: [SelectionRule], requestID: UUID
  ) {
    model.markLibraryDropAdding(requestID: requestID, target: target)
    libraryDropSubmissionCoordinator.submit(
      target: target, rules: rules, requestID: requestID)
  }

  /// Shows first-run setup. Wired to the "Set Up Classick…" menu row and
  /// reused by `autoPresentSetupIfNeeded`.
  func presentSetup(serial: DeviceID? = nil) {
    setupWindowController.show(
      model: model, preferredSerial: serial, onDone: finishSetup)
  }

  /// Requests a fresh library + selection + sync-history snapshot from the
  /// daemon. Used by `MainWindow`'s `.task` on first appear so the persistent
  /// `LibraryView` and History tab both have data on first view
  /// (`AppModel.history` is otherwise only ever populated by an unsolicited
  /// `history_update`).
  func requestLibraryAndSelection() {
    Task {
      await daemonClient.send(.getLibrary(requestID: WireV3Command.newRequestID()))
      await daemonClient.send(.getHistory(requestID: WireV3Command.newRequestID(), limit: 50))
      // Protocol 1.6.0: the sidebar's Playlists section and the device
      // Music page's subscriptions checklist both read `model.playlists`,
      // populated only by a `playlists_update` reply/broadcast — nothing
      // requests the initial list otherwise.
      await daemonClient.send(.listPlaylists(requestID: WireV3Command.newRequestID()))
    }
  }

  /// "Rescan Library" action for the main window's `LibraryView`.
  func rescan() {
    Task { await daemonClient.send(.scanLibrary(requestID: WireV3Command.newRequestID())) }
  }

  func connectSource() {
    guard
      sourceConnectIntent.userRequestedConnect(
        isApplicationActive: NSApplication.shared.isActive)
    else {
      NSApplication.shared.activate(ignoringOtherApps: true)
      return
    }
    sendSourceMountRetry()
  }

  private func sendSourceMountRetry() {
    let requestID = WireV3Command.newRequestID()
    guard
      let command = model.prepareSourceMountRetry(
        isApplicationActive: NSApplication.shared.isActive,
        requestID: requestID)
    else { return }
    Task { await daemonClient.send(command) }
  }

  /// TRUE disk eject for the sidebar's eject glyph: unmounts the iPod's
  /// volume via `NSWorkspace` (the native "safe to unplug" operation) —
  /// deliberately NOT `forgetIpod`, which unpairs the device from Classick
  /// and lives on the device Settings page. The daemon holds no volume
  /// handles while idle (the sync subprocess only runs during a sync, and
  /// the sidebar disables eject mid-sync), so the unmount normally
  /// succeeds; when it doesn't (Finder/another app holding files open),
  /// the system's error is surfaced in an alert rather than swallowed.
  func ejectIpod(serial: DeviceID) {
    guard let drive = model.devices[serial]?.mountPath else { return }
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

  /// Auto-presents setup the first time the daemon confirms (post-handshake)
  /// that the user has no configured music library. Gated on
  /// `needsFirstRunSetup`, which stays false until the `get_config` reply
  /// lands, so this can't fire during the startup race — and latched by
  /// `didAutoPresentSetup` so it happens at most once per launch.
  private func autoPresentSetupIfNeeded() {
    guard !didAutoPresentSetup, model.needsFirstRunSetup else { return }
    didAutoPresentSetup = true
    presentSetup(serial: model.focusedDeviceSerial)
  }

  /// Sends the setup window's `save_config` (folder + auto-sync + the
  /// currently-detected iPod, if any) and clears any error banner from a
  /// prior failed handshake/save.
  func finishSetup(source: String, autoSync: Bool, serial: DeviceID) {
    guard model.canSendDeviceCommand(to: serial) else { return }
    let requestID = WireV3Command.newRequestID()
    let current = model.editableDeviceConfig(for: serial) ?? .defaultState
    let selectionMutationID = UUID()
    let settingsMutationID = UUID()
    let subscriptionsMutationID = UUID()
    let settings = DeviceSettingsWire(
      autoSync: autoSync, rockboxCompat: current.settings.rockboxCompat)
    model.editDeviceSettings(settings, for: serial)
    model.willRequestDeviceConfig(
      serial: serial, requestID: requestID.uuidString.lowercased(), intent: .write)
    model.markDeviceSelectionSubmitted(
      for: serial,
      receipt: .init(
        requestID: requestID.uuidString.lowercased(),
        mutationID: selectionMutationID.uuidString.lowercased()))
    model.markDeviceSettingsSubmitted(
      for: serial,
      receipt: .init(
        requestID: requestID.uuidString.lowercased(),
        mutationID: settingsMutationID.uuidString.lowercased()))
    model.markDeviceSubscriptionsSubmitted(
      for: serial,
      receipt: .init(
        requestID: requestID.uuidString.lowercased(),
        mutationID: subscriptionsMutationID.uuidString.lowercased()))
    sendDeviceCommands(
      serial: serial,
      commands: Self.setupDeviceCommands(
        source: source, serial: serial, current: current, autoSync: autoSync,
        requestID: requestID, selectionMutationID: selectionMutationID,
        settingsMutationID: settingsMutationID,
        subscriptionsMutationID: subscriptionsMutationID))
  }

  /// Settings window's debounced edits. `ipod` is omitted (nil) so an
  /// existing iPod pairing isn't disturbed by unrelated setting changes —
  /// only "Remove this iPod" (`forgetIpod()`) touches that.
  func saveSettings(source: String?, daemon: DaemonSettings) -> String {
    let requestID = WireV3Command.newRequestID()
    Task {
      await daemonClient.send([
        .setSourceLocation(requestID: requestID, sourceRoot: source),
        .setGlobalSettings(
          requestID: WireV3Command.newRequestID(), settings: WireV3GlobalSettings(daemon)),
      ])
    }
    return requestID.uuidString.lowercased()
  }

  func forgetIpod(serial: DeviceID) {
    sendDeviceCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.forget(serial: serial, requestID: WireV3Command.newRequestID())
      ])
  }

  /// "Replace Library…" confirmation sheet's confirm action. The UI
  /// (`DeviceSettingsPage`) is responsible for obtaining the user's typed
  /// confirmation before calling this — mirrors `replace_library`'s own
  /// contract on the wire (see `WireV3Command.replaceLibrary`'s definition
  /// comment): the daemon does not prompt.
  func replaceLibrary(serial: DeviceID) {
    sendDeviceCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.replaceLibrary(
          serial: serial, requestID: WireV3Command.newRequestID())
      ])
  }

  /// "Update existing library for Rockbox" button in Settings — asks the
  /// daemon to re-embed tags/art into already-synced tracks so an iPod
  /// running Rockbox (which doesn't read the iTunesDB) can display them.
  func backfillRockbox(serial: DeviceID) {
    sendDeviceCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.backfillRockbox(
          serial: serial, requestID: WireV3Command.newRequestID())
      ])
  }

  /// Surfaces `model.pendingPrompt` (set by the reducer from a relayed
  /// protocol v3 prompt/form event) as a blocking `NSAlert`, then replies
  /// with the chosen option and clears it so it isn't re-shown.
  private func presentPromptIfNeeded() {
    guard let prompt = model.pendingPrompt else { return }
    guard let serial = model.focusedDeviceSerial,
      let sessionID = model.devices[serial]?.sessionID
    else { return }
    let choice = PromptAlert.present(prompt)
    model.clearPendingPrompt()
    sendDeviceCommands(
      serial: serial,
      commands: [
        .promptDecision(
          route: WireV3Route(deviceID: serial, sessionID: sessionID),
          requestID: WireV3Command.newRequestID(), promptID: prompt.id,
          choice: UInt32(clamping: choice))
      ])
  }

  /// Sync Now / Cancel Sync menu actions. `DaemonClient.send` is async
  /// (actor-isolated socket write); the menu's `Button` actions are sync
  /// closures, so each hop through here spawns a detached-from-the-caller
  /// `Task` to bridge to the actor.
  func syncNow(serial: DeviceID) {
    sendSyncControlCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.sync(serial: serial, requestID: WireV3Command.newRequestID())
      ])
  }

  func cancelSync(serial: DeviceID) {
    guard let sessionID = model.devices[serial]?.sessionID else { return }
    sendSyncControlCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.cancel(
          route: WireV3Route(deviceID: serial, sessionID: sessionID),
          requestID: WireV3Command.newRequestID())
      ])
  }

  /// Pause / Resume menu actions. Pause requests a graceful drain +
  /// checkpoint on the daemon side; resume is just a normal sync trigger —
  /// the sync is diff-based, so it continues where it left off.
  func pause(serial: DeviceID) {
    guard let sessionID = model.devices[serial]?.sessionID else { return }
    sendSyncControlCommands(
      serial: serial,
      commands: [
        DeviceActionCommand.pause(
          route: WireV3Route(deviceID: serial, sessionID: sessionID),
          requestID: WireV3Command.newRequestID())
      ])
  }

  func resume(serial: DeviceID) {
    syncNow(serial: serial)
  }

  /// "Retry" starts a new sync attempt. Clear only the presented terminal
  /// error first; the daemon's next serial/session snapshot remains the
  /// authority for the new run.
  func retry(serial: DeviceID) {
    model.dismissTerminalError(for: serial)
    syncNow(serial: serial)
  }

  /// Sidebar's "+ New Playlist" flow (Task 3). The daemon replies with an
  /// unsolicited `playlists_update` (not a direct reply to this command),
  /// which is what `Sidebar`'s `destinationForNewlyCreatedPlaylist` flow
  /// watches for to pick up the newly assigned slug.
  func savePlaylist(_ payload: PlaylistPayload) -> String {
    let requestID = WireV3Command.newRequestID()
    Task {
      await daemonClient.send(
        .savePlaylist(requestID: requestID, playlist: payload))
    }
    return requestID.uuidString.lowercased()
  }

  /// Playlist editor pages (Task 7): fetches one playlist's full content
  /// the moment the user navigates to its page — `model.playlistDetail`
  /// is otherwise only ever populated by an unsolicited reply.
  func getPlaylist(slug: String) {
    Task {
      await daemonClient.send(
        .getPlaylist(requestID: WireV3Command.newRequestID(), slug: slug))
    }
  }

  /// Toolbar menu's "Delete Playlist" (with confirmation, handled by the
  /// caller). The daemon replies with a correlated `playlists_update` and
  /// broadcasts any changed device configurations.
  func deletePlaylist(slug: String) {
    Task {
      await daemonClient.send(
        .deletePlaylist(requestID: WireV3Command.newRequestID(), slug: slug))
    }
  }

  /// Add Songs picker: expands checked
  /// artist/album/genre rules into real track paths server-side — see
  /// `AppModel.willRequestResolveTracks`'s doc comment for why this
  /// bookkeeping precedes the send (mirrors `previewDevice`). `slug` is the
  /// requesting playlist editor's own slug (no wire change — purely
  /// client-side correlation bookkeeping, see `ResolvedTracksReply`).
  func resolveTracks(slug: String, rules: [SelectionRule]) {
    model.willRequestResolveTracks(slug: slug)
    Task {
      await daemonClient.send(
        .resolveTracks(requestID: WireV3Command.newRequestID(), rules: rules))
    }
  }

  /// Device Music page (Task 5): fetches the device's current selection +
  /// subscriptions + settings, plus a fresh capacity preview, the moment
  /// the user navigates to its Music page. Device config isn't part of
  /// `requestLibraryAndSelection`'s window-appear batch since it's scoped
  /// to whichever device page is showing, not global app state.
  func loadDeviceConfig(serial: DeviceID) {
    guard model.canSendDeviceCommand(to: serial) else { return }
    let configRequestID = WireV3Command.newRequestID()
    let previewRequestID = WireV3Command.newRequestID()
    model.willRequestDeviceConfig(
      serial: serial, requestID: configRequestID.uuidString.lowercased(), intent: .read)
    model.willRequestDevicePreview(
      serial: serial, requestID: previewRequestID.uuidString.lowercased())
    sendDeviceCommands(
      serial: serial,
      commands: [
        .getDeviceConfig(deviceID: serial, requestID: configRequestID),
        .previewDevice(deviceID: serial, requestID: previewRequestID),
      ])
  }

  /// Live capacity/skip preview for a candidate device selection/
  /// subscription edit. Registering the exact request ID lets the reducer
  /// reject an older preview that arrives after a newer request.
  func previewDevice(serial: DeviceID) {
    guard model.canSendDeviceCommand(to: serial) else { return }
    let requestID = WireV3Command.newRequestID()
    model.willRequestDevicePreview(
      serial: serial, requestID: requestID.uuidString.lowercased())
    sendDeviceCommands(
      serial: serial,
      commands: [
        .previewDevice(deviceID: serial, requestID: requestID)
      ])
  }

  /// Device Music edits are one ordered socket batch: the daemon persists
  /// the save before it evaluates the following preview.
  func saveAndPreviewDeviceConfig(
    serial: DeviceID, selection: SelectionState?, subscriptions: SubscriptionsWire?
  ) -> DeviceMusicMutationReceipt? {
    guard model.canSendDeviceCommand(to: serial) else { return nil }
    let selectionRequestID = selection.map { _ in WireV3Command.newRequestID() }
    let subscriptionsRequestID = subscriptions.map { _ in WireV3Command.newRequestID() }
    guard subscriptionsRequestID != nil || selectionRequestID != nil else { return nil }
    let selectionMutationID = selection.map { _ in UUID() }
    let subscriptionsMutationID = subscriptions.map { _ in UUID() }
    let previewRequestID = WireV3Command.newRequestID()
    model.willRequestDevicePreview(
      serial: serial, requestID: previewRequestID.uuidString.lowercased())
    var commands: [WireV3Command] = []
    var receipt = DeviceMusicMutationReceipt()
    if let selection, let requestID = selectionRequestID, let mutationID = selectionMutationID {
      model.willRequestDeviceConfig(
        serial: serial, requestID: requestID.uuidString.lowercased(), intent: .write)
      commands.append(
        .setSelection(
          deviceID: serial, requestID: requestID, mutationID: mutationID,
          selection: WireV3SelectionValue(selection)))
      receipt.selection = .init(
        requestID: requestID.uuidString.lowercased(),
        mutationID: mutationID.uuidString.lowercased())
    }
    if let subscriptions, let requestID = subscriptionsRequestID,
      let mutationID = subscriptionsMutationID
    {
      model.willRequestDeviceConfig(
        serial: serial, requestID: requestID.uuidString.lowercased(), intent: .write)
      commands.append(
        .setSubscriptions(
          deviceID: serial, requestID: requestID, mutationID: mutationID,
          subscriptions: WireV3SubscriptionsValue(subscriptions)))
      receipt.subscriptions = .init(
        requestID: requestID.uuidString.lowercased(),
        mutationID: mutationID.uuidString.lowercased())
    }
    commands.append(.previewDevice(deviceID: serial, requestID: previewRequestID))
    sendDeviceCommands(
      serial: serial,
      commands: commands)
    return receipt
  }

  /// Device Settings page's debounced auto-save (Task 6): the mirror image
  /// of `saveDeviceConfig` above — `selection`/`subscriptions` are always
  /// omitted (nil = "don't change") via `DeviceSettingsLogic.saveSettingsCommand`,
  /// so a toggle edit here can never disturb the Music page's sync intent.
  func saveDeviceSettings(
    serial: DeviceID, settings: DeviceSettingsWire
  ) -> DeviceMutationReceipt? {
    guard model.canSendDeviceCommand(to: serial) else { return nil }
    let requestID = WireV3Command.newRequestID()
    let mutationID = UUID()
    model.willRequestDeviceConfig(
      serial: serial, requestID: requestID.uuidString.lowercased(), intent: .write)
    sendDeviceCommands(
      serial: serial,
      commands: [
        .setSettings(
          deviceID: serial, requestID: requestID, mutationID: mutationID,
          settings: WireV3SettingsValue(settings))
      ])
    return .init(
      requestID: requestID.uuidString.lowercased(),
      mutationID: mutationID.uuidString.lowercased())
  }

  private func sendDeviceCommands(serial: DeviceID, commands: [WireV3Command]) {
    guard model.canSendDeviceCommand(to: serial) else { return }
    Task { [weak self] in
      guard let self, self.model.canSendDeviceCommand(to: serial) else { return }
      await self.daemonClient.send(commands)
    }
  }

  private func sendSyncControlCommands(serial: DeviceID, commands: [WireV3Command]) {
    guard model.canControlSync(to: serial) else { return }
    Task { [weak self] in
      guard let self, self.model.canControlSync(to: serial) else { return }
      await self.daemonClient.send(commands)
    }
  }

  /// No-op under `swift test` (see `Updater.swift`) — SPM's `Package.swift`
  /// graph doesn't carry the Sparkle dependency.
  func checkForUpdates() {
    #if canImport(Sparkle)
      updater.checkForUpdates()
    #endif
  }

  private func postNotifications(for event: WireV3Event) {
    for notification in syncNotificationCoordinator.consume(event, devices: model.devices) {
      guard
        Notifier.shouldPostSyncFinished(
          notifyOn: model.config?.daemon?.notifyOn,
          success: notification.success)
      else { continue }
      Notifier.syncFinished(notification)
    }
  }

  func applicationWillTerminate(_ notification: Notification) {
    eventTask?.cancel()
    Task { await daemonClient.stop() }
  }

  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    daemonShutdownCoordinator.begin(
      shutdown: { [daemonClient, daemonProcess] in
        await daemonProcess.stopForQuit(client: daemonClient)
      },
      forceTerminateOwnedDaemon: { [daemonProcess] in
        daemonProcess.forceTerminateOwnedDaemon()
      },
      reply: { shouldTerminate in
        sender.reply(toApplicationShouldTerminate: shouldTerminate)
      })
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
        onScan: appDelegate.rescan,
        onConnectSource: appDelegate.connectSource,
        onForgetIpod: appDelegate.forgetIpod,
        onEjectIpod: appDelegate.ejectIpod,
        onBackfill: appDelegate.backfillRockbox,
        onSetUp: { serial in appDelegate.presentSetup(serial: serial) },
        onReplaceLibrary: appDelegate.replaceLibrary,
        onAppearRequests: appDelegate.requestLibraryAndSelection,
        onSavePlaylist: { payload in _ = appDelegate.savePlaylist(payload) },
        onSubmitLibraryDrop: appDelegate.submitLibraryDrop,
        onSavePlaylistDraft: appDelegate.savePlaylist,
        onGetPlaylist: { slug in appDelegate.getPlaylist(slug: slug) },
        onDeletePlaylist: { slug in appDelegate.deletePlaylist(slug: slug) },
        onResolveTracks: { slug, rules in appDelegate.resolveTracks(slug: slug, rules: rules) },
        onLoadDeviceConfig: { serial in appDelegate.loadDeviceConfig(serial: serial) },
        onSaveAndPreviewDeviceConfig: { serial, selection, subscriptions in
          appDelegate.saveAndPreviewDeviceConfig(
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
      isInserted: .constant(!ProcessInfo.isRunningInXcodePreviews)
    ) {
      MenuContent(
        model: appDelegate.model,
        daemonFatalError: appDelegate.daemonFatalError,
        onSetUp: { serial in appDelegate.presentSetup(serial: serial) },
        onOpenMain: openMainWindow,
        onOpenSettings: openSettingsWindow,
        onSyncNow: appDelegate.syncNow,
        onRescan: appDelegate.rescan,
        onConnectSource: appDelegate.connectSource,
        onCancelSync: appDelegate.cancelSync,
        onPause: appDelegate.pause,
        onResume: appDelegate.resume,
        onRetry: appDelegate.retry,
        onCheckForUpdates: appDelegate.checkForUpdates
      )
    } label: {
      MenuBarLabel(
        presentation: MenuBarLabelPresentation.make(
          globalPhase: appDelegate.model.phase,
          device: appDelegate.model.focusedDeviceSerial.flatMap {
            appDelegate.model.devices[$0]
          }))
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
