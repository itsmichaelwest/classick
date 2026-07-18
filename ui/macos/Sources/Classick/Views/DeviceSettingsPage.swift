import SwiftUI

/// The device Settings page (Task 6, Figma frame `4:6349`) — per-device
/// preferences plus destructive/removal actions, split out of the retired
/// `DeviceView` dashboard now that the sidebar routes Music (Task 5, sync
/// intent) and Settings (here) to separate canonical pages. This page never
/// touches selection rules or playlist subscriptions — Global Constraints:
/// "canonical-surface (no sync-intent editing here — that's the Music
/// page)" — every toggle edit here sends `save_device_config` with ONLY
/// `settings` populated (see `DeviceSettingsLogic.saveSettingsCommand`).
///
/// Edits a local draft and auto-saves it (debounced), mirroring
/// `DeviceMusicPage`'s pattern: seed the draft once from the daemon's
/// `device_config_update` reply, never re-seed after the user starts
/// editing (so a late/echoed reply can't clobber an in-progress edit).
struct DeviceSettingsPage: View {
    var model: AppModel
    var serial: String
    var onLoadDeviceConfig: (String) -> Void
    var onSaveDeviceSettings: (_ serial: String, _ settings: DeviceSettingsWire) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void
    var onReplaceLibrary: () -> Void

    private struct SettingsDraft: Equatable {
        var autoSync = true
        var rockboxCompat = false
    }

    @State private var draft = SettingsDraft()
    @State private var seededFromModel = false
    @State private var userEdited = false
    /// True only for the seed's own draft assignment, so `.onChange(of:
    /// draft)` can tell "the seed just landed" apart from a real user edit
    /// — without this, merely opening the page fired a save.
    @State private var isSeeding = false
    @State private var saveTask: Task<Void, Never>?
    @State private var showReplaceConfirm = false

    private var config: DeviceConfigState? { model.deviceConfigs[serial] }
    private var isConnected: Bool { model.device?.serial == serial }
    /// See `DeviceMusicPage.isKnownDevice` — on-disk facts (last sync,
    /// synced count) belong to the paired device regardless of connection;
    /// only a page for some OTHER device must placeholder them.
    private var isKnownDevice: Bool {
        serial == (model.device?.serial ?? model.config?.ipod?.serial)
    }
    private var deviceName: String {
        DeviceIdentityLogic.deviceName(serial: serial, isConnected: isConnected, connectedDevice: model.device, pairedIpod: model.config?.ipod)
    }
    private var syncedSummary: String {
        // Keyed on isKnownDevice, not isConnected: the manifest count is a
        // persisted per-paired-device fact, valid while unplugged.
        DeviceIdentityLogic.syncedSummaryText(isConnected: isKnownDevice, syncedCount: model.syncedCount, libraryCount: model.libraryCount)
    }

