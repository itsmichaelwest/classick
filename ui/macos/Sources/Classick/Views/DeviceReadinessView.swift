import SwiftUI

struct DeviceReadinessView: View {
  var guidance: DeviceReadinessGuidance

  var body: some View {
    ContentUnavailableView {
      Label(guidance.title, systemImage: guidance.systemImage)
    } description: {
      Text(guidance.message)
    }
    .accessibilityElement(children: .combine)
  }
}

#if DEBUG
  #Preview("Needs Apple initialization") {
    DeviceReadinessView(
      guidance: DeviceReadinessLogic.guidance(for: "needs_apple_initialization")!)
      .frame(width: 520, height: 320)
  }

  #Preview("Invalid database") {
    DeviceReadinessView(guidance: DeviceReadinessLogic.guidance(for: "invalid_database")!)
      .frame(width: 520, height: 320)
  }

  #Preview("Identity unavailable") {
    DeviceReadinessView(guidance: DeviceReadinessLogic.identityUnavailableGuidance)
      .frame(width: 520, height: 320)
  }
#endif
