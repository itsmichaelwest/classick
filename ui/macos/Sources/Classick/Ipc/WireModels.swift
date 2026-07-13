import Foundation

// Wire contract: docs/ipc-protocol.md (inner `sync_event.line`, v1.0.0) and
// the daemon-pipe protocol described in AGENTS.md / the macOS app plan's
// "Global Constraints". Every command/event is a JSON object with a
// snake_case "type" discriminator. Field names here are verbatim copies of
// the Rust wire names — do not rename without a corresponding Rust change.

struct IpodIdentity: Codable, Equatable, Sendable {
    var serial: String
    var modelLabel: String
    var name: String?

    enum CodingKeys: String, CodingKey {
        case serial
        case modelLabel = "model_label"
        case name
    }
}

struct DaemonSettings: Codable, Equatable, Sendable {
    var enabled: Bool
    var autostartWithWindows: Bool
    var firstSyncMode: String        // "review" | "auto_apply"
    var subsequentSyncMode: String   // "review" | "auto_apply"
    var scheduleMinutes: UInt32
    var notifyOn: String             // "all" | "errors_only" | "none"
    var rockboxCompat: Bool

    enum CodingKeys: String, CodingKey {
        case enabled
        case autostartWithWindows = "autostart_with_windows"
        case firstSyncMode = "first_sync_mode"
        case subsequentSyncMode = "subsequent_sync_mode"
        case scheduleMinutes = "schedule_minutes"
        case notifyOn = "notify_on"
        case rockboxCompat = "rockbox_compat"
    }

    init(
        enabled: Bool,
        autostartWithWindows: Bool,
        firstSyncMode: String,
        subsequentSyncMode: String,
        scheduleMinutes: UInt32,
        notifyOn: String,
        rockboxCompat: Bool = false
    ) {
        self.enabled = enabled
        self.autostartWithWindows = autostartWithWindows
        self.firstSyncMode = firstSyncMode
        self.subsequentSyncMode = subsequentSyncMode
        self.scheduleMinutes = scheduleMinutes
        self.notifyOn = notifyOn
        self.rockboxCompat = rockboxCompat
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try container.decode(Bool.self, forKey: .enabled)
        autostartWithWindows = try container.decode(Bool.self, forKey: .autostartWithWindows)
        firstSyncMode = try container.decode(String.self, forKey: .firstSyncMode)
        subsequentSyncMode = try container.decode(String.self, forKey: .subsequentSyncMode)
        scheduleMinutes = try container.decode(UInt32.self, forKey: .scheduleMinutes)
        notifyOn = try container.decode(String.self, forKey: .notifyOn)
        rockboxCompat = try container.decodeIfPresent(Bool.self, forKey: .rockboxCompat) ?? false
    }
}

struct HistoryEntry: Codable, Equatable, Sendable {
    var timestamp: String
    var durationSecs: UInt64
    var trigger: String
    var outcome: String

    enum CodingKeys: String, CodingKey {
        case timestamp
        case durationSecs = "duration_secs"
        case trigger
        case outcome
    }
    // `summary` ({add,modify,remove,unchanged,skipped}) is present on the
    // wire but decoded leniently — JSONDecoder ignores unknown keys, and
    // it isn't needed for v1 display beyond `outcome`.
}

struct StatusInfo: Equatable, Sendable {
    enum State: String, Codable, Sendable { case idle, syncing }

    struct Storage: Codable, Equatable, Sendable {
        var free: UInt64
        var total: UInt64
    }

    var state: State
    var configured: Bool
    var ipodConnected: Bool
    var lastSync: HistoryEntry?
    var nextScheduledUnixSecs: UInt64?
    var storage: Storage?            // always nil on macOS wire; see Storage.swift
    var syncedCount: Int = 0          // X in "X of Y synced" — manifest track count
    var libraryCount: Int?            // Y — source-library track count; nil until known
}

extension StatusInfo: Codable {
    enum CodingKeys: String, CodingKey {
        case state
        case configured
        case ipodConnected = "ipod_connected"
        case lastSync = "last_sync"
        case nextScheduledUnixSecs = "next_scheduled_unix_secs"
        case storage
        case syncedCount = "synced_count"
        case libraryCount = "library_count"
    }
}

// MARK: - DaemonCommand (sent)

enum DaemonCommand: Encodable, Sendable {
    case subscribeDeviceEvents
    case getStatus
    case getConfig
    case saveConfig(source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?)
    case forgetIpod
    case triggerSync(source: Trigger)
    case cancelSync
    case pause
    case decidePrompt(id: UInt64, choice: Int32)
    case backfillRockbox

