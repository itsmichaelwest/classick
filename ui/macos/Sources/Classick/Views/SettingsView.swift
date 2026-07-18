import AppKit
import ServiceManagement
import SwiftUI

/// `Settings` scene body: General (music folder, schedule, launch-at-login)
/// + About (version, license, GitHub link). APP-level preferences only —
/// everything device-specific (per-device auto-sync, Rockbox compatibility,
/// artwork backfill, Replace Library, Remove iPod) lives on the sidebar's
/// per-device Settings page (`DeviceSettingsPage`), not here.
///
/// Current config is read from `AppModel.config`, populated by the daemon's
/// `config_update` event (see `AppModel.apply`) — the daemon stays the store
/// of record; this view only mirrors it and writes back via `save_config`.
struct SettingsView: View {
    var model: AppModel
    var onSave: (_ source: String?, _ daemon: DaemonSettings) -> Void

    var body: some View {
        TabView {
            GeneralTab(model: model, onSave: onSave)
                .tabItem { Label("General", systemImage: "gearshape") }
            AboutTab()
                .tabItem { Label("About", systemImage: "info.circle") }
        }
        .frame(width: 440, height: 380)
    }
}

private struct GeneralTab: View {
    var model: AppModel
    var onSave: (_ source: String?, _ daemon: DaemonSettings) -> Void

    @State private var sourcePath: String?
    @State private var scheduleMinutes: UInt32 = 0
    @State private var launchAtLogin = false
    @State private var isPickingFolder = false
    @State private var saveTask: Task<Void, Never>?

    private static let scheduleOptions: [(label: String, minutes: UInt32)] = [
        ("Off", 0),
        ("Every hour", 60),
        ("Every 3 hours", 180),
        ("Every 6 hours", 360),
        ("Every 12 hours", 720),
        ("Every 24 hours", 1440),
    ]

    var body: some View {
        Form {
            LabeledContent("Music Library") {
                HStack {
                    Text(sourcePath ?? "Not set")
                        .foregroundStyle(sourcePath == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Choose…") { isPickingFolder = true }
                }
            }

            Text("Classick backs up your iPod's database before every sync.")
                .font(.footnote)
                .foregroundStyle(.secondary)

            Picker(
                "Scheduled sync",
                selection: Binding(
                    get: { scheduleMinutes },
                    set: { scheduleMinutes = $0; scheduleSave() }
                )
            ) {
                ForEach(Self.scheduleOptions, id: \.minutes) { option in
                    Text(option.label).tag(option.minutes)
                }
            }

            Toggle(
                "Launch at login",
                isOn: Binding(
                    get: { launchAtLogin },
                    set: { newValue in
                        launchAtLogin = newValue
                        applyLaunchAtLogin(newValue)
                        scheduleSave()
                    }
                ))

        }
        .formStyle(.grouped)
        .padding(20)
        .onAppear(perform: syncFromConfig)
        .onChange(of: model.config) { _, _ in syncFromConfig() }
        .fileImporter(isPresented: $isPickingFolder, allowedContentTypes: [.folder]) { result in
            if case let .success(url) = result {
                sourcePath = url.path
                scheduleSave()
            }
        }
    }

    private func syncFromConfig() {
        guard let config = model.config else { return }
        sourcePath = config.source
        if let daemon = config.daemon {
            scheduleMinutes = daemon.scheduleMinutes
            launchAtLogin = daemon.autostartWithWindows
        }
    }

    /// Debounces edits (picker/re-pick/toggle) into a single `save_config`
    /// 400ms after the last change, so rapid toggling doesn't spam the wire.
    /// `enabled` (plug-in auto-sync, superseded by the per-device
    /// "Sync automatically when connected" toggle) and `rockboxCompat`
    /// (moved to the per-device Settings page) are no longer editable from
    /// this view, but both are read straight from `model.config` so a save
    /// from THIS view can't reset whatever values are currently persisted —
    /// the wizard-clobber lesson (see `IpodIdentity.customSelection`).
    private func scheduleSave() {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            let daemon = DaemonSettings(
                enabled: model.config?.daemon?.enabled ?? true,
                autostartWithWindows: launchAtLogin,
                firstSyncMode: model.config?.daemon?.firstSyncMode ?? "auto_apply",
                subsequentSyncMode: model.config?.daemon?.subsequentSyncMode ?? "auto_apply",
                scheduleMinutes: scheduleMinutes,
                notifyOn: model.config?.daemon?.notifyOn ?? "all",
                rockboxCompat: model.config?.daemon?.rockboxCompat ?? false)
            onSave(sourcePath, daemon)
        }
    }

    /// `SMAppService` failures (e.g. running via `swift run` outside a
    /// signed, installed `.app`) are swallowed — `autostartWithWindows` in
    /// the saved config still reflects the user's intent regardless.
    private func applyLaunchAtLogin(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            // Best-effort; see comment above.
        }
    }
}

private struct AboutTab: View {
    private var version: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
    }
    private var build: String {
        Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? ""
    }

    var body: some View {
        VStack(spacing: 12) {
            // The real app icon (from the bundle's AppIcon asset) rather than
            // a generic SF Symbol. `applicationIconImage` is always populated
            // for a bundled, signed app.
            Image(nsImage: NSApplication.shared.applicationIconImage)
                .resizable()
                .frame(width: 96, height: 96)
            Text("Classick")
                .font(.title2.bold())
            Text(build.isEmpty ? "Version \(version)" : "Version \(version) (\(build))")
                .foregroundStyle(.secondary)
            Text("Classick links against libgpod (LGPL v2.1). Source and license terms are available on GitHub.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320)
            Link("View on GitHub", destination: URL(string: "https://github.com/itsmichaelwest/classick")!)
        }
        .padding(24)
    }
}

#if DEBUG
#Preview("Configured") {
    SettingsView(
        model: PreviewFixtures.connectedSyncedModel(),
        onSave: { _, _ in })
        .frame(width: 440, height: 380)
}

#Preview("First run") {
    SettingsView(
        model: PreviewFixtures.firstRunModel(),
        onSave: { _, _ in })
        .frame(width: 440, height: 380)
}
#endif
