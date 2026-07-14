import Foundation
import Observation

/// Derived UI phase for the menu-bar surface. `.noDevice`/`.notConfigured`
/// take precedence over sync state when deriving from `status_update`, but
/// direct sync progress (`sync_event` lines) always wins once a sync is
/// actually streaming — see AppModel.apply for the precedence rules.
enum Phase: Equatable, Sendable {
    case noDevice
    case notConfigured
    case idle
    case syncing(current: Int, total: Int, label: String)
    case paused(synced: Int, total: Int?)
    case error(String)
}

struct DeviceState: Equatable, Sendable {
    var serial: String
    var model: String
    var name: String?
    var drive: String
}

struct PendingPrompt: Equatable, Sendable {
    var id: UInt64
    var message: String
    var options: [String]
}

/// The daemon's last-known persisted configuration, as pushed by
/// `config_update`. Settings/Setup UI reads this to seed its controls and
/// writes back via `save_config`; the daemon (not this app) remains the
/// store of record.
struct AppConfig: Equatable, Sendable {
    var source: String?
    var daemon: DaemonSettings?
    var ipod: IpodIdentity?
}

@Observable
@MainActor
final class AppModel {
    private(set) var device: DeviceState?
    private(set) var phase: Phase = .noDevice
    private(set) var lastSync: HistoryEntry?
    private(set) var pendingPrompt: PendingPrompt?
    private(set) var storageText: String?
    private(set) var config: AppConfig?
    private(set) var syncedCount: Int = 0
    private(set) var libraryCount: Int?

    // Tracked separately from `device` because `status_update` carries its
    // own `ipod_connected`/`configured` flags independent of the
    // `device_connected`/`device_disconnected` events.
    private var isIpodConnected = false

    // "Configured" is device-aware: the daemon's persisted iPod identity must
    // match the *currently connected* device's serial, not just "some iPod
    // was ever paired". Without this check, swapping in a different,
    // unpaired iPod while a paired one's config is still cached would show
    // "Sync Now" instead of "Set Up Classick…".
    //
    // `configuredSerial`/`hasSeenConfig` come from `config_update` (the
    // source of truth once we've seen one). Before the first `config_update`
    // arrives, `statusConfigured` — the daemon's own device-agnostic
    // `status_update.configured` flag — is used as a fallback so the menu
    // doesn't flash "Set Up Classick…" during the startup handshake.
    private var configuredSerial: String?
    private var hasSeenConfig = false
    private var statusConfigured = false

    private var isConfiguredForCurrentDevice: Bool {
        // Until the config reply lands (`hasSeenConfig`), we don't yet know
        // *which* iPod is paired, so we can't device-match. Trust the daemon's
        // device-agnostic `status_update.configured` flag in that window — this
        // avoids flashing "Set Up Classick…" during the startup handshake AND
        // on every reconnect of an already-configured device (where
        // `status_update` arrives before `config_update`).
        guard hasSeenConfig else { return statusConfigured }
        // Config known but the `device_connected` event hasn't arrived yet:
        // fall back to "is anything paired at all".
        guard let device else { return configuredSerial != nil }
        // Both known: the paired serial must match the connected device, so a
        // swapped-in unpaired iPod correctly shows "Set Up Classick…".
        return device.serial == configuredSerial
    }

    /// The user has never completed first-run setup: the daemon has reported
    /// its persisted config (post-handshake, so `hasSeenConfig`) and it carries
    /// no music-library source. Stays `false` until the config reply lands, so
    /// first-run auto-presentation waits for the handshake instead of firing
    /// during the startup race. The daemon always answers `get_config` — with
    /// an empty `config_update` when nothing is persisted — so this reliably
    /// flips `true` on a fresh machine.
    var needsFirstRunSetup: Bool {
        hasSeenConfig && (config?.source?.isEmpty ?? true)
    }

    private let decoder = JSONDecoder()

