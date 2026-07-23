import Foundation

/// Shared pure logic for gating `AppModel`'s singleton connected-device
/// fields (`device`, `deviceStorage`, `lastSync`, `syncedCount`, `phase`) on
/// whether the page currently being viewed actually IS the connected
/// device. Used by both `DeviceMusicPage` and `DeviceSettingsPage` (review
/// finding #2): those fields describe whichever iPod is physically plugged
/// in right now, keyed by nothing — with a DIFFERENT iPod connected than the
/// device page being viewed, showing them unconditionally puts one device's
/// name/capacity/synced-count/last-synced on another device's page, and
/// (per `DeviceMusicLogic.isSyncNowDisabled`/`DeviceSettingsLogic.isReplaceLibraryDisabled`)
/// can let a destructive action target the wrong iPod entirely.
enum DeviceIdentityLogic {
    /// Shown in place of a singleton field's real value when the page's
    /// device isn't the connected one — there's no live reading for a
    /// device that isn't plugged in right now.
    static let placeholder = "—"

    /// Device name: the connected device's own identity when this page IS
    /// the connected device (mirrors the pre-fix fallback chain exactly);
    /// otherwise the daemon's cached paired identity, but ONLY when it
    /// matches this page's serial (paired but not plugged in right now);
    /// otherwise just the bare serial — this page's device is neither
    /// connected nor the one Classick has any cached identity for, so
    /// there's nothing to show but its serial.
    static func deviceName(serial: String, isConnected: Bool, connectedDevice: DeviceState?, pairedIpod: IpodIdentity?) -> String {
        if isConnected {
            return connectedDevice?.name ?? connectedDevice?.model ?? pairedIpod?.name ?? pairedIpod?.modelLabel ?? "This iPod"
        }
        guard pairedIpod?.serial == serial else { return serial }
        return pairedIpod?.name ?? pairedIpod?.modelLabel ?? serial
    }

    /// `nil` when connected but the reading hasn't arrived yet (the brief
    /// startup race before `deviceStorage`/`storageText` populate) — the
    /// caller omits the row/bar in that case, same as before this fix.
    /// `placeholder` (never the connected device's real text) when this
    /// page's device isn't the connected one.
    static func capacityText(isConnected: Bool, storageText: String?) -> String? {
        guard isConnected else { return placeholder }
        return storageText
    }

    static func syncedSummaryText(isConnected: Bool, syncedCount: Int, libraryCount: Int?) -> String {
        guard isConnected else { return placeholder }
        if let total = libraryCount { return "\(syncedCount) of \(total)" }
        return "\(syncedCount)"
    }

    /// Apple owns the on-device name. A blank name is treated as absent and
    /// falls back to the best hardware description the daemon supplied.
    static func title(identity: DeviceIdentityWire, hardware: WireV3Hardware) -> String {
        if let name = identity.name?.trimmingCharacters(in: .whitespacesAndNewlines),
           !name.isEmpty
        {
            return name
        }
        return hardwareDescription(hardware) ?? "iPod"
    }

    static func hardwareDescription(_ hardware: WireV3Hardware) -> String? {
        guard let familyFact = hardware.family,
              isDeterministic(familyFact),
              let family = nonEmpty(familyFact.value),
              !family.isEmpty
        else { return nil }

        let familyName: String
        switch family.lowercased() {
        case "classic": familyName = "iPod classic"
        case "nano": familyName = "iPod nano"
        case "mini": familyName = "iPod mini"
        case "shuffle": familyName = "iPod shuffle"
        case "video": familyName = "iPod with video"
        case "photo": familyName = "iPod photo"
        case "ipod": familyName = "iPod"
        default: familyName = "iPod \(family)"
        }

        guard let generationFact = hardware.generation,
              isDeterministic(generationFact),
              let generation = nonEmpty(generationFact.value)
        else { return familyName }
        return "\(familyName) (\(ordinalGeneration(generation)) generation)"
    }

    static func accessibilityLabel(identity: DeviceIdentityWire, hardware: WireV3Hardware) -> String {
        let title = title(identity: identity, hardware: hardware)
        guard let description = hardwareDescription(hardware),
              description.caseInsensitiveCompare(title) != .orderedSame
        else { return title }
        return "\(title), \(description)"
    }

    private static func nonEmpty(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func isDeterministic<T: Codable & Equatable & Sendable>(
        _ fact: WireV3HardwareFact<T>
    ) -> Bool {
        fact.confidence == "certain" && (fact.source == "decoded" || fact.source == "reported")
    }

    private static func ordinalGeneration(_ value: String) -> String {
        switch value {
        case "1": return "1st"
        case "2": return "2nd"
        case "3": return "3rd"
        default: return value
        }
    }
}

struct DeviceReadinessGuidance: Equatable, Sendable {
    var title: String
    var message: String
    var systemImage: String
}

enum DeviceReadinessLogic {
    static func isReady(_ readiness: String) -> Bool {
        readiness == "ready"
    }

    /// Unknown readiness values fail closed. A newer daemon must never make
    /// an older client accidentally enable mutation.
    static func guidance(for readiness: String) -> DeviceReadinessGuidance? {
        switch readiness {
        case "ready":
            return nil
        case "needs_apple_initialization":
            return .init(
                title: "Finish setup in Finder",
                message: "Open Finder and set up this iPod with Apple software before using it with Classick.",
                systemImage: "externaldrive.badge.person.crop")
        case "invalid_database":
            return .init(
                title: "This iPod needs recovery",
                message: "Use Finder or iTunes to restore this iPod, then reconnect it before syncing with Classick.",
                systemImage: "externaldrive.badge.exclamationmark")
        case "identity_unavailable":
            return identityUnavailableGuidance
        default:
            return .init(
                title: "This iPod is not ready",
                message: "Classick cannot safely modify this iPod in its current state. Reconnect it or use Finder to check it.",
                systemImage: "exclamationmark.triangle")
        }
    }

    static let identityUnavailableGuidance = DeviceReadinessGuidance(
        title: "iPod identity unavailable",
        message: "Reconnect the iPod. Classick cannot safely identify or modify it.",
        systemImage: "questionmark.circle")
}
