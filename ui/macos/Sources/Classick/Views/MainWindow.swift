import SwiftUI

/// The primary app window: a source sidebar (Library / Devices / History), a
/// detail area, and a persistent bottom device row. Detail views and the
/// device row are filled in by later tasks; this establishes the shell.
struct MainWindow: View {
    var model: AppModel
    // Action closures injected from AppDelegate (wired in later tasks).
    var onSyncNow: () -> Void
    var onPause: () -> Void
    var onCancelSync: () -> Void
    var onResume: () -> Void
    var onRetry: () -> Void
    var onPreview: (SelectionMode, [SelectionRule]) -> Void
    var onSaveSelection: (SelectionMode, [SelectionRule]) -> Void
    var onScan: () -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void
    var onSetUp: () -> Void
    var onReplaceLibrary: () -> Void = {}
    var onAppearRequests: () -> Void = {}
    // Required (no no-op default): a defaulted `{ _ in }` here is exactly how
    // the "+" New Playlist button shipped silently dead (review finding #1)
    // — the call site can compile clean while never actually wiring the
    // daemon send path. See `ClassickApp`'s `MainWindow(...)` call site.
    var onSavePlaylist: (PlaylistPayload) -> Void
    // Playlist editor pages (Task 7).
    var onGetPlaylist: (String) -> Void = { _ in }
    var onDeletePlaylist: (String) -> Void = { _ in }
    var onResolveTracks: ([SelectionRule]) -> Void = { _ in }
    // Device Music page (Task 5).
    var onLoadDeviceConfig: (String) -> Void = { _ in }
    var onPreviewDevice: (String) -> Void = { _ in }
    var onSaveDeviceConfig: (_ serial: String, _ selection: SelectionState?, _ subscriptions: SubscriptionsWire?) -> Void = { _, _, _ in }
    // Device Settings page (Task 6).
    var onSaveDeviceSettings: (_ serial: String, _ settings: DeviceSettingsWire) -> Void = { _, _ in }

    private var selection: Binding<SidebarDestination?> {
        Binding(get: { model.selectedDestination }, set: { model.selectedDestination = $0 })
    }

    var body: some View {
        NavigationSplitView {
            Sidebar(model: model, selection: selection,
                    onForgetIpod: onForgetIpod, onSavePlaylist: onSavePlaylist)
        } detail: {
            detail
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    DeviceRow(model: model,
                              onSyncNow: onSyncNow, onPause: onPause,
                              onCancelSync: onCancelSync, onResume: onResume,
                              onRetry: onRetry)
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
            SetupCallToActionView(onSetUp: onSetUp)
        } else {
            switch model.selectedDestination {
            case .library, nil:
                LibraryView(model: model, onScan: onScan)
            case let .device(serial, .music):
                DeviceMusicPage(
                    model: model, serial: serial, onSyncNow: onSyncNow,
                    onLoadDeviceConfig: onLoadDeviceConfig, onPreviewDevice: onPreviewDevice,
                    onSaveDeviceConfig: onSaveDeviceConfig)
                    .id(serial)
            case let .device(serial, .settings):
                DeviceSettingsPage(
                    model: model, serial: serial,
                    onLoadDeviceConfig: onLoadDeviceConfig, onSaveDeviceSettings: onSaveDeviceSettings,
                    onForgetIpod: onForgetIpod, onBackfill: onBackfill, onReplaceLibrary: onReplaceLibrary)
                    .id(serial)
            case let .playlist(slug):
                PlaylistPage(
                    model: model, slug: slug, onSavePlaylist: onSavePlaylist,
                    onGetPlaylist: onGetPlaylist, onDeletePlaylist: onDeletePlaylist,
                    onResolveTracks: onResolveTracks)
                    .id(slug)
            case .history:
                HistoryView(model: model)
            }
        }
    }
}

/// Shown in the detail area on a fresh, unconfigured install. Reuses the
/// existing setup flow via `onSetUp`.
struct SetupCallToActionView: View {
    var onSetUp: () -> Void
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "ipod").font(.system(size: 48)).foregroundStyle(.secondary)
            Text("Welcome to Classick").font(.title2.bold())
            Text("Choose your music folder to get started.")
                .foregroundStyle(.secondary)
            Button("Set Up Classick…", action: onSetUp)
                .keyboardShortcut(.defaultAction)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
