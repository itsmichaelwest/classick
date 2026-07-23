import Foundation

extension AppDelegate {
    static func setupIpodIdentity(
        device: DeviceState?,
        preservingCustomSelection customSelection: Bool
    ) -> IpodIdentity? {
        guard let device else { return nil }
        return IpodIdentity(
            serial: device.serial, modelLabel: device.model,
            name: device.name, customSelection: customSelection)
    }

    static func setupDeviceCommands(
        source: String,
        serial: DeviceID,
        current: DeviceConfigState,
        autoSync: Bool,
        transcodeProfile: TranscodeProfile = .alac,
        requestID: UUID,
        selectionMutationID: UUID,
        settingsMutationID: UUID,
        subscriptionsMutationID: UUID
    ) -> [WireV3Command] {
        let settings = DeviceSettingsWire(
            autoSync: autoSync, rockboxCompat: current.settings.rockboxCompat,
            transcodeProfile: transcodeProfile)
        return [
            .setSourceLocation(requestID: WireV3Command.newRequestID(), sourceRoot: source),
            .adoptDevice(
                deviceID: serial, requestID: requestID,
                selectionMutationID: selectionMutationID,
                selection: WireV3SelectionValue(current.selection),
                settingsMutationID: settingsMutationID,
                settings: WireV3SettingsValue(settings),
                subscriptionsMutationID: subscriptionsMutationID,
                subscriptions: WireV3SubscriptionsValue(current.subscriptions)),
        ]
    }

    static func withCustomSelection(
        _ customSelection: Bool,
        from existing: IpodIdentity?
    ) -> IpodIdentity? {
        guard let existing else { return nil }
        return IpodIdentity(
            serial: existing.serial, modelLabel: existing.modelLabel,
            name: existing.name, customSelection: customSelection)
    }
}

extension ProcessInfo {
    static var isRunningInXcodePreviews: Bool {
        processInfo.environment["XCODE_RUNNING_FOR_PREVIEWS"] == "1"
    }
}

func menuBarSystemImage(for phase: Phase) -> String {
    switch phase {
    case .noDevice, .notConfigured, .idle: "ipod"
    case .syncing: "arrow.triangle.2.circlepath"
    case .scanning: "magnifyingglass"
    case .paused: "pause.circle"
    case .error: "exclamationmark.triangle"
    }
}