    var body: some View {
        Form {
            Section {
                LabeledContent("Name", value: deviceName)
                if let capacity = DeviceIdentityLogic.capacityText(isConnected: isConnected, storageText: model.storageText) {
                    LabeledContent("Capacity", value: capacity)
                }
                LabeledContent("Synced", value: syncedSummary)
                // Shown for the KNOWN (paired) device even while
                // disconnected — a fact on disk, not a connection property.
                // Only some OTHER device's page placeholders it.
                if isKnownDevice, let last = model.lastSync {
                    LabeledContent("Last synced", value: shortDate(last.timestamp))
                } else if isKnownDevice {
                    LabeledContent("Last synced", value: "Never synced")
                } else {
                    LabeledContent("Last synced", value: DeviceIdentityLogic.placeholder)
                }
                if let caption = DeviceSettingsLogic.caption(isConnected: isConnected) {
                    Text(caption).font(.caption).foregroundStyle(.secondary)
                }
            }
            Section {
                // Toggles render ONLY once the draft is seeded from the
                // daemon's reply. Rendering before that showed the draft's
                // compiled-in defaults (autoSync=true), which visibly
                // snapped to the persisted values a beat later — reported
                // as "the toggle turns itself off when I open the page."
                // A short placeholder is honest; a wrong toggle isn't.
                if seededFromModel {
                    Toggle("Sync automatically when connected", isOn: Binding(
                        get: { draft.autoSync }, set: { draft.autoSync = $0 }))
                    // Disabled while disconnected (user decision, overriding
                    // the earlier stays-editable rule for THIS toggle):
                    // Rockbox mode implies an on-device format change, so
                    // flipping it with no iPod present promises work the
                    // app can't start.
                    Toggle("Rockbox compatibility mode", isOn: Binding(
                        get: { draft.rockboxCompat }, set: { draft.rockboxCompat = $0 }))
                        .disabled(!isConnected)
                } else {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Loading settings…").foregroundStyle(.secondary)
                    }
                }
                LabeledContent("Force update artwork and metadata") {
                    // Acts on the physically connected iPod immediately —
                    // meaningless without one.
                    Button("Update Now", action: onBackfill)
                        .disabled(!isConnected)
                }
                .labeledContentStyle(.centerAligned)
            }
            Section {
                LabeledContent("Erase iPod and re-sync current selection") {
                    // `role: .destructive` alone doesn't colorize a bordered
                    // button on macOS (role tinting applies in menus/alerts);
                    // bordered + red tint is the System Settings idiom for
                    // destructive form buttons. Role kept for semantics
                    // (menus, accessibility, confirmation dialogs).
                    Button("Replace Library…", role: .destructive) { showReplaceConfirm = true }
                        .buttonStyle(.bordered)
                        .tint(.red)
                        .disabled(DeviceSettingsLogic.isReplaceLibraryDisabled(phase: model.phase, isConnected: isConnected))
                }
                .labeledContentStyle(.centerAligned)
            }
            Section {
                LabeledContent(DeviceSettingsLogic.removeCaption(deviceName: deviceName)) {
                    Button("Remove iPod", role: .destructive, action: onForgetIpod)
                }
                .labeledContentStyle(.centerAligned)
            }
        }
        .formStyle(.grouped)
        .navigationTitle(deviceName)
        // Same dual-coverage rationale as `DeviceMusicPage`: `.task(id:)`
        // covers a config already cached from a prior visit this launch
        // (seed fires immediately); the `.onChange` covers the reply
        // arriving after this view appears.
        .task(id: serial) {
            seedIfNeeded()
            onLoadDeviceConfig(serial)
        }
        .onChange(of: config?.settings) { _, _ in seedIfNeeded() }
        .onChange(of: draft) { _, newDraft in
            // The seed's own assignment lands here too — it must NOT count
            // as a user edit or trigger a save (opening the page used to
            // write device config for no reason).
            if isSeeding { isSeeding = false; return }
            userEdited = true
            scheduleSave(newDraft)
        }
        // See `DeviceMusicPage`'s identical `.onDisappear` for the
        // rationale — cancels an in-flight debounced save the instant this
        // page is navigated away from.
        .onDisappear { saveTask?.cancel() }
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

    private func shortDate(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }

    /// Seeds the local draft from the persisted per-device settings exactly
    /// once, and never after the user has started editing.
    private func seedIfNeeded() {
        guard !seededFromModel, !userEdited, let config else { return }
        isSeeding = true
        draft = SettingsDraft(autoSync: config.settings.autoSync, rockboxCompat: config.settings.rockboxCompat)
        seededFromModel = true
    }

    /// Debounced auto-save: every toggle edit sends `save_device_config`
    /// with only `settings` populated (selection/subscriptions stay nil —
    /// this page never edits sync intent). Mirrors `DeviceMusicPage`'s
    /// 400ms `scheduleSave`.
    private func scheduleSave(_ d: SettingsDraft) {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            onSaveDeviceSettings(serial, DeviceSettingsWire(autoSync: d.autoSync, rockboxCompat: d.rockboxCompat))
        }
    }
}

/// `LabeledContent`'s automatic style aligns label and content on
/// `firstTextBaseline` — right for text-value rows, but a bordered Button's
/// baseline is its TITLE's baseline, so the button chrome hangs below it and
/// the button renders visually low in the row. This style keeps the standard
/// structure (label leading, content trailing) and centers vertically.
/// `LabeledContentStyle` is the system's own customization protocol — this
/// is styling, not a custom control. Scoped to button rows only: text-value
/// rows keep the automatic style (and its secondary-colored values).
struct CenterAlignedLabeledContentStyle: LabeledContentStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .center) {
            configuration.label
            Spacer()
            configuration.content
        }
    }
}

