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

    static func setupDaemonSettings(
        autoSync: Bool,
        preservingRockboxCompat rockboxCompat: Bool
    ) -> DaemonSettings {
        DaemonSettings(
            enabled: autoSync,
            autostartWithWindows: false,
            firstSyncMode: "auto_apply",
            subsequentSyncMode: "auto_apply",
            scheduleMinutes: 0,
            notifyOn: "all",
            rockboxCompat: rockboxCompat)
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
