import Foundation

enum DropSyncBehaviorWire: String, Codable, CaseIterable, Sendable {
  case immediate
  case nextSync = "next_sync"
}

// MARK: - Library selection

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

/// Selection value nested under protocol v3 device configuration.
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

// MARK: - Playlists & per-device config

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
  var playlistRevision: UInt64 = 0
  var acknowledgedRequestID: String
}

/// Subscriptions nested under protocol v3 device configuration — the
/// subscribed playlist slugs only (no
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

/// Per-device settings nested under protocol v3 device configuration
/// (no `version`; same rationale as
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



// Wire contract: docs/ipc-protocol.md. Field names mirror Rust protocol v3
// values where these presentation models are encoded as command payloads.
