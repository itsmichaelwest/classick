import AppKit
import ServiceManagement
import SwiftUI

/// `Settings` scene body: General (music folder, auto-sync, schedule,
/// launch-at-login, forget iPod) + About (version, license, GitHub link).
///
/// Current config is read from `AppModel.config`, populated by the daemon's
/// `config_update` event (see `AppModel.apply`) — the daemon stays the store
/// of record; this view only mirrors it and writes back via `save_config`.
struct SettingsView: View {
    var model: AppModel
    var onSave: (_ source: String?, _ daemon: DaemonSettings) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void

    var body: some View {
        TabView {
            GeneralTab(model: model, onSave: onSave, onForgetIpod: onForgetIpod, onBackfill: onBackfill)
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
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void

    @State private var sourcePath: String?
    @State private var enabled = true
    @State private var scheduleMinutes: UInt32 = 0
    @State private var launchAtLogin = false
    @State private var rockboxCompat = false
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

    private var ipodLabel: String? {
        if let device = model.device { return device.name ?? device.model }
        if let ipod = model.config?.ipod { return ipod.name ?? ipod.modelLabel }
        return nil
    }

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

            Toggle(
                "Sync automatically on plug-in",
                isOn: Binding(
                    get: { enabled },
                    set: { enabled = $0; scheduleSave() }
                ))

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

            Toggle(
                "Rockbox compatibility (embed tags & art in files)",
                isOn: Binding(
                    get: { rockboxCompat },
                    set: { rockboxCompat = $0; scheduleSave() }
                ))
            Text("Embeds tags + cover art into the files so an iPod running Rockbox can read your library (applies to newly synced tracks). Keep any files you copy to the iPod yourself outside iPod_Control.")
                .font(.footnote)
                .foregroundStyle(.secondary)

            Button("Update artwork & metadata") { onBackfill() }
            Text("Refresh artwork + metadata for everything already on the iPod — both the Apple firmware and Rockbox — without re-copying audio. Use after retagging your library (e.g. in Lidarr).")
                .font(.footnote)
                .foregroundStyle(.secondary)

            if let ipodLabel {
                LabeledContent("iPod") {
                    HStack {
                        Text(ipodLabel)
                        Spacer()
                        Button("Remove this iPod", role: .destructive, action: onForgetIpod)
                    }
                }
            }
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
            enabled = daemon.enabled
            scheduleMinutes = daemon.scheduleMinutes
            launchAtLogin = daemon.autostartWithWindows
            rockboxCompat = daemon.rockboxCompat
        }
    }

    /// Debounces edits (toggles/picker/re-pick) into a single `save_config`
    /// 400ms after the last change, so rapid toggling doesn't spam the wire.
    private func scheduleSave() {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            let daemon = DaemonSettings(
                enabled: enabled,
                autostartWithWindows: launchAtLogin,
                firstSyncMode: model.config?.daemon?.firstSyncMode ?? "auto_apply",
                subsequentSyncMode: model.config?.daemon?.subsequentSyncMode ?? "auto_apply",
                scheduleMinutes: scheduleMinutes,
                notifyOn: model.config?.daemon?.notifyOn ?? "all",
                rockboxCompat: rockboxCompat)
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