    enum Trigger: String, Encodable, Sendable {
        case manual, scheduled
        case plugIn = "plug_in"
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case source
        case daemon
        case ipod
        case id
        case choice
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .subscribeDeviceEvents:
            try container.encode("subscribe_device_events", forKey: .type)
        case .getStatus:
            try container.encode("get_status", forKey: .type)
        case .getConfig:
            try container.encode("get_config", forKey: .type)
        case let .saveConfig(source, daemon, ipod):
            try container.encode("save_config", forKey: .type)
            try container.encodeIfPresent(source, forKey: .source)
            try container.encodeIfPresent(daemon, forKey: .daemon)
            try container.encodeIfPresent(ipod, forKey: .ipod)
        case .forgetIpod:
            try container.encode("forget_ipod", forKey: .type)
        case let .triggerSync(source):
            try container.encode("trigger_sync", forKey: .type)
            try container.encode(source, forKey: .source)
        case .cancelSync:
            try container.encode("cancel_sync", forKey: .type)
        case .pause:
            try container.encode("pause", forKey: .type)
        case let .decidePrompt(id, choice):
            try container.encode("decide_prompt", forKey: .type)
            try container.encode(id, forKey: .id)
            try container.encode(choice, forKey: .choice)
        case .backfillRockbox:
            try container.encode("backfill_rockbox", forKey: .type)
        }
    }
}

// MARK: - DaemonEvent (received)

enum DaemonEvent: Decodable, Sendable {
    case hello(protocolVersion: String, coreVersion: String)
    case statusUpdate(StatusInfo)
    case configUpdate(source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?)
    case historyUpdate(entries: [HistoryEntry])
    case deviceConnected(serial: String, modelLabel: String, drive: String, name: String?)
    case deviceDisconnected(serial: String)
    case syncRejected(reason: String)
    case syncEvent(line: String)
    case unknown            // forward-compat: log + ignore

    private enum CodingKeys: String, CodingKey {
        case type
        case protocolVersion = "protocol_version"
        case coreVersion = "core_version"
        case state
        case configured
        case ipodConnected = "ipod_connected"
        case lastSync = "last_sync"
        case nextScheduledUnixSecs = "next_scheduled_unix_secs"
        case storage
        case source
        case daemon
        case ipod
        case entries
        case serial
        case modelLabel = "model_label"
        case drive
        case name
        case reason
        case line
        case syncedCount = "synced_count"
        case libraryCount = "library_count"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "hello":
            let protocolVersion = try container.decode(String.self, forKey: .protocolVersion)
            let coreVersion = try container.decode(String.self, forKey: .coreVersion)
            self = .hello(protocolVersion: protocolVersion, coreVersion: coreVersion)
        case "status_update":
            let state = try container.decode(StatusInfo.State.self, forKey: .state)
            let configured = try container.decode(Bool.self, forKey: .configured)
            let ipodConnected = try container.decode(Bool.self, forKey: .ipodConnected)
            let lastSync = try container.decodeIfPresent(HistoryEntry.self, forKey: .lastSync)
            let nextScheduledUnixSecs = try container.decodeIfPresent(UInt64.self, forKey: .nextScheduledUnixSecs)
            let storage = try container.decodeIfPresent(StatusInfo.Storage.self, forKey: .storage)
            let syncedCount = try container.decodeIfPresent(Int.self, forKey: .syncedCount) ?? 0
            let libraryCount = try container.decodeIfPresent(Int.self, forKey: .libraryCount)
            self = .statusUpdate(StatusInfo(
                state: state,
                configured: configured,
                ipodConnected: ipodConnected,
                lastSync: lastSync,
                nextScheduledUnixSecs: nextScheduledUnixSecs,
                storage: storage,
                syncedCount: syncedCount,
                libraryCount: libraryCount))
        case "config_update":
            let source = try container.decodeIfPresent(String.self, forKey: .source)
            let daemon = try container.decodeIfPresent(DaemonSettings.self, forKey: .daemon)
            let ipod = try container.decodeIfPresent(IpodIdentity.self, forKey: .ipod)
            self = .configUpdate(source: source, daemon: daemon, ipod: ipod)
        case "history_update":
            let entries = try container.decode([HistoryEntry].self, forKey: .entries)
            self = .historyUpdate(entries: entries)
        case "device_connected":
            let serial = try container.decode(String.self, forKey: .serial)
            let modelLabel = try container.decode(String.self, forKey: .modelLabel)
            let drive = try container.decode(String.self, forKey: .drive)
            let name = try container.decodeIfPresent(String.self, forKey: .name)
            self = .deviceConnected(serial: serial, modelLabel: modelLabel, drive: drive, name: name)
        case "device_disconnected":
            let serial = try container.decode(String.self, forKey: .serial)
            self = .deviceDisconnected(serial: serial)
        case "sync_rejected":
            let reason = try container.decode(String.self, forKey: .reason)
            self = .syncRejected(reason: reason)
        case "sync_event":
            let line = try container.decode(String.self, forKey: .line)
            self = .syncEvent(line: line)
        default:
            self = .unknown
        }
    }
}

