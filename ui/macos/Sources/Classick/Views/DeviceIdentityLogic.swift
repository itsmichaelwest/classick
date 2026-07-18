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
}
