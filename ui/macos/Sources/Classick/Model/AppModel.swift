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

@Observable
@MainActor
final class AppModel {
    private(set) var device: DeviceState?
    private(set) var phase: Phase = .noDevice
    private(set) var lastSync: HistoryEntry?
    private(set) var pendingPrompt: PendingPrompt?
    private(set) var storageText: String?

    // Tracked separately from `device` because `status_update` carries its
    // own `ipod_connected`/`configured` flags independent of the
    // `device_connected`/`device_disconnected` events.
    private var isIpodConnected = false
    private var isConfigured = false

    private let decoder = JSONDecoder()

    func apply(_ ev: DaemonEvent) {
        switch ev {
        case .hello, .configUpdate, .historyUpdate, .unknown:
            break

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
            isConfigured = info.configured
            isIpodConnected = info.ipodConnected
            lastSync = info.lastSync
            let targetSyncing: Bool
            switch info.state {
            case .syncing: targetSyncing = true
            case .idle: targetSyncing = false
            }
            phase = computePhase(targetSyncing: targetSyncing)

        case let .syncEvent(line):
            applySyncEvent(line)

        case let .syncRejected(reason):
            phase = .error(Self.humanReadable(rejection: reason))
        }
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
        guard isConfigured else { return .notConfigured }
        guard targetSyncing else { return .idle }
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
