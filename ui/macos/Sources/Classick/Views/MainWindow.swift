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
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void
    var onSetUp: () -> Void
    var onSaveIpodSelection: (Bool) -> Void = { _ in }
    var onReplaceLibrary: () -> Void = {}
    var onAppearRequests: () -> Void = {}
    var onSavePlaylist: (PlaylistPayload) -> Void = { _ in }

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
                LibraryView(model: model, onScan: onScan,
                            onPreview: onPreview, onSaveSelection: onSaveSelection)
            case .device:
                // Device Music/Settings pages are built out in Tasks 5-6; for
                // now both pages of the disclosure route to the existing
                // dashboard so navigation is exercisable end-to-end.
                DeviceView(model: model, onSaveSettings: onSaveSettings,
                           onForgetIpod: onForgetIpod, onBackfill: onBackfill,
                           onSaveIpodSelection: onSaveIpodSelection, onReplaceLibrary: onReplaceLibrary)
            case let .playlist(slug):
                // Playlist editor pages are built in Task 7.
                ContentUnavailableView(
                    "Playlist Editor Coming Soon",
                    systemImage: "music.note.list",
                    description: Text(slug))
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