    func apply(_ ev: DaemonEvent) {
        switch ev {
        case .hello, .historyUpdate, .unknown:
            break

        // Library/selection events (daemon v1.4.0). Fully handled in the
        // reducer's selection state; interim no-op keeps the switch exhaustive
        // until that lands.
        case .libraryUpdate, .selectionUpdate, .selectionPreview:
            break

        case let .configUpdate(source, daemon, ipod):
            config = AppConfig(source: source, daemon: daemon, ipod: ipod)
            // The daemon considers itself configured once it has a persisted
            // iPod identity (daemon: `configured = configured_serial.is_some()`).
            // It emits `config_update` (not a pushed `status_update`) after a
            // `save_config`, so derive the flag here too or the menu would stay
            // stuck on "Set Up…" right after first-run setup. Track the serial
            // itself (not just presence) so a later device swap is caught by
            // `isConfiguredForCurrentDevice`.
            hasSeenConfig = true
            configuredSerial = ipod?.serial
            phase = computePhase(targetSyncing: phaseIsSyncing)

        case let .deviceConnected(serial, modelLabel, drive, name):
            device = DeviceState(serial: serial, model: modelLabel, name: name, drive: drive)
            isIpodConnected = true
            storageText = Self.formatStorage(storageFor(drive: drive))
            phase = computePhase(targetSyncing: phaseIsSyncing)

        case .deviceDisconnected:
            device = nil
            isIpodConnected = false
            storageText = nil
            phase = computePhase(targetSyncing: false)

        case let .statusUpdate(info):
            statusConfigured = info.configured
            isIpodConnected = info.ipodConnected
            lastSync = info.lastSync
            syncedCount = info.syncedCount
            libraryCount = info.libraryCount
            let targetSyncing: Bool
            switch info.state {
            case .syncing: targetSyncing = true
            case .idle, .scanning: targetSyncing = false
            }
            phase = computePhase(targetSyncing: targetSyncing)

        case let .syncEvent(line):
            applySyncEvent(line)

        case let .syncRejected(reason):
            phase = .error(Self.humanReadable(rejection: reason))
        }
    }

    /// Called once a surfaced `pendingPrompt` has been answered (its
    /// `decide_prompt` sent) so the same prompt isn't re-presented.
    func clearPendingPrompt() {
        pendingPrompt = nil
    }

    private var phaseIsSyncing: Bool {
        if case .syncing = phase { return true }
        return false
    }

    /// `noDevice`/`notConfigured` precedence used when deriving phase from
    /// connection/config state. Sync progress events (`sync_event` lines)
    /// bypass this and set `.syncing`/`.idle` directly.
    private func computePhase(targetSyncing: Bool) -> Phase {
        guard isIpodConnected else { return .noDevice }
        guard isConfiguredForCurrentDevice else { return .notConfigured }
        guard targetSyncing else {
            // A paused sync is a resting state, not plain idle: the sync
            // subprocess has already emitted `paused` and exited, so the daemon
            // now broadcasts `idle`. Without this, that trailing idle status
            // would wipe `.paused` and the menu would silently drop the Resume
            // affordance. Hold `.paused` (refreshing its X/Y from the latest
            // status) until the user resumes (targetSyncing → `.syncing`
            // below), the device disconnects (guard above), or the app restarts
            // (phase starts at `.noDevice`, so a cold idle status shows the
            // normal "X synced" count, never a phantom pause).
            if case .paused = phase { return .paused(synced: syncedCount, total: libraryCount) }
            return .idle
        }
        if case .syncing = phase { return phase }
        return .syncing(current: 0, total: 0, label: "")
    }

    private func applySyncEvent(_ line: String) {
        guard let data = line.data(using: .utf8),
              let event = try? decoder.decode(SyncEvent.self, from: data) else { return }
        switch event {
        case let .trackStart(current, total, label):
            phase = .syncing(current: current, total: total, label: label)
        case .finish:
            phase = .idle
        case let .prompt(id, message, options):
            pendingPrompt = PendingPrompt(id: id, message: message, options: options)
        case let .form(id, label, initial, hint):
            pendingPrompt = PendingPrompt(id: id, message: hint ?? label, options: initial.map { [$0] } ?? [])
        case let .error(message, _):
            phase = .error(message)
        case .paused:
            phase = .paused(synced: syncedCount, total: libraryCount)
        case .hello, .header, .summary, .trackDone, .log, .other:
            break
        }
    }

    private static func formatStorage(_ pair: (free: Int64, total: Int64)?) -> String? {
        guard let pair else { return nil }
        let freeGB = pair.free / 1_000_000_000
        let totalGB = pair.total / 1_000_000_000
        return "\(freeGB) / \(totalGB) GB"
    }

    private static func humanReadable(rejection reason: String) -> String {
        switch reason {
        case "already_syncing": return "A sync is already in progress."
        case "no_ipod": return "No iPod is connected."
        case "not_configured": return "Classick isn't configured yet."
        case "too_many_failures": return "Sync disabled after repeated failures."
        default: return reason
        }
    }
}
