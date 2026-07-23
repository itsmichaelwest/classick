import SwiftUI

struct MenuBarLabelPresentation: Equatable {
  var systemImage: String
  var accessibilityLabel: String

  static func make(phase: DevicePhase?) -> Self {
    switch phase {
    case nil, .disconnected, .unconfigured, .idle:
      make(activity: .idle)
    case .syncing:
      make(activity: .syncing)
    case .paused:
      make(activity: .paused)
    case .error:
      make(activity: .error)
    }
  }

  static func make(globalPhase: Phase, device: DeviceViewState?) -> Self {
    if device?.finalization != nil {
      return make(activity: .finalizing)
    }
    if case .scanning = globalPhase {
      return make(activity: .scanning)
    }
    if let device {
      return make(phase: device.phase)
    }

    switch globalPhase {
    case .noDevice, .notConfigured, .idle:
      return make(activity: .idle)
    case .syncing:
      return make(activity: .syncing)
    case .scanning:
      return make(activity: .scanning)
    case .paused:
      return make(activity: .paused)
    case .error:
      return make(activity: .error)
    }
  }

  static func make(globalPhase: Phase, devices: [DeviceID: DeviceViewState]) -> Self {
    if devices.values.contains(where: { $0.finalization != nil }) {
      return make(activity: .finalizing)
    }
    let connected = devices.values.filter(\.connected)
    if connected.contains(where: { $0.phase == .syncing }) {
      return make(activity: .syncing)
    }
    if connected.contains(where: { $0.phase == .paused }) {
      return make(activity: .paused)
    }
    if case .scanning = globalPhase {
      return make(activity: .scanning)
    }
    if connected.contains(where: {
      if case .error = $0.phase { true } else { false }
    }) {
      return make(activity: .error)
    }
    return make(activity: .idle)
  }

  fileprivate enum Activity {
    case idle
    case syncing
    case finalizing
    case paused
    case scanning
    case error
  }

  fileprivate static func make(activity: Activity) -> Self {
    let systemImage = switch activity {
    case .idle: "ipod"
    case .syncing, .finalizing: "arrow.triangle.2.circlepath"
    case .paused: "pause.circle"
    case .scanning: "magnifyingglass"
    case .error: "exclamationmark.triangle"
    }
    return Self(systemImage: systemImage, accessibilityLabel: "Classick")
  }
}

enum MenuBarLabelLayout {
  static let opticalFrameWidth: CGFloat = 18
  static let opticalFrameHeight: CGFloat = 18
  static let symbolPointSize: CGFloat = 14
  static let symbolWeight: Font.Weight = .medium
}

struct MenuBarLabel: View {
  var presentation: MenuBarLabelPresentation

  var body: some View {
    Image(systemName: presentation.systemImage)
      .symbolRenderingMode(.monochrome)
      .font(
        .system(
          size: MenuBarLabelLayout.symbolPointSize,
          weight: MenuBarLabelLayout.symbolWeight))
      .frame(
        width: MenuBarLabelLayout.opticalFrameWidth,
        height: MenuBarLabelLayout.opticalFrameHeight)
      .accessibilityLabel(Text(presentation.accessibilityLabel))
  }
}

#if DEBUG
  private func menuBarLabelPreview(_ activity: MenuBarLabelPresentation.Activity) -> some View {
    MenuBarLabel(presentation: .make(activity: activity))
      .padding(8)
  }

  #Preview("Menu label — Idle") { menuBarLabelPreview(.idle) }
  #Preview("Menu label — Syncing") { menuBarLabelPreview(.syncing) }
  #Preview("Menu label — Finalizing") { menuBarLabelPreview(.finalizing) }
  #Preview("Menu label — Paused") { menuBarLabelPreview(.paused) }
  #Preview("Menu label — Scanning") { menuBarLabelPreview(.scanning) }
  #Preview("Menu label — Error") { menuBarLabelPreview(.error) }
#endif
