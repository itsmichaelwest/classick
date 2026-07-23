import SwiftUI

/// First-run setup: pick the music library folder, confirm the detected
/// iPod, opt into auto-sync, and persist it all in one `save_config`.
/// Reached from the `.notConfigured` menu row ("Set Up Classick…") and
/// auto-presented on first run — see `SetupWindowController`, which hosts
/// this view in an AppKit `NSWindow` (rather than a lazy SwiftUI `Window`
/// scene) so it can be shown deterministically from the app delegate.
/// `onClose` is injected by that controller because a hosted view has no
/// `@Environment(\.dismiss)` window to dismiss.
struct SetupWindow: View {
  var model: AppModel
  var preferredSerial: DeviceID?
  var onDone: (_ source: String, _ autoSync: Bool, _ serial: DeviceID) -> Void
  var onClose: () -> Void

  @State private var pickedPath: String?
  @State private var autoSync = true
  @State private var isPickingFolder = false

  private var candidateDevice: DeviceViewState? {
    if let preferredSerial, let state = model.devices[preferredSerial], state.connected {
      return state
    }
    let connected = model.devices.values.filter(\.connected)
    return connected.count == 1 ? connected[0] : nil
  }

  private var setupDevice: DeviceViewState? {
    guard let candidateDevice, DeviceReadinessLogic.isReady(candidateDevice.readiness) else {
      return nil
    }
    return candidateDevice
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text("Set Up Classick")
        .font(.title2.bold())

      VStack(alignment: .leading, spacing: 6) {
        Text("Music Library")
          .font(.headline)
        HStack {
          Text(pickedPath ?? "No folder selected")
            .foregroundStyle(pickedPath == nil ? .secondary : .primary)
            .lineLimit(1)
            .truncationMode(.middle)
          Spacer()
          Button("Choose…") { isPickingFolder = true }
        }
      }

      VStack(alignment: .leading, spacing: 6) {
        Text("iPod")
          .font(.headline)
        if let candidateDevice,
          let guidance = DeviceReadinessLogic.guidance(for: candidateDevice.readiness)
        {
          Label(guidance.title, systemImage: guidance.systemImage)
          Text(guidance.message)
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        } else if candidateDevice == nil, !model.unidentifiedDevices.isEmpty {
          Label(
            DeviceReadinessLogic.identityUnavailableGuidance.title,
            systemImage: DeviceReadinessLogic.identityUnavailableGuidance.systemImage)
          Text(DeviceReadinessLogic.identityUnavailableGuidance.message)
            .font(.callout)
            .foregroundStyle(.secondary)
        } else {
          Text(
            setupDevice.map {
              DeviceIdentityLogic.title(identity: $0.identity, hardware: $0.hardware)
            } ?? setupDevicePrompt
          )
          .foregroundStyle(setupDevice == nil ? .secondary : .primary)
        }
      }

      Toggle("Sync automatically when plugged in", isOn: $autoSync)
        .disabled(setupDevice == nil)

      Text(
        "Quit Music.app before syncing — iTunes will reject a Classick-managed iPod while it's running."
      )
      .font(.footnote)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      Text("Classick backs up your iPod's database before every sync.")
        .font(.footnote)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      Spacer(minLength: 0)

      HStack {
        Spacer()
        Button("Done") {
          guard let pickedPath, let deviceID = setupDevice?.deviceID else { return }
          onDone(pickedPath, autoSync, deviceID)
          onClose()
        }
        .keyboardShortcut(.defaultAction)
        .disabled(pickedPath == nil || setupDevice == nil)
      }
    }
    .padding(20)
    .frame(width: 420, height: 370)
    .fileImporter(isPresented: $isPickingFolder, allowedContentTypes: [.folder]) { result in
      if case .success(let url) = result {
        pickedPath = url.path
      }
    }
  }

  private var setupDevicePrompt: String {
    model.devices.values.filter(\.connected).count > 1
      ? "Select an iPod in Classick"
      : "Plug in your iPod"
  }
}

#if DEBUG
  #Preview("Device found") {
    SetupWindow(
      model: PreviewFixtures.notConfiguredModel(),
      preferredSerial: try! DeviceID(PreviewFixtures.pairedIpod.serial),
      onDone: { _, _, _ in }, onClose: {})
  }

  #Preview("No device") {
    SetupWindow(
      model: PreviewFixtures.firstRunModel(), preferredSerial: nil,
      onDone: { _, _, _ in }, onClose: {})
  }

  #Preview("Finder initialization required") {
    SetupWindow(
      model: PreviewFixtures.nativeDeviceModel(
        readiness: "needs_apple_initialization", configured: false),
      preferredSerial: PreviewFixtures.nativeDeviceID,
      onDone: { _, _, _ in }, onClose: {})
  }

  #Preview("Identity unavailable") {
    SetupWindow(
      model: PreviewFixtures.unidentifiedDeviceModel(), preferredSerial: nil,
      onDone: { _, _, _ in }, onClose: {})
  }
#endif
