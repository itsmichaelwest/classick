import SwiftUI

/// The primary app window: a source sidebar (Library / Devices / History), a
/// detail area, and a persistent bottom device row. Detail views and the
/// device row are filled in by later tasks; this establishes the shell.
struct MainWindow: View {
  var model: AppModel
  @State private var columnVisibility: NavigationSplitViewVisibility = .all
  // Action closures injected from AppDelegate (wired in later tasks).
  var onSyncNow: (DeviceSerial) -> Void
  var onPause: (DeviceSerial) -> Void
  var onCancelSync: (DeviceSerial) -> Void
  var onResume: (DeviceSerial) -> Void
  var onRetry: (DeviceSerial) -> Void
  var onScan: () -> Void
  var onForgetIpod: (DeviceSerial) -> Void
  /// True disk eject (sidebar's eject glyph) — distinct from
  /// `onForgetIpod` (Settings page's unpair action). Required, no no-op
  /// default — the shipped-silently-dead lesson.
  var onEjectIpod: (DeviceSerial) -> Void
  var onBackfill: (DeviceSerial) -> Void
  var onSetUp: (DeviceSerial?) -> Void
  var onReplaceLibrary: (DeviceSerial) -> Void = { _ in }
  var onAppearRequests: () -> Void = {}
  // Required (no no-op default): a defaulted `{ _ in }` here is exactly how
  // the "+" New Playlist button shipped silently dead (review finding #1)
  // — the call site can compile clean while never actually wiring the
  // daemon send path. See `ClassickApp`'s `MainWindow(...)` call site.
  var onSavePlaylist: (PlaylistPayload) -> Void
  // Playlist editor pages (Task 7).
  var onGetPlaylist: (String) -> Void = { _ in }
  var onDeletePlaylist: (String) -> Void = { _ in }
  var onResolveTracks: (_ slug: String, _ rules: [SelectionRule]) -> Void = { _, _ in }
  // Device Music page (Task 5).
  var onLoadDeviceConfig: (String) -> Void = { _ in }
  var onSaveAndPreviewDeviceConfig:
    (_ serial: String, _ selection: SelectionState?, _ subscriptions: SubscriptionsWire?) -> Void =
      { _, _, _ in }
  // Device Settings page (Task 6).
  var onSaveDeviceSettings: (_ serial: String, _ settings: DeviceSettingsWire) -> Void = { _, _ in }

  private var selection: Binding<SidebarDestination?> {
    Binding(get: { model.selectedDestination }, set: { model.selectedDestination = $0 })
  }

  var body: some View {
    // Sidebar is collapsible via the system toggle — the native pattern
    // (Music/Finder/Mail all allow it). An earlier pinned-open variant
    // stripped the toggle from both toolbars, which left pages with no
    // other toolbar items an unstable toolbar height (layout break on
    // Settings/History); the standard leading toggle keeps the chrome
    // consistent everywhere.
    NavigationSplitView(columnVisibility: $columnVisibility) {
      Sidebar(
        model: model, selection: selection,
        onEjectIpod: onEjectIpod, onSavePlaylist: onSavePlaylist)
    } detail: {
      detail
        // Fill the pane BEFORE attaching the bottom inset: pages
        // that hug their content (e.g. HistoryView's empty state)
        // otherwise shrink the view the inset attaches to, and the
        // floating DeviceRow renders mid-pane instead of pinned to
        // the window's bottom edge.
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .safeAreaInset(edge: .bottom, spacing: 0) {
          DeviceRow(
            model: model,
            onSyncNow: onSyncNow, onPause: onPause,
            onCancelSync: onCancelSync, onResume: onResume,
            onRetry: onRetry, onSetUp: onSetUp)
        }
    }
    .navigationTitle("Classick")
    .frame(minWidth: 860, minHeight: 560)
    .task { onAppearRequests() }
    .onAppear {
      // Default to Library on first show — `selectedDestination` starts
      // `nil` (AppModel has no opinion on initial navigation), and an
      // empty NavigationSplitView selection would render a blank
      // detail pane.
      if model.selectedDestination == nil {
        model.selectedDestination = .library
      }
    }
  }

