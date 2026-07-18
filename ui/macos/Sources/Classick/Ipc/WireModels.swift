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
    /// **Since daemon protocol 1.5.0.** Default `false` (shared selection).
    /// `true` routes this device's selection to its own per-device
    /// `devices/<serial>/selection.json` instead of the shared file. Rides
    /// the existing `ipod` field on `config_update`/`save_config` — see
    /// docs/ipc-protocol.md "IpodIdentity gains custom_selection". Every
    /// Swift construction site MUST thread through the existing value (never
    /// a bare default) or a save silently resets it — see the 0.2.1
    /// wizard-clobber lesson this mirrors for `rockboxCompat`.
    var customSelection: Bool

    enum CodingKeys: String, CodingKey {
        case serial
        case modelLabel = "model_label"
        case name
        case customSelection = "custom_selection"
    }

    init(serial: String, modelLabel: String, name: String? = nil, customSelection: Bool = false) {
        self.serial = serial
        self.modelLabel = modelLabel
        self.name = name
        self.customSelection = customSelection
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        serial = try container.decode(String.self, forKey: .serial)
        modelLabel = try container.decode(String.self, forKey: .modelLabel)
        name = try container.decodeIfPresent(String.self, forKey: .name)
        customSelection = try container.decodeIfPresent(Bool.self, forKey: .customSelection) ?? false
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

/// The subset of the daemon's `SyncSummary` (persisted `HistoryEntry.summary`)
/// this app needs. `add`/`modify`/`remove`/`unchanged`/`skipped`/
/// `metadata_only` are present on the wire but decoded leniently —
/// JSONDecoder ignores unknown keys, and they aren't needed for display
/// beyond `outcome`. **Since daemon protocol 1.5.0**: the three fields below,
/// each `#[serde(default)]` on the Rust side so pre-1.5.0 `history.json`
/// entries (or a `summary` object from an older daemon) deserialize to `0`.
struct SyncSummaryInfo: Codable, Equatable, Sendable {
    var skippedForSpaceTracks: Int
    var skippedForSpaceBytes: UInt64
    var artworkFailedSources: Int

    enum CodingKeys: String, CodingKey {
        case skippedForSpaceTracks = "skipped_for_space_tracks"
        case skippedForSpaceBytes = "skipped_for_space_bytes"
        case artworkFailedSources = "artwork_failed_sources"
    }

    init(skippedForSpaceTracks: Int = 0, skippedForSpaceBytes: UInt64 = 0, artworkFailedSources: Int = 0) {
        self.skippedForSpaceTracks = skippedForSpaceTracks
        self.skippedForSpaceBytes = skippedForSpaceBytes
        self.artworkFailedSources = artworkFailedSources
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        skippedForSpaceTracks = try container.decodeIfPresent(Int.self, forKey: .skippedForSpaceTracks) ?? 0
        skippedForSpaceBytes = try container.decodeIfPresent(UInt64.self, forKey: .skippedForSpaceBytes) ?? 0
        artworkFailedSources = try container.decodeIfPresent(Int.self, forKey: .artworkFailedSources) ?? 0
    }
}

struct HistoryEntry: Codable, Equatable, Sendable {
    var timestamp: String
    var durationSecs: UInt64
    var trigger: String
    var outcome: String
    /// Absent on the wire when the run never reached a summarizable state
    /// (e.g. aborted before planning). **Since 1.5.0** it also carries the
    /// skipped-for-space + artwork-failure rollups — see `SyncSummaryInfo`.
    var summary: SyncSummaryInfo?
    /// **Since daemon protocol 1.5.0.** Mirrors the subprocess `finish`
    /// event's `db_restored` (§4.11) for that run. Omitted on the wire (not
    /// `false`) when it didn't fire, matching the subprocess field's own
    /// old-client-compat convention — decode absence as `false`.
    var dbRestored: Bool

    enum CodingKeys: String, CodingKey {
        case timestamp
        case durationSecs = "duration_secs"
        case trigger
        case outcome
        case summary
        case dbRestored = "db_restored"
    }

    init(timestamp: String, durationSecs: UInt64, trigger: String, outcome: String,
         summary: SyncSummaryInfo? = nil, dbRestored: Bool = false) {
        self.timestamp = timestamp
        self.durationSecs = durationSecs
        self.trigger = trigger
        self.outcome = outcome
        self.summary = summary
        self.dbRestored = dbRestored
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        timestamp = try container.decode(String.self, forKey: .timestamp)
        durationSecs = try container.decode(UInt64.self, forKey: .durationSecs)
        trigger = try container.decode(String.self, forKey: .trigger)
        outcome = try container.decode(String.self, forKey: .outcome)
        summary = try container.decodeIfPresent(SyncSummaryInfo.self, forKey: .summary)
        dbRestored = try container.decodeIfPresent(Bool.self, forKey: .dbRestored) ?? false
    }
}

struct StatusInfo: Equatable, Sendable {
    enum State: String, Codable, Sendable { case idle, syncing, scanning }

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

// MARK: - Library selection (daemon protocol v1.4.0)

enum SelectionMode: String, Codable, Equatable, Sendable {
    case all, include, exclude
}

enum SelectionRule: Codable, Equatable, Hashable, Sendable {
    // Hashable is declared here (synthesized) rather than retroactively in
    // the test target — Swift 6 rejects cross-module retroactive
    // conformances without @retroactive, and the tests use Set([rules]).
    case artist(name: String)
    case album(artist: String, album: String)
    case genre(name: String)

    private enum CodingKeys: String, CodingKey {
        case kind, name, artist, album
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "artist": self = .artist(name: try c.decode(String.self, forKey: .name))
        case "album": self = .album(
            artist: try c.decode(String.self, forKey: .artist),
            album: try c.decode(String.self, forKey: .album))
        case "genre": self = .genre(name: try c.decode(String.self, forKey: .name))
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c,
                debugDescription: "unknown rule kind \(other)")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .artist(name):
            try c.encode("artist", forKey: .kind)
            try c.encode(name, forKey: .name)
        case let .album(artist, album):
            try c.encode("album", forKey: .kind)
            try c.encode(artist, forKey: .artist)
            try c.encode(album, forKey: .album)
        case let .genre(name):
            try c.encode("genre", forKey: .kind)
            try c.encode(name, forKey: .name)
        }
    }
}

