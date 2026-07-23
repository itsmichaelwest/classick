import Foundation

struct DeviceMutationReceipt: Equatable, Sendable {
  var requestID: String
  var mutationID: String
}

struct DeviceMusicMutationReceipt: Equatable, Sendable {
  var selection: DeviceMutationReceipt? = nil
  var subscriptions: DeviceMutationReceipt? = nil
}

enum DeviceConfigComponentStatus: Equatable, Sendable {
  case saved
  case localDraft
  case savingOnHost
  case waitingForDevice
  case hostAcceptanceFailed(String)
  case deviceDeliveryFailed(String)

  var message: String? {
    switch self {
    case .saved, .localDraft, .savingOnHost, .waitingForDevice: nil
    case .hostAcceptanceFailed(let message): "Couldn’t save on this Mac: \(message)"
    case .deviceDeliveryFailed(let message): "Saved on this Mac — \(message)"
    }
  }
}

struct DeviceConfigEditingState: Equatable {
  var selection: AcknowledgedDraft<SelectionState>
  var settings: AcknowledgedDraft<DeviceSettingsWire>
  var subscriptions: AcknowledgedDraft<SubscriptionsWire>

  init(config: DeviceConfigState, state: DeviceViewState) {
    selection = AcknowledgedDraft(canonical: config.selection, revision: state.selectionRevision)
    settings = AcknowledgedDraft(canonical: config.settings, revision: state.settingsRevision)
    subscriptions = AcknowledgedDraft(
      canonical: config.subscriptions, revision: state.subscriptionsRevision)
  }

  var value: DeviceConfigState {
    DeviceConfigState(
      selection: selection.value,
      subscriptions: subscriptions.value,
      settings: settings.value,
      preview: nil)
  }

  mutating func prepareForProtocolReconnect() {
    selection.prepareForProtocolReconnect()
    settings.prepareForProtocolReconnect()
    subscriptions.prepareForProtocolReconnect()
  }

  func status(
    for component: DeviceConfigComponent,
    delivery: DeviceConfigDeliveryState?
  ) -> DeviceConfigComponentStatus {
    switch component {
    case .selection:
      componentStatus(draft: selection, delivery: delivery?.selection.delivery)
    case .settings:
      componentStatus(draft: settings, delivery: delivery?.settings.delivery)
    case .subscriptions:
      componentStatus(draft: subscriptions, delivery: delivery?.subscriptions.delivery)
    }
  }

  private func componentStatus<Value: Equatable>(
    draft: AcknowledgedDraft<Value>, delivery: WireV3Delivery?
  ) -> DeviceConfigComponentStatus {
    if let failure = draft.hostAcceptanceFailure { return .hostAcceptanceFailed(failure) }
    if draft.hasUnsubmittedChanges { return .localDraft }
    if draft.hasPendingSubmission { return .savingOnHost }
    guard let delivery else { return .saved }
    if let failure = delivery.lastFailure { return .deviceDeliveryFailed(failure) }
    return delivery.state == "pending_device" ? .waitingForDevice : .saved
  }
}

enum DeviceConfigComponent: String, Equatable, Sendable {
  case selection
  case settings
  case subscriptions
}

enum DeviceConfigStatusLogic {
  static func mostImportant(
    _ statuses: [DeviceConfigComponentStatus]
  ) -> DeviceConfigComponentStatus {
    statuses.max(by: { priority($0) < priority($1) }) ?? .saved
  }

  private static func priority(_ status: DeviceConfigComponentStatus) -> Int {
    switch status {
    case .saved: 0
    case .waitingForDevice: 1
    case .localDraft: 2
    case .savingOnHost: 3
    case .deviceDeliveryFailed: 4
    case .hostAcceptanceFailed: 5
    }
  }
}
