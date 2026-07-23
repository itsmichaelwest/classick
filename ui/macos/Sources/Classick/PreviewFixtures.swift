#if DEBUG
  import Foundation

  /// Canned `AppModel`s and wire values for `#Preview` blocks across every
  /// view in `Views/`. Nothing here is reachable from a Release build (see the
  /// `#if DEBUG` wrapping this whole file, mirroring `AppModel.seedPreviewStorage`)
  /// — it exists purely so Xcode's canvas (and a developer hand-tweaking layout)
  /// has realistic, deterministic data to render against, without spinning up a
  /// real daemon connection.
  ///
  /// State is built the same way production code builds it: a fresh `AppModel`
  /// fed a sequence of synthetic `WireV3Event`s through `apply(_:)`, mirroring
  /// the order `AppDelegate` itself would send them in (config → device →
  /// status → library → history → playlists → device config → preview). The
  /// one field with no event-based path (`deviceStorage`/`storageText`) goes
  /// through `AppModel.seedPreviewStorage` instead — see that method's doc
  /// comment for why.
  @MainActor
  enum PreviewFixtures {
    private static let requestID = "00000000-0000-4000-8000-000000000001"

    // MARK: - Library (populated)

    /// Five artists, mixed album sizes (5–23 tracks), a five-genre spread
    /// (Electronic/Alternative Rock/Rock/Jazz/Pop), and — per the preview
    /// brief — deliberately non-round per-album track counts throughout
    /// (17, 23, 12, 11, 5, 13...) rather than suspiciously tidy 10s/20s, so
    /// singular/plural label formatting ("1 track" vs. "N tracks") and
    /// column alignment get exercised with realistic numbers.
    static let boardsOfCanada = LibraryArtist(
      name: "Boards of Canada",
      albums: [
        LibraryAlbum(
          name: "Music Has the Right to Children", genre: "Electronic", tracks: 17,
          bytes: 595_000_000),
        LibraryAlbum(name: "Geogaddi", genre: "Electronic", tracks: 23, bytes: 805_000_000),
      ])
    static let radiohead = LibraryArtist(
      name: "Radiohead",
      albums: [
        LibraryAlbum(
          name: "OK Computer", genre: "Alternative Rock", tracks: 12, bytes: 408_000_000),
        LibraryAlbum(
          name: "In Rainbows", genre: "Alternative Rock", tracks: 10, bytes: 340_000_000),
      ])
    static let fleetwoodMac = LibraryArtist(
      name: "Fleetwood Mac",
      albums: [
        LibraryAlbum(name: "Rumours", genre: "Rock", tracks: 11, bytes: 330_000_000)
      ])
    static let milesDavis = LibraryArtist(
      name: "Miles Davis",
      albums: [
        LibraryAlbum(name: "Kind of Blue", genre: "Jazz", tracks: 5, bytes: 225_000_000)
      ])
    static let beyonce = LibraryArtist(
      name: "Beyoncé",
      albums: [
        LibraryAlbum(name: "Lemonade", genre: "Pop", tracks: 13, bytes: 416_000_000)
      ])

    static let libraryArtists = [boardsOfCanada, radiohead, fleetwoodMac, milesDavis, beyonce]

    static let libraryGenres = [
      LibraryGenre(name: "Electronic", tracks: 40, bytes: 1_400_000_000),
      LibraryGenre(name: "Alternative Rock", tracks: 22, bytes: 748_000_000),
      LibraryGenre(name: "Rock", tracks: 11, bytes: 330_000_000),
      LibraryGenre(name: "Jazz", tracks: 5, bytes: 225_000_000),
      LibraryGenre(name: "Pop", tracks: 13, bytes: 416_000_000),
    ]

    static let musicFolderPath = "/Users/michael/Music/Classick Library"

    static let richLibrary = LibraryInfo(
      sourceRoot: musicFolderPath,
      scannedAtUnixSecs: 1_752_600_000,
      artists: libraryArtists,
      genres: libraryGenres,
      totalTracks: 91,
      totalBytes: 3_119_000_000)

    /// A completed scan that found nothing — Global Constraints' "library
    /// empty" state.
    static let emptyLibrary = LibraryInfo(
      sourceRoot: musicFolderPath,
      scannedAtUnixSecs: 1_752_600_000,
      artists: [],
      genres: [],
      totalTracks: 0,
      totalBytes: 0)

    // MARK: - Device identity

    static let pairedIpod = IpodIdentity(
      serial: "000A27002138B0A8", modelLabel: "iPod Classic (6th gen, 160GB)",
      name: "Michael's iPod", customSelection: false)

    static let connectedDevice = DeviceState(
      serial: pairedIpod.serial, model: pairedIpod.modelLabel,
      name: pairedIpod.name, drive: "/Volumes/Michael's iPod")

    private static func deviceSnapshot(
      identity: IpodIdentity = pairedIpod,
      configured: Bool = true,
      connected: Bool = true,
      phase: DevicePhaseLabel = .idle,
      sessionID: UInt64? = nil,
      storage: (free: Int64, total: Int64)? = nil,
      syncedCount: Int = 0,
      libraryCount: Int? = nil,
      latestSuccessfulSync: HistoryEntry? = nil,
      latestAttempt: HistoryEntry? = nil,
      lastTerminalError: String? = nil
    ) -> DeviceSnapshotWire {
      DeviceSnapshotWire(
        identity: .init(
          serial: identity.serial, modelLabel: identity.modelLabel, name: identity.name),
        configured: configured,
        connected: connected,
        mount: connected ? connectedDevice.drive : nil,
        phase: phase,
        sessionID: sessionID,
        storage: storage.map {
          .init(free: UInt64(clamping: $0.free), total: UInt64(clamping: $0.total))
        },
        syncedCount: syncedCount,
        libraryCount: libraryCount,
        latestSuccessfulSync: latestSuccessfulSync,
        latestAttempt: latestAttempt,
        lastTerminalError: lastTerminalError,
        selectionRevision: 1,
        settingsRevision: 1,
        subscriptionsRevision: 1)
    }

    static let secondPairedIpod = IpodIdentity(
      serial: "T4U5V6W7X8Y9", modelLabel: "iPod Classic (5th gen, 80GB)",
      name: "Old iPod", customSelection: false)

    // MARK: - History

    static let historyEntries: [HistoryEntry] = [
      HistoryEntry(
        serial: pairedIpod.serial, timestamp: "2026-07-10T09:15:00Z", durationSecs: 612,
        trigger: "plug_in", outcome: "ok"),
      HistoryEntry(
        serial: pairedIpod.serial, timestamp: "2026-07-12T18:42:00Z", durationSecs: 305,
        trigger: "manual", outcome: "ok"),
      HistoryEntry(
        serial: pairedIpod.serial, timestamp: "2026-07-14T08:03:00Z", durationSecs: 12,
        trigger: "scheduled", outcome: "error"),
      // The most recent run also restored the iTunesDB from Classick's own
      // backup — HistoryView needs at least one of these per the preview
      // brief, and it's this run's `lastSync` too (see `connectedSyncedModel`).
      HistoryEntry(
        serial: pairedIpod.serial, timestamp: "2026-07-16T21:11:00Z", durationSecs: 480,
        trigger: "manual", outcome: "ok", dbRestored: true),
    ]

    static var mostRecentSync: HistoryEntry { historyEntries.last! }

    // MARK: - Playlists

    static let roadTripMix = PlaylistSummary(
      slug: "road-trip-mix", name: "Road Trip Mix", kind: .manual, tracks: 11, bytes: 380_000_000,
      error: nil)
    static let nightDrive = PlaylistSummary(
      slug: "night-drive", name: "Night Drive", kind: .manual, tracks: 23, bytes: 780_000_000,
      error: nil)
    static let electronicEssentials = PlaylistSummary(
      slug: "electronic-essentials", name: "Electronic Essentials", kind: .smart, tracks: 40,
      bytes: 1_400_000_000, error: nil)
    static let brokenPlaylist = PlaylistSummary(
      slug: "broken-playlist", name: "Broken Playlist", kind: .smart, tracks: 0, bytes: 0,
      error: "Couldn't parse smart-playlist rules (unknown field \u{201C}mood\u{201D}).")

    static let playlistSummaries = [roadTripMix, electronicEssentials, nightDrive, brokenPlaylist]

    /// ~10 tracks (11, per the brief's "~10"): a mix of zero-padded and
    /// non-zero-padded source filenames (Miles Davis' pair, "1 …"/"2 …",
    /// unpadded — see `ManualPlaylistLogic.appendingTracks`'s doc comment for
    /// why that distinction matters elsewhere in this editor), plus one path
    /// under an artist that isn't in `richLibrary` at all, so the "missing
    /// file" warning icon (`ManualPlaylistLogic.isLikelyMissing`) has
    /// something to point at in the preview.
    static let manualPlaylistDetail = PlaylistDetail(
      slug: roadTripMix.slug,
      name: roadTripMix.name,
      kind: .manual,
      tracks: [
        "Boards of Canada/Music Has the Right to Children/03 Telephasic Workshop.flac",
        "Boards of Canada/Music Has the Right to Children/07 Aquarius.flac",
        "Radiohead/OK Computer/02 Paranoid Android.flac",
        "Radiohead/OK Computer/04 Exit Music (For a Film).flac",
        "Radiohead/In Rainbows/01 15 Step.flac",
        "Fleetwood Mac/Rumours/02 Dreams.flac",
        "Fleetwood Mac/Rumours/04 Go Your Own Way.flac",
        "Miles Davis/Kind of Blue/1 So What.flac",
        "Miles Davis/Kind of Blue/2 Freddie Freeloader.flac",
        "Beyoncé/Lemonade/06 Sorry.flac",
        "The Delta Sound/Untitled Demos/3 Rough Mix.flac",
      ],
      rules: nil,
      error: nil,
      playlistRevision: 1,
      acknowledgedRequestID: requestID)

    static let smartPlaylistDetail = PlaylistDetail(
      slug: electronicEssentials.slug,
      name: electronicEssentials.name,
      kind: .smart,
      tracks: nil,
      rules: SmartRulesWire(
        matching: .all,
        rules: [
          SmartRuleWire(field: .genre, op: .is, value: "Electronic"),
          SmartRuleWire(field: .year, op: .gte, value: "2000"),
        ],
        limit: .bytes(1_500_000_000),
        order: .recentlyModified,
        seed: 42),
      error: nil,
      playlistRevision: 1,
      acknowledgedRequestID: requestID)

    static let brokenPlaylistDetail = PlaylistDetail(
      slug: brokenPlaylist.slug, name: nil, kind: nil, tracks: nil, rules: nil,
      error: brokenPlaylist.error,
      playlistRevision: 1,
      acknowledgedRequestID: requestID)

    /// History outcome strings on the wire are `ok`/`error`/`aborted`
    /// (Rust `SyncOutcome`) — fixtures must use those, not invented values
    /// (wire-audit finding M-1).

    // MARK: - Device capacity / preview

    static let devicePreviewFits = DevicePreview(
      serial: pairedIpod.serial,
      selectedTracks: 91, selectedBytes: 3_119_000_000,
      playlistExtraTracks: 33, playlistExtraBytes: 1_180_000_000,
      projectedFreeBytes: 118_000_000_000, unresolvedSubscriptions: nil,
      acknowledgedRequestID: requestID)

    static let devicePreviewOverfull = DevicePreview(
      serial: pairedIpod.serial,
      selectedTracks: 91, selectedBytes: 3_119_000_000,
      playlistExtraTracks: 40, playlistExtraBytes: 1_400_000_000,
      projectedFreeBytes: 0, unresolvedSubscriptions: [brokenPlaylist.slug],
      acknowledgedRequestID: requestID)

    static let skippedForSpace = SkippedForSpace(albums: 2, tracks: 14, bytes: 512_000_000)
    static let artworkShortfall = ArtworkSummary(embedded: 77, eligible: 84, failedSources: 3)

    // MARK: - Settings

    static func daemonSettings(rockboxCompat: Bool = false) -> DaemonSettings {
      DaemonSettings(
        enabled: true, autostartWithWindows: true,
        firstSyncMode: "auto_apply", subsequentSyncMode: "auto_apply",
        scheduleMinutes: 360, notifyOn: "all", rockboxCompat: rockboxCompat)
    }

    // MARK: - Protocol v3 progress builders

    /// Sync progress is received from the daemon (the app never sends it, only
    /// receives them from the subprocess) — so previews that need a phase
    /// only reachable via typed protocol v3 progress (scanning-in-progress, syncing,
    /// paused, error, finish rollups) hand-write the JSON line, exactly as
    /// the daemon would emit it as flat protocol v3 progress.
    private static func trackStartLine(current: Int, total: Int, label: String, etaSecs: UInt64?)
      -> String
    {
      let etaField = etaSecs.map { ",\"eta_secs\":\($0)" } ?? ""
      return
        "{\"type\":\"track_start\",\"current\":\(current),\"total\":\(total),\"label\":\"\(label)\"\(etaField)}"
    }

    private static let pausedLine = "{\"type\":\"paused\"}"

    private static func errorLine(_ message: String) -> String {
      "{\"type\":\"error\",\"message\":\"\(message)\"}"
    }

    private static let finishOverfullLine =
      "{\"type\":\"finish\",\"success\":true,"
      + "\"skipped_for_space\":{\"albums\":2,\"tracks\":14,\"bytes\":512000000},"
      + "\"artwork\":{\"embedded\":77,\"eligible\":84,\"failed_sources\":3},"
      + "\"db_restored\":false}"

    // MARK: - AppModel builders

    /// Applies the same event sequence `AppDelegate` sends on a normal
    /// connect (config → device → status → library → history → playlists),
    /// then overrides `deviceStorage` with canned numbers. Every scenario
    /// below starts here and layers on whatever makes it distinct.
    private static func baseConnectedModel(
      library: LibraryInfo? = richLibrary,
      syncedCount: Int,
      libraryCount: Int?,
      lastSync: HistoryEntry?,
      storage: (free: Int64, total: Int64) = (free: 42_000_000_000, total: 160_000_000_000)
    ) -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: pairedIpod, configRevision: 1,
          acknowledgedRequestID: requestID))
      m.apply(
        .deviceConnected(
          serial: pairedIpod.serial, modelLabel: pairedIpod.modelLabel,
          drive: connectedDevice.drive, name: pairedIpod.name))
      m.apply(
        .statusUpdate(
          StatusInfo(
            state: .idle, configured: true, ipodConnected: true,
            lastSync: lastSync, nextScheduledUnixSecs: nil, storage: nil,
            syncedCount: syncedCount, libraryCount: libraryCount)))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 1,
            devices: [
              deviceSnapshot(
                storage: storage, syncedCount: syncedCount, libraryCount: libraryCount,
                latestSuccessfulSync: lastSync, latestAttempt: lastSync)
            ])))
      if let library {
        m.apply(.libraryUpdate(library))
      }
      m.apply(.historyUpdate(entries: historyEntries, acknowledgedRequestID: requestID))
      m.apply(
        .playlistsUpdate(
          playlistSummaries, playlistRevision: 1, acknowledgedRequestID: requestID))
      m.seedPreviewStorage(free: storage.free, total: storage.total)
      return m
    }

    /// Device connected, fully synced, entire library. The general-purpose
    /// "everything is fine" fixture most view previews start from.
    static func connectedSyncedModel() -> AppModel {
      let m = baseConnectedModel(syncedCount: 91, libraryCount: 91, lastSync: mostRecentSync)
      m.apply(
        .deviceConfigUpdate(
          serial: pairedIpod.serial,
          selection: SelectionState(mode: .all, rules: []),
          subscriptions: SubscriptionsWire(playlists: [electronicEssentials.slug, roadTripMix.slug]
          ),
          settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1,
          acknowledgedRequestID: requestID))
      m.willRequestDevicePreview(serial: try! DeviceID(pairedIpod.serial), requestID: requestID)
      m.apply(.devicePreview(devicePreviewFits))
      return m
    }

    /// Device connected, "Selected items" mode with a handful of album-level
    /// rules already checked.
    static func connectedSelectedItemsModel() -> AppModel {
      let m = baseConnectedModel(syncedCount: 40, libraryCount: 91, lastSync: mostRecentSync)
      let rules: [SelectionRule] = [
        .artist(name: boardsOfCanada.name),
        .album(artist: radiohead.name, album: "OK Computer"),
        .genre(name: "Jazz"),
      ]
      m.apply(
        .deviceConfigUpdate(
          serial: pairedIpod.serial,
          selection: SelectionState(mode: .include, rules: rules),
          subscriptions: SubscriptionsWire(playlists: [electronicEssentials.slug]),
          settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1,
          acknowledgedRequestID: requestID))
      m.willRequestDevicePreview(serial: try! DeviceID(pairedIpod.serial), requestID: requestID)
      m.apply(
        .devicePreview(
          DevicePreview(
            serial: pairedIpod.serial,
            selectedTracks: 62, selectedBytes: 1_820_000_000,
            playlistExtraTracks: 40, playlistExtraBytes: 1_400_000_000,
            projectedFreeBytes: 92_000_000_000, unresolvedSubscriptions: nil,
            acknowledgedRequestID: requestID)))
      return m
    }

    /// Device connected, entire-library mode, but nothing has synced yet —
    /// Global Constraints' "device empty" state.
    static func connectedNothingSyncedModel() -> AppModel {
      let m = baseConnectedModel(syncedCount: 0, libraryCount: 91, lastSync: nil)
      m.apply(
        .deviceConfigUpdate(
          serial: pairedIpod.serial,
          selection: SelectionState(mode: .all, rules: []),
          subscriptions: SubscriptionsWire(playlists: []),
          settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1,
          acknowledgedRequestID: requestID))
      return m
    }

    /// Device connected, but this run's capacity preview doesn't fit — the
    /// fit-pass had to defer whole albums, and some artwork failed to embed.
    /// Exercises the capacity bar's overflow highlight plus both rollup
    /// lines (`DeviceRowFormatting`).
    static func connectedOverfullModel() -> AppModel {
      let m = baseConnectedModel(
        syncedCount: 77, libraryCount: 91, lastSync: mostRecentSync,
        storage: (free: 1_000_000_000, total: 6_000_000_000))
      m.apply(
        .deviceConfigUpdate(
          serial: pairedIpod.serial,
          selection: SelectionState(mode: .all, rules: []),
          subscriptions: SubscriptionsWire(playlists: [
            electronicEssentials.slug, roadTripMix.slug, nightDrive.slug,
          ]),
          settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1,
          acknowledgedRequestID: requestID))
      m.willRequestDevicePreview(serial: try! DeviceID(pairedIpod.serial), requestID: requestID)
      m.apply(.devicePreview(devicePreviewOverfull))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 2,
            devices: [
              deviceSnapshot(
                phase: .syncing, sessionID: 1,
                storage: (free: 1_000_000_000, total: 6_000_000_000),
                syncedCount: 77, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync)
            ])))
      m.apply(.syncEvent(line: finishOverfullLine, serial: pairedIpod.serial, sessionID: 1))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 3,
            devices: [
              deviceSnapshot(
                storage: (free: 1_000_000_000, total: 6_000_000_000),
                syncedCount: 77, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync)
            ])))
      return m
    }

    /// Paired iPod, currently unplugged — the sidebar's dimmed row / device
    /// pages' "Not connected" caption.
    static func disconnectedModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: pairedIpod, configRevision: 1,
          acknowledgedRequestID: requestID))
      m.apply(.libraryUpdate(richLibrary))
      m.apply(.historyUpdate(entries: historyEntries, acknowledgedRequestID: requestID))
      m.apply(
        .playlistsUpdate(
          playlistSummaries, playlistRevision: 1, acknowledgedRequestID: requestID))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 1,
            devices: [
              deviceSnapshot(
                connected: false, phase: .disconnected, syncedCount: 91, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync)
            ])))
      m.apply(
        .deviceConfigUpdate(
          serial: pairedIpod.serial,
          selection: SelectionState(mode: .all, rules: []),
          subscriptions: SubscriptionsWire(playlists: [electronicEssentials.slug]),
          settings: DeviceSettingsWire(autoSync: true, rockboxCompat: false),
          selectionRevision: 1, settingsRevision: 1, subscriptionsRevision: 1,
          acknowledgedRequestID: requestID))
      return m
    }

    /// iPod physically connected, but the daemon has no persisted pairing for
    /// it yet (or the paired serial doesn't match) — `.notConfigured` phase.
    static func notConfiguredModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: nil, configRevision: 1,
          acknowledgedRequestID: requestID))
      m.apply(.libraryUpdate(richLibrary))
      m.apply(
        .deviceConnected(
          serial: pairedIpod.serial, modelLabel: pairedIpod.modelLabel,
          drive: connectedDevice.drive, name: pairedIpod.name))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 1,
            devices: [deviceSnapshot(configured: false, phase: .unconfigured)])))
      return m
    }

    /// No iPod at all — the very first launch before anything's plugged in,
    /// but the library is already configured and scanned.
    static func noDeviceModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: nil, configRevision: 1,
          acknowledgedRequestID: requestID))
      m.apply(.libraryUpdate(richLibrary))
      m.apply(.historyUpdate(entries: historyEntries, acknowledgedRequestID: requestID))
      m.apply(
        .playlistsUpdate(
          playlistSummaries, playlistRevision: 1, acknowledgedRequestID: requestID))
      return m
    }

    /// Fresh install: the daemon has answered `get_config` with nothing
    /// persisted — `needsFirstRunSetup` is true, so `MainWindow` shows the
    /// setup call-to-action instead of any page.
    static func firstRunModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: nil, daemon: nil, ipod: nil, configRevision: 1, acknowledgedRequestID: requestID))
      return m
    }

    /// Library configured and scanned once already, currently mid-rescan.
    static func scanningModel() -> AppModel {
      let m = baseConnectedModel(syncedCount: 91, libraryCount: 91, lastSync: mostRecentSync)
      m.apply(
        .statusUpdate(
          StatusInfo(
            state: .scanning, configured: true, ipodConnected: true,
            lastSync: mostRecentSync, nextScheduledUnixSecs: nil, storage: nil,
            syncedCount: 91, libraryCount: nil)))
      m.apply(
        .syncEvent(
          line: trackStartLine(current: 47, total: 91, label: "", etaSecs: nil),
          serial: pairedIpod.serial, sessionID: 1))
      return m
    }

    /// Empty library: source configured and scanned, zero tracks found.
    static func emptyLibraryModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: nil, configRevision: 1,
          acknowledgedRequestID: requestID))
      m.apply(.libraryUpdate(emptyLibrary))
      return m
    }

    /// Source configured, but the daemon hasn't completed its first scan.
    static func needsScanModel() -> AppModel {
      let m = AppModel()
      m.apply(
        .configUpdate(
          source: musicFolderPath, daemon: daemonSettings(), ipod: nil, configRevision: 1,
          acknowledgedRequestID: requestID))
      return m
    }

    static func sourceAttentionModel() -> AppModel {
      let m = connectedSyncedModel()
      m.apply(
        .sourceAvailability(
          .init(state: .authRequired, sourceRoot: nil, acknowledgedRequestID: nil)))
      return m
    }

    /// Actively syncing — `Phase.syncing` with a live track label + ETA.
    static func syncingModel() -> AppModel {
      let m = connectedSyncedModel()
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 2,
            devices: [
              deviceSnapshot(
                phase: .syncing, sessionID: 1,
                storage: (free: 42_000_000_000, total: 160_000_000_000),
                syncedCount: 91, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync)
            ])))
      m.apply(
        .syncEvent(
          line: trackStartLine(
            current: 34, total: 91,
            label: "Boards of Canada – Music Has the Right to Children – 07 Aquarius.flac",
            etaSecs: 245), serial: pairedIpod.serial, sessionID: 1))
      return m
    }

    static func finalizingModel() -> AppModel {
      let m = syncingModel()
      m.apply(
        .syncEvent(
          line:
            #"{"type":"finalizing","reason":"cancelled","staged_albums":12,"staged_tracks":91}"#,
          serial: pairedIpod.serial,
          sessionID: 1))
      return m
    }

    /// A sync that's been paused mid-run.
    static func pausedModel() -> AppModel {
      let m = syncingModel()
      m.apply(.syncEvent(line: pausedLine, serial: pairedIpod.serial, sessionID: 1))
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 3,
            devices: [
              deviceSnapshot(
                phase: .paused,
                storage: (free: 42_000_000_000, total: 160_000_000_000),
                syncedCount: 34, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync)
            ])))
      return m
    }

    /// A rejected/failed sync — `Phase.error`.
    static func errorModel() -> AppModel {
      let m = connectedSyncedModel()
      let message = "iTunes is running \u{2014} quit it and try again."
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 2,
            devices: [
              deviceSnapshot(
                phase: .error,
                storage: (free: 42_000_000_000, total: 160_000_000_000),
                syncedCount: 91, libraryCount: 91,
                latestSuccessfulSync: mostRecentSync, latestAttempt: mostRecentSync,
                lastTerminalError: message)
            ])))
      return m
    }

    static func longContentErrorModel() -> AppModel {
      let m = connectedSyncedModel()
      let identity = IpodIdentity(
        serial: pairedIpod.serial,
        modelLabel: pairedIpod.modelLabel,
        name: "Michael's extraordinarily long engraved silver iPod Classic used for road trips",
        customSelection: false)
      let message =
        "Classick could not verify the complete artwork publication for this iPod after saving the database."
      m.apply(
        .deviceInventorySnapshot(
          .init(
            revision: 2,
            devices: [
              deviceSnapshot(
                identity: identity,
                phase: .error,
                storage: (free: 42_000_000_000, total: 160_000_000_000),
                syncedCount: 91,
                libraryCount: 91,
                latestSuccessfulSync: mostRecentSync,
                latestAttempt: mostRecentSync,
                lastTerminalError: message)
            ])))
      return m
    }

    /// Fixture stand-in for the daemon's `get_playlist` reply, so preview
    /// hosts can answer `onGetPlaylist` and playlist pages actually load in
    /// the canvas (a no-op closure left them on "Loading…" forever). Slugs
    /// without a dedicated detail get a synthesized manual list rather than
    /// an eternal spinner.
    static func playlistDetail(forSlug slug: String) -> PlaylistDetail {
      switch slug {
      case roadTripMix.slug: return manualPlaylistDetail
      case electronicEssentials.slug: return smartPlaylistDetail
      case brokenPlaylist.slug: return brokenPlaylistDetail
      case nightDrive.slug:
        return PlaylistDetail(
          slug: nightDrive.slug, name: nightDrive.name, kind: .manual,
          tracks: Array(manualPlaylistDetail.tracks?.prefix(6) ?? []),
          rules: nil, error: nil, playlistRevision: 1, acknowledgedRequestID: requestID)
      default:
        return PlaylistDetail(
          slug: slug, name: nil, kind: nil, tracks: nil, rules: nil,
          error: "No fixture detail for slug “\(slug)”.",
          playlistRevision: 1,
          acknowledgedRequestID: requestID)
      }
    }

    /// Playlist editor pages: seeds `model.playlistDetail` with the given
    /// detail (and `model.playlists`/`model.library` so the surrounding
    /// chrome — preview footer, missing-track detection — has something
    /// real to read).
    static func playlistDetailModel(_ detail: PlaylistDetail) -> AppModel {
      let m = AppModel()
      m.apply(.libraryUpdate(richLibrary))
      m.apply(
        .playlistsUpdate(
          playlistSummaries, playlistRevision: 1, acknowledgedRequestID: requestID))
      m.apply(.playlistDetail(detail))
      return m
    }

    /// `PlaylistPage`'s loading state: `get_playlist` sent, no reply yet.
    static func playlistLoadingModel() -> AppModel {
      let m = AppModel()
      m.apply(.libraryUpdate(richLibrary))
      m.apply(
        .playlistsUpdate(
          playlistSummaries, playlistRevision: 1, acknowledgedRequestID: requestID))
      return m
    }
  }
#endif