/// Wire shape of a selection nested under `device_config_update`/
/// `save_device_config` (`{mode, rules}` only — no `version`, a file-format
/// implementation detail not part of the wire contract; mirrors how
/// `selection_update`/`save_selection` already flatten to just
/// `mode`+`rules`). Also `AppModel`'s own global-selection state (protocol
/// v1.4.0's `get_selection`/`selection_update`), which never carried
/// `version` either. Decodes leniently (mode/rules default when absent)
/// even though every known sender includes both today.
struct SelectionState: Codable, Equatable, Sendable {
    var mode: SelectionMode
    var rules: [SelectionRule]

    enum CodingKeys: String, CodingKey { case mode, rules }

    init(mode: SelectionMode, rules: [SelectionRule]) {
        self.mode = mode
        self.rules = rules
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        mode = try c.decodeIfPresent(SelectionMode.self, forKey: .mode) ?? .all
        rules = try c.decodeIfPresent([SelectionRule].self, forKey: .rules) ?? []
    }
}

struct LibraryAlbum: Codable, Equatable, Sendable {
    var name: String
    var genre: String?
    var tracks: Int
    var bytes: UInt64
}

struct LibraryArtist: Codable, Equatable, Sendable {
    var name: String
    var albums: [LibraryAlbum]
}

struct LibraryGenre: Codable, Equatable, Sendable {
    var name: String
    var tracks: Int
    var bytes: UInt64
}

