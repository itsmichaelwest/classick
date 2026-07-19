import Foundation

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
    case "album":
      self = .album(
        artist: try c.decode(String.self, forKey: .artist),
        album: try c.decode(String.self, forKey: .album))
    case "genre": self = .genre(name: try c.decode(String.self, forKey: .name))
    case let other:
      throw DecodingError.dataCorruptedError(
        forKey: .kind, in: c,
        debugDescription: "unknown rule kind \(other)")
    }
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: CodingKeys.self)
    switch self {
    case .artist(let name):
      try c.encode("artist", forKey: .kind)
      try c.encode(name, forKey: .name)
    case .album(let artist, let album):
      try c.encode("album", forKey: .kind)
      try c.encode(artist, forKey: .artist)
      try c.encode(album, forKey: .album)
    case .genre(let name):
      try c.encode("genre", forKey: .kind)
      try c.encode(name, forKey: .name)
    }
  }
}

/// Wire shape of a selection nested under `device_config_update`/
/// `save_device_config` (`{mode, rules}` only — no `version`, a file-format
/// implementation detail not part of the wire contract. Also used for
/// `selection_update` and `AppModel`'s selection state.
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
  var acknowledgedRequestID: String? = nil
}

struct SelectionPreviewInfo: Equatable, Sendable {
  var selectedTracks: Int
  var selectedBytes: UInt64
  var adds: Int
  var removes: Int
  var serial: String
  var acknowledgedRequestID: String
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
// `CaseIterable` on `SmartField`/`SmartOp` is purely additive (auto-
// synthesized for a plain enum with no associated values) — the rule
// builder's field/op pickers (`SmartRulesEditor`, Task 7) need to enumerate
// every case; nothing before that view needed to. Doesn't touch
// `Codable`/wire shape.
enum SmartField: String, Codable, Equatable, CaseIterable, Sendable {
  case artist, album, genre, year
}
enum SmartOp: String, Codable, Equatable, CaseIterable, Sendable {
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
    case .bytes(let n): try c.encode(n, forKey: .bytes)
    case .tracks(let n): try c.encode(n, forKey: .tracks)
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