// MARK: - SyncEvent (inner v1.0.0 `sync_event.line`)

enum SyncEvent: Decodable, Sendable {
    case hello(protocolVersion: String, coreVersion: String)
    case header(source: String, ipod: String, manifest: String)
    case summary(add: Int, modify: Int, metadataOnly: Int, remove: Int, unchanged: Int, totalPlanned: Int)
    case trackStart(current: Int, total: Int, label: String)
    case trackDone
    case log(message: String)
    case prompt(id: UInt64, message: String, options: [String])
    case form(id: UInt64, label: String, initial: String?, hint: String?)
    case error(message: String, recoveryHints: [String]?)
    case finish(success: Bool)
    case paused
    case other            // forward-compat: unknown inner types (e.g. `review`)

    private enum CodingKeys: String, CodingKey {
        case type
        case protocolVersion = "protocol_version"
        case coreVersion = "core_version"
        case source
        case ipod
        case manifest
        case add
        case modify
        case metadataOnly = "metadata_only"
        case remove
        case unchanged
        case totalPlanned = "total_planned"
        case current
        case total
        case label
        case message
        case id
        case options
        case initial
        case hint
        case recoveryHints = "recovery_hints"
        case success
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "hello":
            let protocolVersion = try container.decode(String.self, forKey: .protocolVersion)
            let coreVersion = try container.decode(String.self, forKey: .coreVersion)
            self = .hello(protocolVersion: protocolVersion, coreVersion: coreVersion)
        case "header":
            let source = try container.decode(String.self, forKey: .source)
            let ipod = try container.decode(String.self, forKey: .ipod)
            let manifest = try container.decode(String.self, forKey: .manifest)
            self = .header(source: source, ipod: ipod, manifest: manifest)
        case "summary":
            let add = try container.decode(Int.self, forKey: .add)
            let modify = try container.decode(Int.self, forKey: .modify)
            let metadataOnly = try container.decode(Int.self, forKey: .metadataOnly)
            let remove = try container.decode(Int.self, forKey: .remove)
            let unchanged = try container.decode(Int.self, forKey: .unchanged)
            let totalPlanned = try container.decode(Int.self, forKey: .totalPlanned)
            self = .summary(add: add, modify: modify, metadataOnly: metadataOnly, remove: remove, unchanged: unchanged, totalPlanned: totalPlanned)
        case "track_start":
            let current = try container.decode(Int.self, forKey: .current)
            let total = try container.decode(Int.self, forKey: .total)
            let label = try container.decode(String.self, forKey: .label)
            self = .trackStart(current: current, total: total, label: label)
        case "track_done":
            self = .trackDone
        case "log":
            let message = try container.decode(String.self, forKey: .message)
            self = .log(message: message)
        case "prompt":
            let id = try container.decode(UInt64.self, forKey: .id)
            let message = try container.decode(String.self, forKey: .message)
            let options = try container.decode([String].self, forKey: .options)
            self = .prompt(id: id, message: message, options: options)
        case "form":
            let id = try container.decode(UInt64.self, forKey: .id)
            let label = try container.decode(String.self, forKey: .label)
            let initial = try container.decodeIfPresent(String.self, forKey: .initial)
            let hint = try container.decodeIfPresent(String.self, forKey: .hint)
            self = .form(id: id, label: label, initial: initial, hint: hint)
        case "error":
            let message = try container.decode(String.self, forKey: .message)
            let recoveryHints = try container.decodeIfPresent([String].self, forKey: .recoveryHints)
            self = .error(message: message, recoveryHints: recoveryHints)
        case "finish":
            let success = try container.decode(Bool.self, forKey: .success)
            self = .finish(success: success)
        case "paused":
            self = .paused
        default:
            self = .other
        }
    }
}
