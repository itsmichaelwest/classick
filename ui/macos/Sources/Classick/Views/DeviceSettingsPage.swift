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
/// `device_config` reply, never re-seed after the user starts
/// editing (so a late/echoed reply can't clobber an in-progress edit).
struct DeviceSettingsPage: View {
  var model: AppModel
  var serial: DeviceID
  var onLoadDeviceConfig: (DeviceID) -> Void
  var onSaveDeviceSettings:
    (_ serial: DeviceID, _ settings: DeviceSettingsWire) -> DeviceMutationReceipt?
  var onForgetIpod: (DeviceID) -> Void
  var onBackfill: (DeviceID) -> Void
  var onReplaceLibrary: (DeviceID) -> Void

  private struct SettingsDraft: Equatable {
    var autoSync = true
    var rockboxCompat = false
    var transcodeProfile: TranscodeProfile = .alac
  }

  @State private var draft = SettingsDraft()
  @State private var hasCanonicalDraft = false
  @State private var saveTask: Task<Void, Never>?
  @State private var showReplaceConfirm = false

  private var deviceState: DeviceViewState? {
    DeviceSurfaceLogic.state(serial: serial, in: model.devices)
  }
  private var config: DeviceConfigState? {
    model.editableDeviceConfig(for: serial) ?? deviceState?.config
  }
  private var configStatus: DeviceConfigComponentStatus {
    model.deviceConfigStatus(for: serial, component: .settings)
  }
  private var canEditDevice: Bool { model.canSendDeviceCommand(to: serial) }
  private var isConnected: Bool { deviceState?.connected == true }
  private var surfacePhase: Phase {
    DeviceSurfaceLogic.phase(for: deviceState, globalPhase: model.phase)
  }
  private var deviceName: String {
    guard let deviceState else { return "iPod" }
    return DeviceIdentityLogic.title(identity: deviceState.identity, hardware: deviceState.hardware)
  }
  private var readinessGuidance: DeviceReadinessGuidance? {
    deviceState.flatMap { DeviceReadinessLogic.guidance(for: $0.readiness) }
  }
  private var syncedSummary: String {
    if let total = deviceState?.libraryCount {
      return "\(deviceState?.syncedCount ?? 0) of \(total)"
    }
    return "\(deviceState?.syncedCount ?? 0)"
  }

  var body: some View {
    Group {
      if let readinessGuidance {
        DeviceReadinessView(guidance: readinessGuidance)
      } else {
        settingsForm
      }
    }
    .navigationTitle(deviceName)
    // Same dual-coverage rationale as `DeviceMusicPage`: `.task(id:)`
    // covers a config already cached from a prior visit this launch
    // (seed fires immediately); the `.onChange` covers the reply
    // arriving after this view appears.
    .task(id: serial) {
      guard canEditDevice else { return }
      seedIfNeeded()
      onLoadDeviceConfig(serial)
      submitPendingChanges()
    }
    .onChange(of: canEditDevice) { _, isAvailable in
      handleDeviceAvailabilityChange(isAvailable)
    }
    .onChange(of: config?.settings) { _, _ in seedIfNeeded() }
    .onChange(of: deviceState?.settingsRevision) { _, _ in seedIfNeeded() }
    .onDisappear { submitPendingChanges() }
    .sheet(isPresented: $showReplaceConfirm) {
      ReplaceLibraryConfirmationSheet(
        deviceName: deviceName,
        syncedCount: deviceState?.syncedCount ?? 0,
        onConfirm: {
          onReplaceLibrary(serial)
          showReplaceConfirm = false
        },
        onCancel: { showReplaceConfirm = false }
      )
    }
  }

  private var settingsForm: some View {
    Form {
      if configStatus.message != nil {
        Section { DeviceConfigStatusView(status: configStatus) }
      }
      Section {
        LabeledContent("Name", value: deviceName)
        if let hardware = deviceState.flatMap({ DeviceIdentityLogic.hardwareDescription($0.hardware) }) {
          LabeledContent("Model", value: hardware)
        }
        if let capacity = DeviceSurfaceLogic.storageText(deviceState) {
          LabeledContent("Capacity", value: capacity)
        }
        LabeledContent("Synced", value: syncedSummary)
        if let last = model.latestSuccessfulSync(for: serial) {
          LabeledContent("Last synced", value: shortDate(last.timestamp))
        } else {
          LabeledContent("Last synced", value: "Never synced")
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
        if hasCanonicalDraft {
          Toggle(
            "Sync automatically when connected",
            isOn: Binding(
              get: { draft.autoSync }, set: { value in edit { $0.autoSync = value } })
          )
          .disabled(!canEditDevice)
          Toggle(
            "Rockbox compatibility mode",
            isOn: Binding(
              get: { draft.rockboxCompat }, set: { value in edit { $0.rockboxCompat = value } })
          )
          .disabled(!canEditDevice)
          Picker(
            "Music format",
            selection: Binding(
              get: { draft.transcodeProfile },
              set: { value in edit { $0.transcodeProfile = value } })
          ) {
            ForEach(TranscodeProfile.allCases) { profile in
              Text(profile.title).tag(profile)
            }
          }
          .pickerStyle(.menu)
          .disabled(!canEditDevice)
        } else {
          HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading settings…").foregroundStyle(.secondary)
          }
        }
        LabeledContent("Force update artwork and metadata") {
          // Acts on the physically connected iPod immediately —
          // meaningless without one.
          Button("Update Now") { onBackfill(serial) }
            .disabled(!isConnected || !canEditDevice)
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
            .disabled(
              DeviceSettingsLogic.isReplaceLibraryDisabled(
                phase: surfacePhase, isConnected: isConnected) || !canEditDevice)
        }
        .labeledContentStyle(.centerAligned)
      }
      Section {
        LabeledContent(DeviceSettingsLogic.removeCaption(deviceName: deviceName)) {
          Button("Remove iPod", role: .destructive) { onForgetIpod(serial) }
            .disabled(!canEditDevice)
        }
        .labeledContentStyle(.centerAligned)
      }
    }
    .formStyle(.grouped)
  }

