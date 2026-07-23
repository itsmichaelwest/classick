import SwiftUI

struct DeviceConfigStatusView: View {
  var status: DeviceConfigComponentStatus

  var body: some View {
    if let message = status.message {
      HStack(spacing: 6) {
        Image(systemName: systemImage)
        Text(message)
        Spacer(minLength: 0)
      }
      .font(.caption)
      .foregroundStyle(foregroundStyle)
      .padding(.horizontal, 16)
      .padding(.vertical, 6)
      .accessibilityElement(children: .combine)
    }
  }

  private var systemImage: String {
    switch status {
    case .hostAcceptanceFailed, .deviceDeliveryFailed: "exclamationmark.triangle.fill"
    case .waitingForDevice: "ipod"
    case .localDraft, .savingOnHost: "arrow.triangle.2.circlepath"
    case .saved: "checkmark"
    }
  }

  private var foregroundStyle: Color {
    switch status {
    case .hostAcceptanceFailed, .deviceDeliveryFailed: .orange
    default: .secondary
    }
  }
}