struct LibraryInfo: Equatable, Sendable {
    var sourceRoot: String?
    var scannedAtUnixSecs: UInt64?
    var artists: [LibraryArtist]
    var genres: [LibraryGenre]
    var totalTracks: Int
    var totalBytes: UInt64
}

struct SelectionPreviewInfo: Equatable, Sendable {
    var selectedTracks: Int
    var selectedBytes: UInt64
    var adds: Int
    var removes: Int
}

// MARK: - Playlists & per-device config (daemon protocol v1.6.0)

enum PlaylistKind: String, Codable, Equatable, Sendable {
    case manual, smart
}

/// One entry on `playlists_update` — a summary (track COUNT, not the
/// ordered list) for the playlists sidebar/list. See `PlaylistDetail` for
/// the full-content reply the editor needs.
struct PlaylistSummary: Codable, Equatable, Sendable {
    var slug: String
    var name: String
    var kind: PlaylistKind
    var tracks: Int
    var bytes: UInt64
    var error: String?
}

enum SmartMatching: String, Codable, Equatable, Sendable { case all, any }
enum SmartField: String, Codable, Equatable, Sendable { case artist, album, genre, year }
enum SmartOp: String, Codable, Equatable, Sendable {
    case `is` = "is"
    case contains
    case gte
    case lte
}
enum SmartOrder: String, Codable, Equatable, Sendable {
    case recentlyModified = "recently_modified"
    case randomStable = "random_stable"
    case alpha
}

struct SmartRuleWire: Codable, Equatable, Sendable {
    var field: SmartField
    var op: SmartOp
    var value: String
}

/// `{"bytes":<u64>}` or `{"tracks":<int>}` — Rust's `Limit` enum carries no
/// explicit `#[serde(tag = …)]`, so it serializes externally tagged (the
/// variant name is the sole object key).
enum SmartLimitWire: Codable, Equatable, Sendable {
    case bytes(UInt64)
    case tracks(Int)

    private enum CodingKeys: String, CodingKey { case bytes, tracks }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if let n = try c.decodeIfPresent(UInt64.self, forKey: .bytes) {
            self = .bytes(n)
        } else if let n = try c.decodeIfPresent(Int.self, forKey: .tracks) {
            self = .tracks(n)
        } else {
            throw DecodingError.dataCorruptedError(
                forKey: .bytes, in: c, debugDescription: "unknown smart-playlist limit shape")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .bytes(n): try c.encode(n, forKey: .bytes)
        case let .tracks(n): try c.encode(n, forKey: .tracks)
        }
    }
}

/// Verbatim mirror of `playlist_rules::SmartRules`. `limit` serializes as an
/// explicit `null` (not omitted) when absent — unlike most optional fields
/// elsewhere on this wire, the Rust struct's fields carry no
/// `skip_serializing_if`. `version`/`matching`/`order`/`seed` decode
/// leniently even though the daemon always sends them today — deliberately
/// broader than the Rust struct, where only `limit`/`order`/`seed` carry
/// `#[serde(default)]`.
struct SmartRulesWire: Codable, Equatable, Sendable {
    var version: Int
    var matching: SmartMatching
    var rules: [SmartRuleWire]
    var limit: SmartLimitWire?
    var order: SmartOrder
    var seed: UInt64

    enum CodingKeys: String, CodingKey { case version, matching, rules, limit, order, seed }

    init(version: Int = 1, matching: SmartMatching, rules: [SmartRuleWire],
         limit: SmartLimitWire? = nil, order: SmartOrder = .alpha, seed: UInt64 = 0) {
        self.version = version
        self.matching = matching
        self.rules = rules
        self.limit = limit
        self.order = order
        self.seed = seed
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        matching = try c.decodeIfPresent(SmartMatching.self, forKey: .matching) ?? .all
        rules = try c.decodeIfPresent([SmartRuleWire].self, forKey: .rules) ?? []
        limit = try c.decodeIfPresent(SmartLimitWire.self, forKey: .limit)
        order = try c.decodeIfPresent(SmartOrder.self, forKey: .order) ?? .alpha
        seed = try c.decodeIfPresent(UInt64.self, forKey: .seed) ?? 0
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(version, forKey: .version)
        try c.encode(matching, forKey: .matching)
        try c.encode(rules, forKey: .rules)
        if let limit {
            try c.encode(limit, forKey: .limit)
        } else {
            try c.encodeNil(forKey: .limit)
        }
        try c.encode(order, forKey: .order)
        try c.encode(seed, forKey: .seed)
    }
}

