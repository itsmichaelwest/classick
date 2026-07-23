#if DEBUG
  import Foundation

  extension WireV3Event {
    private static let previewUUID = UUID(uuidString: "00000000-0000-4000-8000-000000000001")!

    static func hello(protocolVersion _: String, coreVersion: String) -> Self {
      .hello(
        WireV3Hello(
          protocolVersion: "3.0.0", role: .daemon, softwareVersion: coreVersion,
          capabilities: WireV3Codec.daemonCapabilities))
    }

    static func configUpdate(
      source: String?, daemon: DaemonSettings?, ipod _: IpodIdentity?, configRevision: UInt64,
      acknowledgedRequestID: String?
    ) -> Self {
      .globalConfig(
        WireV3GlobalConfigEvent(
          requestID: fixtureUUID(acknowledgedRequestID), revision: configRevision,
          sourceRoot: source, settings: WireV3GlobalSettings(daemon ?? previewDaemonSettings)))
    }

    static func deviceConnected(
      serial: String, modelLabel: String, drive: String, name: String?
    ) -> Self {
      .deviceInventory(
        WireV3DeviceInventory(
          requestID: nil, revision: 0,
          devices: [
            identifiedDevice(
              id: fixtureDeviceID(serial), modelLabel: modelLabel, name: name,
              configured: true, connected: true, mount: drive, phase: "idle")
          ], unidentified: []))
    }

    static func deviceDisconnected(serial _: String) -> Self {
      .deviceInventory(
        WireV3DeviceInventory(
          requestID: nil, revision: UInt64.max, devices: [], unidentified: []))
    }

    static func deviceInventorySnapshot(_ snapshot: DeviceInventorySnapshot) -> Self {
      .deviceInventory(
        WireV3DeviceInventory(
          requestID: nil, revision: snapshot.revision,
          devices: snapshot.devices.map { wire in
            identifiedDevice(
              id: fixtureDeviceID(wire.identity.serial), modelLabel: wire.identity.modelLabel,
              name: wire.identity.name, configured: wire.configured, connected: wire.connected,
              mount: wire.mount, phase: wire.phase.rawValue, sessionID: wire.sessionID,
              storage: wire.storage.map {
                WireV3Storage(totalBytes: $0.total, freeBytes: $0.free, freshness: "cached")
              }, syncedCount: wire.syncedCount, libraryCount: wire.libraryCount,
              error: wire.lastTerminalError)
          }, unidentified: []))
    }

    static func statusUpdate(_: StatusInfo) -> Self {
      .inventorySubscriptionChanged(
        WireV3InventorySubscriptionEvent(requestID: previewUUID, subscribed: true))
    }

    static func libraryUpdate(_ info: LibraryInfo) -> Self {
      .library(
        WireV3LibraryEvent(
          requestID: fixtureUUID(info.acknowledgedRequestID), sourceRoot: info.sourceRoot,
          scannedAtUnixSecs: info.scannedAtUnixSecs, artists: info.artists, genres: info.genres,
          totalTracks: info.totalTracks, totalBytes: info.totalBytes))
    }

    static func historyUpdate(
      entries: [HistoryEntry], acknowledgedRequestID: String
    ) -> Self {
      let wireEntries = entries.map { entry -> WireV3HistoryEntry in
        let object: [String: Any] = [
          "device_id": fixtureDeviceID(entry.serial).rawValue,
          "session_id": entry.sessionID as Any,
          "timestamp": entry.timestamp,
          "duration_secs": entry.durationSecs,
          "trigger": entry.trigger,
          "operation": "sync",
          "outcome": entry.outcome,
          "summary": entry.summary.flatMap { try? encodedObject($0) } as Any,
          "db_restored": entry.dbRestored,
        ]
        return try! JSONDecoder().decode(
          WireV3HistoryEntry.self, from: try! JSONSerialization.data(withJSONObject: object))
      }
      return .history(
        WireV3HistoryEvent(
          requestID: fixtureUUID(acknowledgedRequestID) ?? previewUUID, entries: wireEntries))
    }

    static func playlistsUpdate(
      _ playlists: [PlaylistSummary], playlistRevision: UInt64,
      acknowledgedRequestID: String?
    ) -> Self {
      .playlists(
        WireV3PlaylistsEvent(
          requestID: fixtureUUID(acknowledgedRequestID), revision: playlistRevision,
          playlists: playlists))
    }

    static func playlistDetail(_ detail: PlaylistDetail) -> Self {
      let result: WireV3PlaylistDetailResult
      if let error = detail.error {
        result = .unavailable(error)
      } else if detail.kind == .smart, let name = detail.name, let rules = detail.rules {
        result = .found(.smart(slug: detail.slug, name: name, rules: rules))
      } else {
        result = .found(
          .manual(slug: detail.slug, name: detail.name ?? detail.slug, tracks: detail.tracks ?? []))
      }
      return .playlistDetail(
        WireV3PlaylistDetailEvent(
          requestID: fixtureUUID(detail.acknowledgedRequestID) ?? previewUUID,
          revision: detail.playlistRevision, slug: detail.slug, result: result))
    }

    static func deviceConfigUpdate(
      serial: String, selection: SelectionState, subscriptions: SubscriptionsWire,
      settings: DeviceSettingsWire, selectionRevision: UInt64, settingsRevision: UInt64,
      subscriptionsRevision: UInt64, acknowledgedRequestID _: String?
    ) -> Self {
      let delivery = WireV3Delivery(state: "device_committed", lastFailure: nil)
      return .deviceConfig(
        WireV3DeviceConfig(
          requestID: nil, deviceID: fixtureDeviceID(serial),
          selection: .init(
            revision: selectionRevision, mutationID: previewUUID, value: selection,
            delivery: delivery),
          settings: .init(
            revision: settingsRevision, mutationID: previewUUID, value: settings,
            delivery: delivery),
          subscriptions: .init(
            revision: subscriptionsRevision, mutationID: previewUUID, value: subscriptions,
            delivery: delivery)))
    }

    static func devicePreview(_ preview: DevicePreview) -> Self {
      .devicePreview(
        WireV3DevicePreviewEvent(
          deviceID: fixtureDeviceID(preview.serial),
          requestID: fixtureUUID(preview.acknowledgedRequestID) ?? previewUUID,
          selectedTracks: preview.selectedTracks, selectedBytes: preview.selectedBytes,
          playlistExtraTracks: preview.playlistExtraTracks,
          playlistExtraBytes: preview.playlistExtraBytes,
          projectedFreeBytes: preview.projectedFreeBytes,
          unresolvedSubscriptions: preview.unresolvedSubscriptions ?? []))
    }

    static func deviceSelectionAdded(_ info: DeviceSelectionAddedInfo) -> Self {
      let sync: WireV3DropSyncResult
      switch info.delivery {
      case .addedAndSyncing: sync = .started(sessionID: 1)
      case .addedForNextSync: sync = .nextSync
      case .alreadyPresent: sync = .alreadyPresent
      }
      return .deviceSelectionAdded(
        WireV3DeviceSelectionAddedEvent(
          deviceID: fixtureDeviceID(info.serial),
          requestID: fixtureUUID(info.acknowledgedRequestID) ?? previewUUID,
          mutationID: previewUUID, matchedTracks: info.matchedTracks,
          missingTracks: info.missingTracks, selectionChanged: info.selectionChanged,
          selectionRevision: info.selectionRevision,
          selection: WireV3SelectionValue(info.selection),
          delivery: WireV3Delivery(state: "pending_device", lastFailure: nil), sync: sync))
    }

    static func playlistSelectionAppended(_ info: PlaylistSelectionAppendedInfo) -> Self {
      .playlistSelectionAppended(
        WireV3PlaylistSelectionAppendedEvent(
          requestID: fixtureUUID(info.acknowledgedRequestID) ?? previewUUID,
          slug: info.slug, appendedTracks: info.appendedTracks,
          revision: info.playlistRevision,
          playlist: .manual(
            slug: info.playlist.slug, name: info.playlist.name, tracks: info.playlist.tracks)))
    }

    static func libraryMutationRejected(_ info: LibraryMutationRejectedInfo) -> Self {
      let target: WireV3LibraryMutationTarget
      switch info.target {
      case .deviceSelection(let serial): target = .deviceSelection(fixtureDeviceID(serial))
      case .manualPlaylist(let slug): target = .manualPlaylist(slug)
      }
      return .libraryMutationRejected(
        WireV3LibraryMutationRejectedEvent(
          requestID: fixtureUUID(info.acknowledgedRequestID) ?? previewUUID,
          target: target, code: info.code, message: info.message))
    }

    static func syncRejected(
      reason: String, serial: String, acknowledgedRequestID: String
    ) -> Self {
      .syncRejected(
        WireV3SyncRejectedEvent(
          deviceID: fixtureDeviceID(serial),
          requestID: fixtureUUID(acknowledgedRequestID) ?? previewUUID,
          operation: "sync", reason: reason, message: reason))
    }

    static func selectionUpdate(
      mode _: SelectionMode, rules _: [SelectionRule], serial _: String?,
      acknowledgedRequestID _: String?
    ) -> Self {
      .inventorySubscriptionChanged(
        WireV3InventorySubscriptionEvent(requestID: previewUUID, subscribed: true))
    }

    static func selectionPreview(_ info: SelectionPreviewInfo) -> Self {
      .selectionPreview(
        WireV3SelectionPreviewEvent(
          deviceID: fixtureDeviceID(info.serial),
          requestID: fixtureUUID(info.acknowledgedRequestID) ?? previewUUID,
          selectedTracks: info.selectedTracks, selectedBytes: info.selectedBytes,
          adds: info.adds, removes: info.removes))
    }

    static func resolvedTracks(tracks: [String], acknowledgedRequestID: String) -> Self {
      .resolvedTracks(
        WireV3ResolvedTracksEvent(
          requestID: fixtureUUID(acknowledgedRequestID) ?? previewUUID, tracks: tracks))
    }

    static func sourceAvailability(_ info: SourceAvailabilityInfo) -> Self {
      .sourceAvailability(
        WireV3SourceAvailabilityEvent(
          requestID: fixtureUUID(info.acknowledgedRequestID), state: info.state,
          sourceRoot: info.sourceRoot))
    }

    static func syncEvent(line: String, serial: String?, sessionID: UInt64) -> Self {
      guard let serial else {
        return .inventorySubscriptionChanged(
          WireV3InventorySubscriptionEvent(requestID: previewUUID, subscribed: true))
      }
      var object = try! JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
      object["type"] = progressType(object["type"] as? String ?? "")
      object["device_id"] = fixtureDeviceID(serial).rawValue
      object["session_id"] = sessionID
      return decodeFixture(try! JSONSerialization.data(withJSONObject: object))
    }

    private static func identifiedDevice(
      id: DeviceID, modelLabel: String, name: String?, configured: Bool, connected: Bool,
      mount: String?, phase: String, sessionID: UInt64? = nil, storage: WireV3Storage? = nil,
      syncedCount: Int = 0, libraryCount: Int? = nil, error: String? = nil
    ) -> WireV3IdentifiedDevice {
      WireV3IdentifiedDevice(
        deviceID: id, name: name, readiness: "ready",
        hardware: WireV3Hardware(
          family: WireV3HardwareFact(
            value: modelLabel, source: "preview_fixture", confidence: "certain"),
          generation: nil, modelCode: nil, colour: nil, firmware: nil, capacityBytes: nil),
        profileStatus: configured ? "adopted" : "not_adopted", connected: connected,
        mountPath: mount, phase: phase, sessionID: sessionID, storage: storage,
        syncedCount: syncedCount, libraryCount: libraryCount, lastTerminalError: error)
    }

    private static func fixtureUUID(_ value: String?) -> UUID? {
      value.flatMap(UUID.init(uuidString:))
    }

    private static func fixtureDeviceID(_ value: String) -> DeviceID {
      if let canonical = try? DeviceID(value) { return canonical }
      let hexadecimal = value.uppercased()
      guard hexadecimal.count <= 16, hexadecimal.allSatisfy(\.isHexDigit) else {
        let hash = value.utf8.reduce(UInt64(1_469_598_103_934_665_603)) {
          ($0 ^ UInt64($1)) &* 1_099_511_628_211
        }
        return try! DeviceID(String(format: "%016llX", hash))
      }
      return try! DeviceID(String(repeating: "0", count: 16 - hexadecimal.count) + hexadecimal)
    }

    private static func encodedObject<T: Encodable>(_ value: T) throws -> Any {
      try JSONSerialization.jsonObject(with: JSONEncoder().encode(value))
    }

    private static func decodeFixture(_ data: Data) -> Self {
      guard
        case .event(let event) = try! WireV3Codec.decode(
          data, direction: .daemonToDesktopEvents)
      else { preconditionFailure("preview fixture is not a v3 event") }
      return event
    }

    private static func progressType(_ oldType: String) -> String {
      [
        "header": "run_header", "summary": "sync_summary", "finish": "sync_finished",
        "cancelled": "sync_cancelled", "paused": "sync_paused", "log": "sync_log",
        "error": "sync_error",
      ][oldType] ?? oldType
    }

    private static let previewDaemonSettings = DaemonSettings(
      enabled: true, autostartWithWindows: false, firstSyncMode: "review",
      subsequentSyncMode: "auto_apply", scheduleMinutes: 30, notifyOn: "all",
      rockboxCompat: false, dropSyncBehavior: .immediate)
  }
#endif
