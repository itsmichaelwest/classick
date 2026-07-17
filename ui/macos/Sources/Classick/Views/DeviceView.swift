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
    var onSaveIpodSelection: (Bool) -> Void = { _ in }
    var onReplaceLibrary: () -> Void = {}

    @State private var autoSync = true
    @State private var rockboxCompat = false
    @State private var customSelection = false
    @State private var saveTask: Task<Void, Never>?
    @State private var showReplaceConfirm = false

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
                    Picker("Selection", selection: Binding(
                        get: { customSelection },
                        set: { newValue in
                            customSelection = newValue
                            onSaveIpodSelection(newValue)
                        }
                    )) {
                        Text("Shared").tag(false)
                        Text("Custom for this iPod").tag(true)
                    }
                }
                Section {
                    Button("Replace Library…", role: .destructive) { showReplaceConfirm = true }
                        .disabled(isSyncOrScanRunning)
                    Text("Erases everything on this iPod and re-syncs your current selection from scratch.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section {
                    Button("Remove this iPod", role: .destructive, action: onForgetIpod)
                }
            }
            .formStyle(.grouped)
            .onAppear(perform: syncFromConfig)
            .onChange(of: model.config) { _, _ in syncFromConfig() }
            .sheet(isPresented: $showReplaceConfirm) {
                ReplaceLibraryConfirmationSheet(
                    deviceName: deviceName,
                    syncedCount: model.syncedCount,
                    onConfirm: {
                        onReplaceLibrary()
                        showReplaceConfirm = false
                    },
                    onCancel: { showReplaceConfirm = false }
                )
            }
        }
    }

    private var deviceName: String {
        model.device?.name ?? model.device?.model ?? "this iPod"
    }

    /// First line of defense against a Replace request racing an in-flight
    /// sync/scan (see `DaemonCommand.replaceLibrary`'s doc comment: the
    /// daemon rejects with `sync_rejected` in that case, which the reducer
    /// already surfaces via `Phase.error` — this just avoids sending it in
    /// the first place).
    private var isSyncOrScanRunning: Bool {
        switch model.phase {
        case .syncing, .scanning: return true
        default: return false
        }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
    }

    private func syncFromConfig() {
        if let d = model.config?.daemon {
            autoSync = d.enabled
            rockboxCompat = d.rockboxCompat
        }
        customSelection = model.config?.ipod?.customSelection ?? false
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

/// Typed-confirmation gate for "Replace Library…" (Task 17): the Confirm
/// button only arms once the user has typed the device's exact name,
/// case-sensitive — the same pattern GitHub uses for "delete this
/// repository". Pure + `static` so it's unit-testable without a view.
enum ReplaceConfirmation {
    static func isArmed(input: String, deviceName: String) -> Bool {
        !deviceName.isEmpty && input == deviceName
    }
}

/// "Replace Library…" confirmation sheet. Cancel is always available; Confirm
/// is disabled until `ReplaceConfirmation.isArmed` — see that type's doc
/// comment. Sending `replace_library` itself is the caller's job
/// (`onConfirm`); this view only gates the click.
private struct ReplaceLibraryConfirmationSheet: View {
    var deviceName: String
    var syncedCount: Int
    var onConfirm: () -> Void
    var onCancel: () -> Void

    @State private var input = ""

    private var isArmed: Bool {
        ReplaceConfirmation.isArmed(input: input, deviceName: deviceName)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Replace Library on “\(deviceName)”?")
                .font(.headline)
            Text("This removes all \(syncedCount) tracks currently on “\(deviceName)”, then syncs your current selection.")
                .fixedSize(horizontal: false, vertical: true)
            Text("Type “\(deviceName)” to confirm.")
                .font(.footnote)
                .foregroundStyle(.secondary)
            TextField("Device name", text: $input)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Replace Library", role: .destructive, action: onConfirm)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!isArmed)
            }
        }
        .padding(20)
        .frame(width: 380)
    }
}