/// `save_playlist`'s `playlist` field — Encodable only (the app constructs
/// and sends this; the daemon never sends one back, see `PlaylistDetail`).
/// An absent `slug` means "create a new playlist"; a present one means
/// "create-or-replace at exactly this slug".
enum PlaylistPayload: Encodable, Equatable, Sendable {
    case manual(slug: String?, name: String, tracks: [String])
    case smart(slug: String?, name: String, rules: SmartRulesWire)

    private enum CodingKeys: String, CodingKey { case kind, slug, name, tracks, rules }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .manual(slug, name, tracks):
            try c.encode("manual", forKey: .kind)
            try c.encodeIfPresent(slug, forKey: .slug)
            try c.encode(name, forKey: .name)
            try c.encode(tracks, forKey: .tracks)
        case let .smart(slug, name, rules):
            try c.encode("smart", forKey: .kind)
            try c.encodeIfPresent(slug, forKey: .slug)
            try c.encode(name, forKey: .name)
            try c.encode(rules, forKey: .rules)
        }
    }
}

/// Reply to `get_playlist`: the one playlist's full content, for the
/// editor — unlike `PlaylistsUpdate`'s summary (a track count), `tracks`
/// here is the actual ordered path list. On failure (unknown slug,
/// unopenable store, or an unparseable on-disk file) only `slug`/`error`
/// are set — `name`/`kind`/`tracks`/`rules` all stay `nil`.
struct PlaylistDetail: Equatable, Sendable {
    var slug: String
    var name: String?
    var kind: PlaylistKind?
    var tracks: [String]?
    var rules: SmartRulesWire?
    var error: String?
}

/// Wire shape of subscriptions nested under `device_config_update`/
/// `save_device_config` — the subscribed playlist slugs only (no
/// `version`; same rationale as `SelectionState`). `version` isn't listed
/// in `CodingKeys`, so it's never encoded/decoded — it always holds its
/// default, matching the wire's own omission.
struct SubscriptionsWire: Codable, Equatable, Sendable {
    var version = 1
    var playlists: [String]

    enum CodingKeys: String, CodingKey { case playlists }

    init(playlists: [String]) {
        self.playlists = playlists
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        playlists = try c.decodeIfPresent([String].self, forKey: .playlists) ?? []
    }
}

/// Wire shape of per-device settings nested under `device_config_update`/
/// `save_device_config` (no `version`; same rationale as
/// `SubscriptionsWire`).
struct DeviceSettingsWire: Codable, Equatable, Sendable {
    var version = 1
    var autoSync: Bool
    var rockboxCompat: Bool

    enum CodingKeys: String, CodingKey {
        case autoSync = "auto_sync"
        case rockboxCompat = "rockbox_compat"
    }

    init(autoSync: Bool, rockboxCompat: Bool) {
        self.autoSync = autoSync
        self.rockboxCompat = rockboxCompat
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        autoSync = try c.decodeIfPresent(Bool.self, forKey: .autoSync) ?? true
        rockboxCompat = try c.decodeIfPresent(Bool.self, forKey: .rockboxCompat) ?? false
    }
}