  init(
    version: Int = 1, matching: SmartMatching, rules: [SmartRuleWire],
    limit: SmartLimitWire? = nil, order: SmartOrder = .alpha, seed: UInt64 = 0
  ) {
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
    case .manual(let slug, let name, let tracks):
      try c.encode("manual", forKey: .kind)
      try c.encodeIfPresent(slug, forKey: .slug)
      try c.encode(name, forKey: .name)
      try c.encode(tracks, forKey: .tracks)
    case .smart(let slug, let name, let rules):
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
  var acknowledgedRequestID: String
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

/// Reply to `preview_device`. Version 2 requires both the target `serial` and
/// request correlation id. `unresolvedSubscriptions` is
/// omitted from the wire (not sent as `[]`) when every subscription
/// resolved — decode absence as `nil`, not an empty array, so callers can
/// still tell "omitted" from "explicitly empty" if that ever matters.
struct DevicePreview: Equatable, Sendable {
  var serial: String
  var selectedTracks: Int
  var selectedBytes: UInt64
  var playlistExtraTracks: Int
  var playlistExtraBytes: UInt64
  var projectedFreeBytes: UInt64?
  var unresolvedSubscriptions: [String]?
  var acknowledgedRequestID: String
}

// MARK: - DaemonCommand (sent)

enum DurableIntentKey: Hashable, Sendable {
  case config
  case selection
  case deviceConfig(serial: String)
  case playlist(String)
  case deviceRemoval(serial: String)
}

enum DaemonCommand: Encodable, Sendable {
  case subscribeDeviceEvents
  case shutdown
  case getStatus(requestID: String)
  case getConfig(requestID: String)
  case saveConfig(source: String?, daemon: DaemonSettings?, ipod: IpodIdentity?, requestID: String)
  case forgetIpod(serial: String, requestID: String)
  case triggerSync(source: Trigger, serial: String, requestID: String)
  case cancelSync(serial: String, requestID: String)
  case pause(serial: String, requestID: String)
  case decidePrompt(id: UInt64, choice: Int32, serial: String, requestID: String)
  case backfillRockbox(serial: String, requestID: String)
  case getLibrary(requestID: String)
  case scanLibrary(requestID: String)
  case retrySourceMount(allowUI: Bool, requestID: String)
  case previewSelection(
    mode: SelectionMode, rules: [SelectionRule], serial: String, requestID: String)
  case getHistory(limit: Int, requestID: String)
  /// **Since daemon protocol 1.5.0.** One-shot "erase and start over": wipes
  /// every track on the iPod, then syncs the current selection. The UI must
  /// obtain the user's explicit confirmation itself before sending this —
  /// the daemon does not prompt. See docs/ipc-protocol.md "New command:
  /// replace_library".
  case replaceLibrary(serial: String, requestID: String)
  // MARK: Protocol 1.6.0 — playlists, per-device config, device preview
  case listPlaylists(requestID: String)
  case getPlaylist(slug: String, requestID: String)
  case savePlaylist(PlaylistPayload, requestID: String)
  case deletePlaylist(slug: String, requestID: String)
  case getDeviceConfig(serial: String, requestID: String)
  /// Each part `nil` means "don't change" — the same sentinel convention
  /// as `saveConfig`.
  case saveDeviceConfig(
    serial: String, selection: SelectionState?, subscriptions: SubscriptionsWire?,
    settings: DeviceSettingsWire?, requestID: String)
  case previewDevice(serial: String, requestID: String)
  /// **Since daemon protocol 1.7.0.** Expands artist/album/genre rules
  /// (the same `SelectionRule` shape `save_device_config`'s selection
  /// uses) into real, resolvable source-relative track paths, evaluated
  /// server-side against the cached library index — the client has no
  /// per-file data of its own (`LibraryArtist`/`LibraryAlbum` carry track
  /// COUNTS only). Backs the playlist editor's Add Songs picker (Task 7):
  /// there is no other way to turn a picked album into literal `.m3u8`
  /// entries. Replies with `resolved_tracks`.
  case resolveTracks(rules: [SelectionRule], requestID: String)

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
    case requestID = "request_id"
    case allowUI = "allow_ui"
  }

  static func newRequestID() -> String {
    UUID().uuidString.lowercased()
  }

  var requestID: String? {
    switch self {
    case .subscribeDeviceEvents, .shutdown:
      nil
    case .getStatus(let requestID), .getConfig(let requestID),
      .forgetIpod(_, let requestID), .triggerSync(_, _, let requestID),
      .cancelSync(_, let requestID), .pause(_, let requestID),
      .decidePrompt(_, _, _, let requestID), .backfillRockbox(_, let requestID),
      .getLibrary(let requestID), .scanLibrary(let requestID),
      .retrySourceMount(_, let requestID), .previewSelection(_, _, _, let requestID),
      .getHistory(_, let requestID), .replaceLibrary(_, let requestID),
      .listPlaylists(let requestID), .getPlaylist(_, let requestID),
      .savePlaylist(_, let requestID), .deletePlaylist(_, let requestID),
      .getDeviceConfig(_, let requestID), .saveDeviceConfig(_, _, _, _, let requestID),
      .previewDevice(_, let requestID), .resolveTracks(_, let requestID):
      requestID
    case .saveConfig(_, _, _, let requestID):
      requestID
    }
  }

  var durableIntentKey: DurableIntentKey? {
    switch self {
    case .saveConfig:
      .config
    case .forgetIpod(let serial, _):
      .deviceRemoval(serial: serial)
    case .savePlaylist(let playlist, let requestID):
      .playlist(playlist.slug ?? "new:\(requestID)")
    case .deletePlaylist(let slug, _):
      .playlist(slug)
    case .saveDeviceConfig(let serial, _, _, _, _):
      .deviceConfig(serial: serial)
    default:
      nil
    }
  }

  func encode(to encoder: Encoder) throws {
    var container = encoder.container(keyedBy: CodingKeys.self)
    switch self {
    case .subscribeDeviceEvents:
      try container.encode("subscribe_device_events", forKey: .type)
    case .shutdown:
      try container.encode("shutdown", forKey: .type)
    case .getStatus(let requestID):
      try container.encode("get_status", forKey: .type)
      try container.encode(requestID, forKey: .requestID)
    case .getConfig(let requestID):
      try container.encode("get_config", forKey: .type)
      try container.encode(requestID, forKey: .requestID)
    case .saveConfig(let source, let daemon, let ipod, let requestID):
      try container.encode("save_config", forKey: .type)
      try container.encodeIfPresent(source, forKey: .source)
      try container.encodeIfPresent(daemon, forKey: .daemon)
      try container.encodeIfPresent(ipod, forKey: .ipod)
      try container.encode(requestID, forKey: .requestID)
    case .forgetIpod(let serial, let requestID):
      try container.encode("forget_ipod", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .triggerSync(let source, let serial, let requestID):
      try container.encode("trigger_sync", forKey: .type)
      try container.encode(source, forKey: .source)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .cancelSync(let serial, let requestID):
      try container.encode("cancel_sync", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .pause(let serial, let requestID):
      try container.encode("pause", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .decidePrompt(let id, let choice, let serial, let requestID):
      try container.encode("decide_prompt", forKey: .type)
      try container.encode(id, forKey: .id)
      try container.encode(choice, forKey: .choice)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .backfillRockbox(let serial, let requestID):
      try container.encode("backfill_rockbox", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .getLibrary(let requestID):
      try container.encode("get_library", forKey: .type)
      try container.encode(requestID, forKey: .requestID)
    case .scanLibrary(let requestID):
      try container.encode("scan_library", forKey: .type)
      try container.encode(requestID, forKey: .requestID)
    case .retrySourceMount(let allowUI, let requestID):
      try container.encode("retry_source_mount", forKey: .type)
      try container.encode(allowUI, forKey: .allowUI)
      try container.encode(requestID, forKey: .requestID)
    case .previewSelection(let mode, let rules, let serial, let requestID):
      try container.encode("preview_selection", forKey: .type)
      try container.encode(mode, forKey: .mode)
      try container.encode(rules, forKey: .rules)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .getHistory(let limit, let requestID):
      try container.encode("get_history", forKey: .type)
      try container.encode(limit, forKey: .limit)
      try container.encode(requestID, forKey: .requestID)
    case .replaceLibrary(let serial, let requestID):
      try container.encode("replace_library", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .listPlaylists(let requestID):
      try container.encode("list_playlists", forKey: .type)
      try container.encode(requestID, forKey: .requestID)
    case .getPlaylist(let slug, let requestID):
      try container.encode("get_playlist", forKey: .type)
      try container.encode(slug, forKey: .slug)
      try container.encode(requestID, forKey: .requestID)
    case .savePlaylist(let playlist, let requestID):
      try container.encode("save_playlist", forKey: .type)
      try container.encode(playlist, forKey: .playlist)
      try container.encode(requestID, forKey: .requestID)
    case .deletePlaylist(let slug, let requestID):
      try container.encode("delete_playlist", forKey: .type)
      try container.encode(slug, forKey: .slug)
      try container.encode(requestID, forKey: .requestID)
    case .getDeviceConfig(let serial, let requestID):
      try container.encode("get_device_config", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .saveDeviceConfig(
      let serial, let selection, let subscriptions, let settings, let requestID):
      try container.encode("save_device_config", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encodeIfPresent(selection, forKey: .selection)
      try container.encodeIfPresent(subscriptions, forKey: .subscriptions)
      try container.encodeIfPresent(settings, forKey: .settings)
      try container.encode(requestID, forKey: .requestID)
    case .previewDevice(let serial, let requestID):
      try container.encode("preview_device", forKey: .type)
      try container.encode(serial, forKey: .serial)
      try container.encode(requestID, forKey: .requestID)
    case .resolveTracks(let rules, let requestID):
      try container.encode("resolve_tracks", forKey: .type)
      try container.encode(rules, forKey: .rules)
      try container.encode(requestID, forKey: .requestID)
    }
  }
}

extension PlaylistPayload {
  fileprivate var slug: String? {
    switch self {
    case .manual(let slug, _, _), .smart(let slug, _, _): slug
    }
  }
}