  @ViewBuilder
  private var detail: some View {
    if model.needsFirstRunSetup {
      SetupCallToActionView { onSetUp(model.focusedDeviceSerial) }
    } else {
      switch model.selectedDestination {
      case .library, nil:
        LibraryView(model: model, onScan: onScan)
      case .device(let serial, .music):
        DeviceMusicPage(
          model: model, serial: serial, onSyncNow: onSyncNow,
          onLoadDeviceConfig: onLoadDeviceConfig,
          onSaveAndPreviewDeviceConfig: onSaveAndPreviewDeviceConfig, onScan: onScan
        )
        .id(serial)
      case .device(let serial, .settings):
        DeviceSettingsPage(
          model: model, serial: serial,
          onLoadDeviceConfig: onLoadDeviceConfig, onSaveDeviceSettings: onSaveDeviceSettings,
          onForgetIpod: onForgetIpod, onBackfill: onBackfill, onReplaceLibrary: onReplaceLibrary
        )
        .id(serial)
      case .playlist(let slug):
        PlaylistPage(
          model: model, slug: slug, onSavePlaylist: onSavePlaylist,
          onGetPlaylist: onGetPlaylist, onDeletePlaylist: onDeletePlaylist,
          onResolveTracks: onResolveTracks
        )
        .id(slug)
      case .history:
        HistoryView(model: model)
      }
    }
  }
}

/// Shown in the detail area on a fresh, unconfigured install — this IS
/// Global Constraints' "no source configured" empty state ("Choose your
/// music folder…", opens setup). It's implemented once, here, rather than
/// per-page (Library/Device Music) because `MainWindow.detail` gates the
/// entire detail area on `needsFirstRunSetup` before any page reachable —
/// no page-level "no source configured" branch is ever reachable once a
/// source has been configured, since `needsFirstRunSetup` only flips back
/// to `true` if the source is cleared, which would route back here too.
struct SetupCallToActionView: View {
  var onSetUp: () -> Void
  var body: some View {
    VStack(spacing: 14) {
      Image(systemName: "ipod").font(.system(size: 48)).foregroundStyle(.secondary)
      Text("Welcome to Classick").font(.title2.bold())
      Button("Choose your music folder…", action: onSetUp)
        .keyboardShortcut(.defaultAction)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}

#if DEBUG
  extension MainWindow {
    /// Every closure the real call site (`ClassickApp`) wires up, defaulted
    /// to no-ops — previews render state, they don't need live daemon
    /// plumbing.
    fileprivate init(previewModel model: AppModel) {
      self.init(
        model: model,
        onSyncNow: { _ in }, onPause: { _ in }, onCancelSync: { _ in },
        onResume: { _ in }, onRetry: { _ in }, onScan: {},
        onForgetIpod: { _ in }, onEjectIpod: { _ in },
        onBackfill: { _ in }, onSetUp: { _ in }, onReplaceLibrary: { _ in },
        onAppearRequests: {}, onSavePlaylist: { _ in },
        // Answer get_playlist from fixture data — a no-op here left
        // every playlist page in the canvas on "Loading…" forever,
        // since the reply the page waits for can never arrive.
        onGetPlaylist: { slug in
          model.apply(.playlistDetail(PreviewFixtures.playlistDetail(forSlug: slug)))
        })
    }
  }

  #Preview("Full app") {
    MainWindow(previewModel: PreviewFixtures.connectedSyncedModel())
      .frame(width: 1000, height: 640)
  }

  #Preview("First run") {
    MainWindow(previewModel: PreviewFixtures.firstRunModel())
      .frame(width: 1000, height: 640)
  }
#endif