/// Reply to `preview_device`. **Carries no `serial`/correlation id** — like
/// `selection_preview`, it's a plain reply on the requesting connection, not
/// a broadcast — so the caller must remember which device it asked about;
/// see `AppModel.willRequestDevicePreview`. `unresolvedSubscriptions` is
/// omitted from the wire (not sent as `[]`) when every subscription
/// resolved — decode absence as `nil`, not an empty array, so callers can
/// still tell "omitted" from "explicitly empty" if that ever matters.
struct DevicePreview: Equatable, Sendable {
    var selectedTracks: Int
    var selectedBytes: UInt64
    var playlistExtraTracks: Int
    var playlistExtraBytes: UInt64
    var projectedFreeBytes: UInt64?
    var unresolvedSubscriptions: [String]?
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
    case getLibrary
    case scanLibrary
    case getSelection
    case saveSelection(mode: SelectionMode, rules: [SelectionRule])
    case previewSelection(mode: SelectionMode, rules: [SelectionRule])
    case getHistory(limit: Int)
    /// **Since daemon protocol 1.5.0.** One-shot "erase and start over": wipes
    /// every track on the iPod, then syncs the current selection. The UI must
    /// obtain the user's explicit confirmation itself before sending this —
    /// the daemon does not prompt. See docs/ipc-protocol.md "New command:
    /// replace_library".
    case replaceLibrary
    // MARK: Protocol 1.6.0 — playlists, per-device config, device preview
    case listPlaylists
    case getPlaylist(slug: String)
    case savePlaylist(PlaylistPayload)
    case deletePlaylist(slug: String)
    case getDeviceConfig(serial: String)
    /// Each part `nil` means "don't change" — the same sentinel convention
    /// as `saveConfig`.
    case saveDeviceConfig(serial: String, selection: SelectionState?, subscriptions: SubscriptionsWire?, settings: DeviceSettingsWire?)
    case previewDevice(serial: String)

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
        case mode
        case rules
        case limit
        case slug
        case playlist
        case serial
        case selection
        case subscriptions
        case settings
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
        case .getLibrary:
            try container.encode("get_library", forKey: .type)
        case .scanLibrary:
            try container.encode("scan_library", forKey: .type)
        case .getSelection:
            try container.encode("get_selection", forKey: .type)
        case let .saveSelection(mode, rules):
            try container.encode("save_selection", forKey: .type)
            try container.encode(mode, forKey: .mode)
            try container.encode(rules, forKey: .rules)
        case let .previewSelection(mode, rules):
            try container.encode("preview_selection", forKey: .type)
            try container.encode(mode, forKey: .mode)
            try container.encode(rules, forKey: .rules)
        case let .getHistory(limit):
            try container.encode("get_history", forKey: .type)
            try container.encode(limit, forKey: .limit)
        case .replaceLibrary:
            try container.encode("replace_library", forKey: .type)
        case .listPlaylists:
            try container.encode("list_playlists", forKey: .type)
        case let .getPlaylist(slug):
            try container.encode("get_playlist", forKey: .type)
            try container.encode(slug, forKey: .slug)
        case let .savePlaylist(playlist):
            try container.encode("save_playlist", forKey: .type)
            try container.encode(playlist, forKey: .playlist)
        case let .deletePlaylist(slug):
            try container.encode("delete_playlist", forKey: .type)
            try container.encode(slug, forKey: .slug)
        case let .getDeviceConfig(serial):
            try container.encode("get_device_config", forKey: .type)
            try container.encode(serial, forKey: .serial)
        case let .saveDeviceConfig(serial, selection, subscriptions, settings):
            try container.encode("save_device_config", forKey: .type)
            try container.encode(serial, forKey: .serial)
            try container.encodeIfPresent(selection, forKey: .selection)
            try container.encodeIfPresent(subscriptions, forKey: .subscriptions)
            try container.encodeIfPresent(settings, forKey: .settings)
        case let .previewDevice(serial):
            try container.encode("preview_device", forKey: .type)
            try container.encode(serial, forKey: .serial)
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
    case libraryUpdate(LibraryInfo)
    case selectionUpdate(mode: SelectionMode, rules: [SelectionRule])
    case selectionPreview(SelectionPreviewInfo)
    // MARK: Protocol 1.6.0 — playlists, per-device config, device preview
    case playlistsUpdate([PlaylistSummary])
    case playlistDetail(PlaylistDetail)
    case deviceConfigUpdate(serial: String, selection: SelectionState, subscriptions: SubscriptionsWire, settings: DeviceSettingsWire)
    case devicePreview(DevicePreview)
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
        case sourceRoot = "source_root"
        case scannedAtUnixSecs = "scanned_at_unix_secs"
        case artists
        case genres
        case totalTracks = "total_tracks"
        case totalBytes = "total_bytes"
        case mode
        case rules
        case selectedTracks = "selected_tracks"
        case selectedBytes = "selected_bytes"
        case adds
        case removes
        case playlists
        case slug
        case kind
        case tracks
        case error
        case selection
        case subscriptions
        case settings
        case playlistExtraTracks = "playlist_extra_tracks"
        case playlistExtraBytes = "playlist_extra_bytes"
        case projectedFreeBytes = "projected_free_bytes"
        case unresolvedSubscriptions = "unresolved_subscriptions"
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
            // Unknown state values MUST decode as .idle (protocol §Daemon
            // v1.4.0) — a hard decode failure here would drop the whole
            // status_update and freeze the menu on stale state.
            let stateRaw = try container.decode(String.self, forKey: .state)
            let state = StatusInfo.State(rawValue: stateRaw) ?? .idle
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
        case "library_update":
            self = .libraryUpdate(LibraryInfo(
                sourceRoot: try container.decodeIfPresent(String.self, forKey: .sourceRoot),
                scannedAtUnixSecs: try container.decodeIfPresent(UInt64.self, forKey: .scannedAtUnixSecs),
                artists: try container.decodeIfPresent([LibraryArtist].self, forKey: .artists) ?? [],
                genres: try container.decodeIfPresent([LibraryGenre].self, forKey: .genres) ?? [],
                totalTracks: try container.decodeIfPresent(Int.self, forKey: .totalTracks) ?? 0,
                totalBytes: try container.decodeIfPresent(UInt64.self, forKey: .totalBytes) ?? 0))
        case "selection_update":
            self = .selectionUpdate(
                mode: try container.decodeIfPresent(SelectionMode.self, forKey: .mode) ?? .all,
                rules: try container.decodeIfPresent([SelectionRule].self, forKey: .rules) ?? [])
        case "selection_preview":
            self = .selectionPreview(SelectionPreviewInfo(
                selectedTracks: try container.decodeIfPresent(Int.self, forKey: .selectedTracks) ?? 0,
                selectedBytes: try container.decodeIfPresent(UInt64.self, forKey: .selectedBytes) ?? 0,
                adds: try container.decodeIfPresent(Int.self, forKey: .adds) ?? 0,
                removes: try container.decodeIfPresent(Int.self, forKey: .removes) ?? 0))
        case "playlists_update":
            let playlists = try container.decodeIfPresent([PlaylistSummary].self, forKey: .playlists) ?? []
            self = .playlistsUpdate(playlists)
        case "playlist_detail":
            let slug = try container.decode(String.self, forKey: .slug)
            let name = try container.decodeIfPresent(String.self, forKey: .name)
            let kind = try container.decodeIfPresent(PlaylistKind.self, forKey: .kind)
            let tracks = try container.decodeIfPresent([String].self, forKey: .tracks)
            let rules = try container.decodeIfPresent(SmartRulesWire.self, forKey: .rules)
            let error = try container.decodeIfPresent(String.self, forKey: .error)
            self = .playlistDetail(PlaylistDetail(slug: slug, name: name, kind: kind, tracks: tracks, rules: rules, error: error))
        case "device_config_update":
            let serial = try container.decode(String.self, forKey: .serial)
            let selection = try container.decodeIfPresent(SelectionState.self, forKey: .selection)
                ?? SelectionState(mode: .all, rules: [])
            let subscriptions = try container.decodeIfPresent(SubscriptionsWire.self, forKey: .subscriptions)
                ?? SubscriptionsWire(playlists: [])
            let settings = try container.decodeIfPresent(DeviceSettingsWire.self, forKey: .settings)
                ?? DeviceSettingsWire(autoSync: true, rockboxCompat: false)
            self = .deviceConfigUpdate(serial: serial, selection: selection, subscriptions: subscriptions, settings: settings)
        case "device_preview":
            self = .devicePreview(DevicePreview(
                selectedTracks: try container.decodeIfPresent(Int.self, forKey: .selectedTracks) ?? 0,
                selectedBytes: try container.decodeIfPresent(UInt64.self, forKey: .selectedBytes) ?? 0,
                playlistExtraTracks: try container.decodeIfPresent(Int.self, forKey: .playlistExtraTracks) ?? 0,
                playlistExtraBytes: try container.decodeIfPresent(UInt64.self, forKey: .playlistExtraBytes) ?? 0,
                projectedFreeBytes: try container.decodeIfPresent(UInt64.self, forKey: .projectedFreeBytes),
                unresolvedSubscriptions: try container.decodeIfPresent([String].self, forKey: .unresolvedSubscriptions)))
        default:
            self = .unknown
        }
    }
}

