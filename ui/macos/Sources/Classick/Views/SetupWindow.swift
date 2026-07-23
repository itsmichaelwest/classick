import SwiftUI

/// First-run setup: confirm the global music folder and detected iPod, then
/// choose this iPod's sync settings.
/// Reached from the `.notConfigured` menu row ("Set Up Classick…") and
/// auto-presented on first run — see `SetupWindowController`, which hosts
/// this view in an AppKit `NSWindow` (rather than a lazy SwiftUI `Window`
/// scene) so it can be shown deterministically from the app delegate.
/// `onClose` is injected by that controller because a hosted view has no
/// `@Environment(\.dismiss)` window to dismiss.
struct SetupWindow: View {
  var model: AppModel
  var preferredSerial: DeviceID?
  var onDone: (
    _ source: String,
    _ autoSync: Bool,
    _ transcodeProfile: TranscodeProfile,
    _ serial: DeviceID
  ) -> Void
  var onClose: () -> Void

  @State private var pickedPath: String?
  @State private var autoSync = true
  @State private var transcodeProfile: TranscodeProfile = .alac
  @State private var isPickingFolder = false

  private var sourcePath: String? {
    SetupWindowLogic.sourcePath(
      pickedPath: pickedPath,
      configuredPath: model.config?.source)
  }

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
        Text("Music Folder")
          .font(.headline)
        HStack {
          Text(sourcePath ?? "No folder selected")
            .foregroundStyle(sourcePath == nil ? .secondary : .primary)
            .lineLimit(1)
            .truncationMode(.middle)
          Spacer()
          Button("Choose…") { isPickingFolder = true }
        }
        Text("Used for every iPod you sync with this Mac.")
          .font(.caption)
          .foregroundStyle(.secondary)
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

      Toggle("Sync automatically when connected", isOn: $autoSync)
        .disabled(setupDevice == nil)

      Picker("Music format", selection: $transcodeProfile) {
        ForEach(TranscodeProfile.allCases) { profile in
          Text(profile.title).tag(profile)
        }
      }
      .pickerStyle(.menu)
      .disabled(setupDevice == nil)

      Text(
        "Classick will ask you to quit Music before syncing, and creates a recovery snapshot before changing the iPod."
      )
      .font(.footnote)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)

      Spacer(minLength: 0)

      HStack {
        Spacer()
        Button("Set Up") {
          guard let sourcePath, let deviceID = setupDevice?.deviceID else { return }
          onDone(sourcePath, autoSync, transcodeProfile, deviceID)
          onClose()
        }
        .keyboardShortcut(.defaultAction)
        .disabled(sourcePath == nil || setupDevice == nil)
      }
    }
    .padding(20)
    .frame(width: 420, height: 410)
    .fileImporter(isPresented: $isPickingFolder, allowedContentTypes: [.folder]) { result in
      if case .success(let url) = result {
        pickedPath = url.path
      }
    }
  }

  private var setupDevicePrompt: String {
    model.devices.values.filter(\.connected).count > 1
      ? "Select an iPod in Classick"
      : "Connect an iPod"
  }
}

enum SetupWindowLogic {
  static func sourcePath(pickedPath: String?, configuredPath: String?) -> String? {
    if let pickedPath, !pickedPath.isEmpty {
      return pickedPath
    }
    guard let configuredPath, !configuredPath.isEmpty else { return nil }
    return configuredPath
  }
}

#if DEBUG
  #Preview("Device found") {
    SetupWindow(
      model: PreviewFixtures.notConfiguredModel(),
      preferredSerial: try! DeviceID(PreviewFixtures.pairedIpod.serial),
      onDone: { _, _, _, _ in }, onClose: {})
  }

  #Preview("No device") {
    SetupWindow(
      model: PreviewFixtures.firstRunModel(), preferredSerial: nil,
      onDone: { _, _, _, _ in }, onClose: {})
  }

  #Preview("Finder initialization required") {
    SetupWindow(
      model: PreviewFixtures.nativeDeviceModel(
        readiness: "needs_apple_initialization", configured: false),
      preferredSerial: PreviewFixtures.nativeDeviceID,
      onDone: { _, _, _, _ in }, onClose: {})
  }

  #Preview("Identity unavailable") {
    SetupWindow(
      model: PreviewFixtures.unidentifiedDeviceModel(), preferredSerial: nil,
      onDone: { _, _, _, _ in }, onClose: {})
  }
#endif
