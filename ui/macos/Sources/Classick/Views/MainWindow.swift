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
    var onAppearRequests: () -> Void = {}

    enum SidebarItem: Hashable { case library, device, history }
    @State private var selection: SidebarItem = .library

    var body: some View {
        NavigationSplitView {
            List(selection: $selection) {
                Section("Library") {
                    Label("Music Library", systemImage: "music.note.list").tag(SidebarItem.library)
                }
                if model.device != nil {
                    Section("Devices") {
                        Label(model.device?.name ?? model.device?.model ?? "iPod",
                              systemImage: "ipod").tag(SidebarItem.device)
                    }
                }
                Section("History") {
                    Label("Sync History", systemImage: "clock.arrow.circlepath").tag(SidebarItem.history)
                }
            }
            .navigationSplitViewColumnWidth(min: 200, ideal: 210, max: 260)
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
    }

    @ViewBuilder
    private var detail: some View {
        if model.needsFirstRunSetup {
            SetupCallToActionView(onSetUp: onSetUp)
        } else {
            switch selection {
            case .library:
                LibraryView(model: model, onScan: onScan,
                            onPreview: onPreview, onSaveSelection: onSaveSelection)
            case .device:
                DeviceView(model: model, onSaveSettings: onSaveSettings,
                           onForgetIpod: onForgetIpod, onBackfill: onBackfill)
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

// TEMPORARY stubs — replaced by Tasks 9–10. Kept minimal so the scene compiles
// and the window is runnable during Phase B.
struct DeviceView: View {
    var model: AppModel
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void = { _, _ in }
    var onForgetIpod: () -> Void = {}
    var onBackfill: () -> Void = {}
    var body: some View { Text("device view") }
}
struct HistoryView: View { var model: AppModel; var body: some View { Text("history") } }