// MARK: - SyncEvent (inner v1.0.0 `sync_event.line`, `finish` extended in 1.3.0)

/// `finish.skipped_for_space` — whole-album fit-pass deferral rollup. Absent
/// when nothing was deferred this run.
struct SkippedForSpace: Codable, Equatable, Sendable {
    var albums: Int
    var tracks: Int
    var bytes: UInt64
}

/// `finish.artwork` — cover-art embed rollup across this run's
/// Add/Modify/MetadataOnly actions. Absent when the run never reached the
/// apply loop.
struct ArtworkSummary: Codable, Equatable, Sendable {
    var embedded: Int
    var eligible: Int
    var failedSources: Int

    enum CodingKeys: String, CodingKey {
        case embedded
        case eligible
        case failedSources = "failed_sources"
    }
}

enum SyncEvent: Decodable, Sendable {
    case hello(protocolVersion: String, coreVersion: String)
    case header(source: String, ipod: String, manifest: String)
    case summary(add: Int, modify: Int, metadataOnly: Int, remove: Int, unchanged: Int, totalPlanned: Int)
    case trackStart(current: Int, total: Int, label: String, etaSecs: UInt64?)
    case trackDone
    case log(message: String)
    case prompt(id: UInt64, message: String, options: [String])
    case form(id: UInt64, label: String, initial: String?, hint: String?)
    case error(message: String, recoveryHints: [String]?)
    /// **Since subprocess protocol 1.3.0:** `skippedForSpace`/`artwork` are
    /// `nil` when absent from the wire (nothing to report); `dbRestored`
    /// defaults `false` when absent (mirroring the wire's own
    /// absent-means-false convention) rather than being optional.
    case finish(success: Bool, skippedForSpace: SkippedForSpace?, artwork: ArtworkSummary?, dbRestored: Bool)
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
        case etaSecs = "eta_secs"
        case skippedForSpace = "skipped_for_space"
        case artwork
        case dbRestored = "db_restored"
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
            let etaSecs = try container.decodeIfPresent(UInt64.self, forKey: .etaSecs)
            self = .trackStart(current: current, total: total, label: label, etaSecs: etaSecs)
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
            let skippedForSpace = try container.decodeIfPresent(SkippedForSpace.self, forKey: .skippedForSpace)
            let artwork = try container.decodeIfPresent(ArtworkSummary.self, forKey: .artwork)
            let dbRestored = try container.decodeIfPresent(Bool.self, forKey: .dbRestored) ?? false
            self = .finish(success: success, skippedForSpace: skippedForSpace, artwork: artwork, dbRestored: dbRestored)
        case "paused":
            self = .paused
        default:
            self = .other
        }
    }
}