extension LabeledContentStyle where Self == CenterAlignedLabeledContentStyle {
    static var centerAligned: CenterAlignedLabeledContentStyle { .init() }
}

/// Pure logic backing `DeviceSettingsPage` — no SwiftUI, fully
/// unit-testable. Follows `DeviceMusicLogic`'s pattern of a plain static-fn
/// enum alongside its view.
enum DeviceSettingsLogic {
    /// THE load-bearing function for this page: builds the
    /// `save_device_config` command for a toggle edit, touching ONLY
    /// `settings` — `selection`/`subscriptions` stay `nil` ("don't change"),
    /// so a Settings-page edit can never disturb the Music page's sync
    /// intent. Returns the real `DaemonCommand` (not a bespoke struct) so
    /// tests can assert the exact wire shape via `JSONSerialization`, same
    /// as `WireCodecTests`.
    static func saveSettingsCommand(serial: String, settings: DeviceSettingsWire) -> DaemonCommand {
        .saveDeviceConfig(serial: serial, selection: nil, subscriptions: nil, settings: settings)
    }

    /// Replace Library's disabled predicate: guards against racing an
    /// in-flight sync/scan (mirrors the retired `DeviceView`'s
    /// `isSyncOrScanRunning`) — AND (review finding #2) against targeting
    /// the wrong device. `replace_library` carries no serial on the wire —
    /// it wipes whichever iPod is physically connected right now, not
    /// necessarily the one this page represents. Earlier this only checked
    /// `phase`, on the theory that the daemon's `sync_rejected` would catch
    /// a disconnected Replace; that reasoning doesn't hold for "the WRONG
    /// device is connected", since the daemon has no way to know it's wrong
    /// — it just wipes whatever's plugged in. So `isConnected` (this page's
    /// serial == the connected device's serial) must hold too.
    static func isReplaceLibraryDisabled(phase: Phase, isConnected: Bool) -> Bool {
        guard isConnected else { return true }
        switch phase {
        case .syncing, .scanning: return true
        default: return false
        }
    }

    /// Disconnected-device caption (Global Constraints exact string) — `nil`
    /// when connected so the page renders no extra line.
    static func caption(isConnected: Bool) -> String? {
        isConnected ? nil : "Not connected — changes apply on next sync"
    }

    /// "Remove {name} from Classick" caption above the Remove iPod button.
    static func removeCaption(deviceName: String) -> String {
        "Remove \(deviceName) from Classick"
    }
}

/// Typed-confirmation gate for "Replace Library…" — the Confirm button only
/// arms once the user has typed the device's exact name, case-sensitive —
/// the same pattern GitHub uses for "delete this repository". Relocated
/// verbatim from the retired `DeviceView` (Task 6); pure + `static` so it's
/// unit-testable without a view.
enum ReplaceConfirmation {
    static func isArmed(input: String, deviceName: String) -> Bool {
        !deviceName.isEmpty && input == deviceName
    }
}

/// "Replace Library…" confirmation sheet. Cancel is always available; Confirm
/// is disabled until `ReplaceConfirmation.isArmed` — see that type's doc
/// comment. Sending `replace_library` itself is the caller's job
/// (`onConfirm`); this view only gates the click. Relocated verbatim from
/// the retired `DeviceView` (Task 6).
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

#if DEBUG
#Preview("Connected") {
    DeviceSettingsPage(
        model: PreviewFixtures.connectedSyncedModel(), serial: PreviewFixtures.pairedIpod.serial,
        onLoadDeviceConfig: { _ in }, onSaveDeviceSettings: { _, _ in },
        onForgetIpod: {}, onBackfill: {}, onReplaceLibrary: {})
        .frame(width: 520, height: 520)
}

#Preview("Disconnected") {
    DeviceSettingsPage(
        model: PreviewFixtures.disconnectedModel(), serial: PreviewFixtures.pairedIpod.serial,
        onLoadDeviceConfig: { _ in }, onSaveDeviceSettings: { _, _ in },
        onForgetIpod: {}, onBackfill: {}, onReplaceLibrary: {})
        .frame(width: 520, height: 520)
}
#endif
