import SwiftUI

/// Device dashboard: identity, capacity, sync status, and device-scoped
/// controls (auto-sync, Rockbox compat, backfill, forget). Reads state the
/// daemon already sends via `config_update`/device events (see `AppModel`);
/// writes via the same `save_config` / `backfill` / `forget_ipod` commands
/// `SettingsView` uses.
///
/// `model.device` can go `nil` mid-display if the iPod disconnects while this
/// view is on screen (the sidebar selection isn't reset on disconnect — see
/// `MainWindow`), so this view must degrade to a placeholder rather than
/// force-unwrap.
struct DeviceView: View {
    var model: AppModel
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void

    @State private var autoSync = true
    @State private var rockboxCompat = false
    @State private var saveTask: Task<Void, Never>?

    var body: some View {
        if model.device == nil {
            ContentUnavailableView(
                "No iPod Connected",
                systemImage: "ipod",
                description: Text("Plug in your iPod to see its status here.")
            )
        } else {
            Form {
                Section {
                    LabeledContent("iPod", value: model.device?.name ?? model.device?.model ?? "—")
                    if let s = model.storageText { LabeledContent("Capacity", value: s) }
                    LabeledContent("Synced", value: syncedSummary)
                    if let last = model.lastSync {
                        LabeledContent("Last sync", value: shortDate(last.timestamp))
                    }
                }
                Section("Sync") {
                    Toggle("Sync automatically on plug-in", isOn: Binding(
                        get: { autoSync }, set: { autoSync = $0; scheduleSave() }))
                    Toggle("Rockbox compatibility (embed tags & art)", isOn: Binding(
                        get: { rockboxCompat }, set: { rockboxCompat = $0; scheduleSave() }))
                    Button("Update artwork & metadata", action: onBackfill)
                }
                Section {
                    Button("Remove this iPod", role: .destructive, action: onForgetIpod)
                }
            }
            .formStyle(.grouped)
            .onAppear(perform: syncFromConfig)
            .onChange(of: model.config) { _, _ in syncFromConfig() }
        }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
    }

    private func syncFromConfig() {
        guard let d = model.config?.daemon else { return }
        autoSync = d.enabled
        rockboxCompat = d.rockboxCompat
    }

    /// Debounced save that preserves the config fields this view doesn't edit
    /// (source, schedule, launch-at-login, notify) — same pattern as
    /// `SettingsView.GeneralTab.scheduleSave()`.
    private func scheduleSave() {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            let cur = model.config?.daemon
            let daemon = DaemonSettings(
                enabled: autoSync,
                autostartWithWindows: cur?.autostartWithWindows ?? false,
                firstSyncMode: cur?.firstSyncMode ?? "auto_apply",
                subsequentSyncMode: cur?.subsequentSyncMode ?? "auto_apply",
                scheduleMinutes: cur?.scheduleMinutes ?? 0,
                notifyOn: cur?.notifyOn ?? "all",
                rockboxCompat: rockboxCompat)
            onSaveSettings(nil, daemon)   // nil source: don't disturb the folder
        }
    }

    private func shortDate(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
}