  private func shortDate(_ iso: String) -> String {
    guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
    return d.formatted(date: .abbreviated, time: .shortened)
  }

  /// Reconciles persisted settings while preserving newer local edits.
  private func seedIfNeeded() {
    guard let config else { return }
    draft = SettingsDraft(
      autoSync: config.settings.autoSync,
      rockboxCompat: config.settings.rockboxCompat,
      transcodeProfile: config.settings.transcodeProfile)
    hasCanonicalDraft = true
  }

  /// Debounced auto-save: every toggle edit sends `save_device_config`
  /// with only `settings` populated (selection/subscriptions stay nil —
  /// this page never edits sync intent). Mirrors `DeviceMusicPage`'s
  /// 400ms `scheduleSave`.
  private func scheduleSave() {
    saveTask?.cancel()
    saveTask = Task {
      guard
        await DeviceDraftSaveGate.waitUntilReady(
          serial: serial, model: model)
      else { return }
      submitPendingChanges()
    }
  }

  private func submitPendingChanges() {
    saveTask?.cancel()
    guard canEditDevice, let settings = model.pendingDeviceSettings(for: serial),
      let receipt = onSaveDeviceSettings(serial, settings)
    else { return }
    model.markDeviceSettingsSubmitted(for: serial, receipt: receipt)
  }

  private func edit(_ mutation: (inout SettingsDraft) -> Void) {
    guard hasCanonicalDraft, canEditDevice else { return }
    var edited = draft
    mutation(&edited)
    guard edited != draft else { return }
    draft = edited
    model.editDeviceSettings(
      DeviceSettingsWire(
        autoSync: edited.autoSync,
        rockboxCompat: edited.rockboxCompat,
        transcodeProfile: edited.transcodeProfile),
      for: serial)
    scheduleSave()
  }

  private func handleDeviceAvailabilityChange(_ isAvailable: Bool) {
    guard isAvailable else {
      submitPendingChanges()
      showReplaceConfirm = false
      return
    }
    seedIfNeeded()
    onLoadDeviceConfig(serial)
    submitPendingChanges()
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
  /// Builds the protocol-3 component mutation for a settings edit.
  static func saveSettingsCommand(
    deviceID: DeviceID,
    settings: DeviceSettingsWire,
    requestID: UUID,
    mutationID: UUID
  ) -> WireV3Command {
    .setSettings(
      deviceID: deviceID, requestID: requestID, mutationID: mutationID,
      settings: WireV3SettingsValue(settings))
  }

  /// Replace Library's disabled predicate: guards against racing an
  /// in-flight sync/scan (mirrors the retired `DeviceView`'s
  /// `isSyncOrScanRunning`) — AND (review finding #2) against targeting
  /// the wrong device. `replace_library` targets an explicit serial on the
  /// wire. Earlier this only checked
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
      Text(
        "This removes all \(syncedCount) tracks currently on “\(deviceName)”, then syncs your current selection."
      )
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
      model: PreviewFixtures.connectedSyncedModel(),
      serial: try! DeviceID(PreviewFixtures.pairedIpod.serial),
      onLoadDeviceConfig: { _ in }, onSaveDeviceSettings: { _, _ in .init(requestID: "preview", mutationID: "preview") },
      onForgetIpod: { _ in }, onBackfill: { _ in }, onReplaceLibrary: { _ in }
    )
    .frame(width: 520, height: 520)
  }

  #Preview("Disconnected") {
    DeviceSettingsPage(
      model: PreviewFixtures.disconnectedModel(),
      serial: try! DeviceID(PreviewFixtures.pairedIpod.serial),
      onLoadDeviceConfig: { _ in }, onSaveDeviceSettings: { _, _ in .init(requestID: "preview", mutationID: "preview") },
      onForgetIpod: { _ in }, onBackfill: { _ in }, onReplaceLibrary: { _ in }
    )
    .frame(width: 520, height: 520)
  }
#endif
