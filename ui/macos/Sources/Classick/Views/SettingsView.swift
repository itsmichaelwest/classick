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
/// `global_config` event (see `AppModel.apply`) — the daemon stays the store
/// of record; this view only mirrors it and writes back via `save_config`.
struct SettingsView: View {
    var model: AppModel
    var onSave: (_ source: String?, _ daemon: DaemonSettings) -> String

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
    var onSave: (_ source: String?, _ daemon: DaemonSettings) -> String

    private struct GlobalSettingsDraft: Equatable {
        var sourcePath: String?
        var scheduleMinutes: UInt32
        var launchAtLogin: Bool
        var dropSyncBehavior: DropSyncBehaviorWire
    }

    @State private var sourcePath: String?
    @State private var scheduleMinutes: UInt32 = 0
    @State private var launchAtLogin = false
    @State private var dropSyncBehavior: DropSyncBehaviorWire = .immediate
    @State private var acknowledgedDraft = AcknowledgedDraft(
        canonical: GlobalSettingsDraft(
            sourcePath: nil, scheduleMinutes: 0, launchAtLogin: false,
            dropSyncBehavior: .immediate),
        revision: 0)
    @State private var hasCanonicalDraft = false
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
                    set: { value in edit { $0.scheduleMinutes = value } }
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
                        edit { $0.launchAtLogin = newValue }
                        applyLaunchAtLogin(newValue)
                    }
                ))

            Picker(
                "After adding music to an iPod",
                selection: Binding(
                    get: { dropSyncBehavior },
                    set: { value in edit { $0.dropSyncBehavior = value } }
                )
            ) {
                Text("Sync immediately").tag(DropSyncBehaviorWire.immediate)
                Text("On next sync").tag(DropSyncBehaviorWire.nextSync)
            }

        }
        .formStyle(.grouped)
        .padding(20)
        .onAppear(perform: syncFromConfig)
        .onChange(of: model.config) { _, _ in syncFromConfig() }
        .onChange(of: model.configRevision) { _, _ in syncFromConfig() }
        .fileImporter(isPresented: $isPickingFolder, allowedContentTypes: [.folder]) { result in
            if case let .success(url) = result {
                edit { $0.sourcePath = url.path }
            }
        }
    }

    private func syncFromConfig() {
        guard let config = model.config else { return }
        let canonical = GlobalSettingsDraft(
            sourcePath: config.source,
            scheduleMinutes: config.daemon?.scheduleMinutes ?? 0,
            launchAtLogin: config.daemon?.autostartWithWindows ?? false,
            dropSyncBehavior: config.daemon?.dropSyncBehavior ?? .immediate)
        acknowledgedDraft.reconcile(
            canonical: canonical, revision: model.configRevision,
            acknowledgedRequestID: model.configAcknowledgedRequestID)
        sourcePath = acknowledgedDraft.value.sourcePath
        scheduleMinutes = acknowledgedDraft.value.scheduleMinutes
        launchAtLogin = acknowledgedDraft.value.launchAtLogin
        dropSyncBehavior = acknowledgedDraft.value.dropSyncBehavior
        hasCanonicalDraft = true
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
                rockboxCompat: model.config?.daemon?.rockboxCompat ?? false,
                dropSyncBehavior: dropSyncBehavior)
            let requestID = onSave(sourcePath, daemon)
            acknowledgedDraft.markSubmitted(requestID: requestID)
        }
    }

    private func edit(_ mutation: (inout GlobalSettingsDraft) -> Void) {
        guard hasCanonicalDraft else { return }
        var edited = acknowledgedDraft.value
        mutation(&edited)
        guard edited != acknowledgedDraft.value else { return }
        acknowledgedDraft.edit(edited)
        sourcePath = acknowledgedDraft.value.sourcePath
        scheduleMinutes = acknowledgedDraft.value.scheduleMinutes
        launchAtLogin = acknowledgedDraft.value.launchAtLogin
        dropSyncBehavior = acknowledgedDraft.value.dropSyncBehavior
        scheduleSave()
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
        onSave: { _, _ in "preview" })
        .frame(width: 440, height: 380)
}

#Preview("First run") {
    SettingsView(
        model: PreviewFixtures.firstRunModel(),
        onSave: { _, _ in "preview" })
        .frame(width: 440, height: 380)
}
#endif
