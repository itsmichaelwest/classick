import Foundation
import XCTest

@testable import Classick

final class WireProtocol3Tests: XCTestCase {
  private let expectedRoute = WireV3Route(
    deviceID: DeviceID("000A27002138B0A8"), sessionID: 42)

  func testSharedHelloVectorsAndAdmission() throws {
    let manifest = try loadManifest()
    for vector in manifest.vectors {
      let data = try fixture(vector.path)
      switch vector.expectation {
      case "valid_hello":
        let hello = try WireV3Codec.decodeInitialHello(data)
        XCTAssertEqual(hello.role.rawValue, vector.expectedRole)
      case "admission_failure":
        guard case .incompatible = WireV3Codec.admitDaemonHello(data) else {
          return XCTFail("admitted \(vector.path)")
        }
      case "decode_failure":
        XCTAssertThrowsError(try WireV3Codec.decodeInitialHello(data), vector.path)
      case "canonicalize_hello":
        let hello = try WireV3Codec.decodeInitialHello(data)
        XCTAssertEqual(hello.capabilities, hello.capabilities.sorted())
      case "ignored_desktop_event":
        guard
          case .ignoredUnknownEvent(type: "future_device_hint") = try WireV3Codec.decode(
            data, direction: .daemonToDesktopEvents)
        else { return XCTFail("did not ignore \(vector.path)") }
      default:
        XCTFail("unknown expectation \(vector.expectation ?? "nil")")
      }
    }

    let daemon = try fixture("valid/hello-daemon.json")
    guard case .compatible(let hello) = WireV3Codec.admitDaemonHello(daemon) else {
      return XCTFail("valid daemon was rejected")
    }
    XCTAssertEqual(hello.role, .daemon)
    XCTAssertEqual(Set(hello.capabilities), Set(WireV3Codec.daemonCapabilities))
  }

  func testAllSharedPositiveCollectionsDecode() throws {
    let manifest = try loadManifest()
    for collection in manifest.progress.positiveCollections
      + manifest.device.positiveCollections + manifest.operations.positiveCollections
    {
      let direction = try direction(collection.stream, vector: nil)
      for (index, line) in try fixtureLines(collection.path).enumerated() {
        XCTAssertNoThrow(
          try WireV3Codec.decode(line, direction: direction),
          "\(collection.path):\(index + 1)")
      }
    }
  }

  func testAllSharedNegativeVectorsAreRejected() throws {
    let manifest = try loadManifest()
    for vector in manifest.progress.negativeVectors
      + manifest.device.negativeVectors + manifest.operations.negativeVectors
    {
      guard let stream = vector.stream else { throw FixtureError.invalidManifest }
      let direction = try direction(stream, vector: vector)
      XCTAssertThrowsError(
        try WireV3Codec.decode(try fixture(vector.path), direction: direction), vector.path)
    }
  }

  func testProgressIsFlatTypedAndRouted() throws {
    let lines = try fixtureLines("progress/events.ndjson")
    let trackDone = try XCTUnwrap(
      lines.first { data in
        (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["type"] as? String
          == "track_done"
      })
    guard
      case .event(.progress(let event)) = try WireV3Codec.decode(
        trackDone, direction: .daemonToDesktopEvents)
    else { return XCTFail("track_done was not typed progress") }
    XCTAssertEqual(event.kind, .trackDone)
    XCTAssertEqual(event.route, expectedRoute)

    let object = try XCTUnwrap(
      JSONSerialization.jsonObject(with: trackDone) as? [String: Any])
    XCTAssertNil(object["line"])
    XCTAssertNil(object["sync_event"])
  }

  func testDeviceIDRejectsNoncanonicalForms() throws {
    XCTAssertEqual(DeviceID("000A27002138B0A8").rawValue, "000A27002138B0A8")
    for invalid in ["0x000A27002138B0A8", "000a27002138b0a8", "000A27002138B0A", ""] {
      XCTAssertThrowsError(try DeviceID(invalid), invalid)
    }
  }

  func testDeviceInventoryAndConfigDecodeAsTypedEvents() throws {
    let lines = try fixtureLines("device/events.ndjson")

    guard
      case .event(.deviceInventory(let inventory)) = try WireV3Codec.decode(
        lines[0], direction: .daemonToDesktopEvents)
    else { return XCTFail("device inventory was not typed") }
    XCTAssertEqual(inventory.devices.first?.deviceID, "000A27002138B0A8")
    XCTAssertEqual(inventory.devices.first?.hardware.family?.value, "classic")
    XCTAssertEqual(inventory.unidentified.first?.observationID.rawValue, 7)

    guard
      case .event(.deviceConfig(let config)) = try WireV3Codec.decode(
        lines[3], direction: .daemonToDesktopEvents)
    else { return XCTFail("device config was not typed") }
    XCTAssertEqual(config.deviceID, "000A27002138B0A8")
    XCTAssertFalse(config.settings.value.autoSync)
    XCTAssertEqual(config.settings.delivery.state, "pending_device")
  }

  @MainActor
  func testSharedGoldensDriveProductionReducer() throws {
    let model = AppModel()
    let deviceLines = try fixtureLines("device/events.ndjson")
    let operationLines = try fixtureLines("operations/events.ndjson")
    let deviceID = DeviceID("000A27002138B0A8")
    let configRequestID = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8765"

    model.apply(try event(deviceLines[0]))
    model.willRequestDeviceConfig(
      serial: deviceID, requestID: configRequestID, intent: .read)
    model.apply(try event(deviceLines[3]))
    model.apply(try event(operationLines[0]))
    model.apply(try event(operationLines[5]))
    model.apply(try event(operationLines[12]))

    XCTAssertEqual(model.devices[deviceID]?.identity.name, "Michael West's iPod")
    XCTAssertEqual(model.devices[deviceID]?.config?.settings.autoSync, false)
    XCTAssertEqual(model.config?.source, "/Volumes/Music/FLAC")
    XCTAssertEqual(model.library?.totalTracks, 11)
    XCTAssertEqual(model.playlists.map(\.slug), ["favourites"])
  }

  func testLibraryScanRequestCorrelationIsOptionalWithoutBeingSynthesized() throws {
    let requested = Data(
      #"{"type":"library_scan_started","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808","session_id":43}"#
        .utf8)
    guard
      case .event(.libraryScan(let requestedEvent)) = try WireV3Codec.decode(
        requested, direction: .daemonToDesktopEvents)
    else { return XCTFail("requested scan was not typed") }
    XCTAssertEqual(
      requestedEvent.requestID?.uuidString.lowercased(),
      "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808")
    XCTAssertEqual(requestedEvent.sessionID, 43)

    let unsolicited = Data(
      #"{"type":"library_scan_progress","session_id":44,"files_scanned":250,"tracks_indexed":220}"#
        .utf8)
    guard
      case .event(.libraryScan(let unsolicitedEvent)) = try WireV3Codec.decode(
        unsolicited, direction: .daemonToDesktopEvents)
    else { return XCTFail("unsolicited scan was not typed") }
    XCTAssertNil(unsolicitedEvent.requestID)
    XCTAssertEqual(unsolicitedEvent.sessionID, 44)
    XCTAssertEqual(unsolicitedEvent.filesScanned, 250)
    XCTAssertEqual(unsolicitedEvent.tracksIndexed, 220)
  }

  func testLibraryScanStillRequiresSessionIdentity() throws {
    let missingSession = Data(
      #"{"type":"library_scan_finished","success":true}"#.utf8)
    XCTAssertThrowsError(
      try WireV3Codec.decode(missingSession, direction: .daemonToDesktopEvents))
  }

  func testInventoryRejectsSessionOutsideSyncingAndUnadoptedSyncing() throws {
    let idleWithSession = Data(
      #"{"type":"device_inventory","revision":1,"devices":[{"device_id":"000A27002138B0A8","readiness":"ready","hardware":{},"profile_status":"adopted","connected":true,"mount_path":"/Volumes/iPod","phase":"idle","session_id":42,"synced_count":0}],"unidentified":[]}"#.utf8)
    XCTAssertThrowsError(
      try WireV3Codec.decode(idleWithSession, direction: .daemonToDesktopEvents))

    let unadoptedSyncing = Data(
      #"{"type":"device_inventory","revision":1,"devices":[{"device_id":"000A27002138B0A8","readiness":"ready","hardware":{},"profile_status":"not_adopted","connected":true,"mount_path":"/Volumes/iPod","phase":"syncing","session_id":42,"synced_count":0}],"unidentified":[]}"#.utf8)
    XCTAssertThrowsError(
      try WireV3Codec.decode(unadoptedSyncing, direction: .daemonToDesktopEvents))
  }

  func testKnownWrongDirectionAndRepeatedHelloAreRejected() throws {
    let command = try XCTUnwrap(try fixtureLines("progress/commands.ndjson").first)
    XCTAssertThrowsError(
      try WireV3Codec.decode(command, direction: .daemonToDesktopEvents))
    let event = try XCTUnwrap(try fixtureLines("progress/events.ndjson").first)
    XCTAssertThrowsError(
      try WireV3Codec.decode(event, direction: .desktopToDaemonCommands))
    XCTAssertThrowsError(
      try WireV3Codec.decode(
        try fixture("valid/hello-daemon.json"), direction: .daemonToDesktopEvents))
  }

  func testEveryCommandEncodesCanonicalLowercaseUUIDs() throws {
    let requestID = UUID(uuidString: "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF")!
    let mutationID = UUID(uuidString: "FEDCBAFE-DCBA-4FED-8CBA-FEDCBAFEDCBA")!
    let deviceID = DeviceID("000A27002138B0A8")
    let route = WireV3Route(deviceID: deviceID, sessionID: 42)
    let selection = WireV3SelectionValue(.init(mode: .all, rules: []))
    let settings = WireV3SettingsValue(.init(autoSync: true, rockboxCompat: false))
    let subscriptions = WireV3SubscriptionsValue(.init(playlists: []))
    let globalSettings = WireV3GlobalSettings(
      .init(
        enabled: true, autostartWithWindows: false, firstSyncMode: "review",
        subsequentSyncMode: "auto_apply", scheduleMinutes: 30, notifyOn: "all",
        rockboxCompat: false, dropSyncBehavior: .immediate))
    let commands: [WireV3Command] = [
      .getGlobalConfig(requestID: requestID),
      .setSourceLocation(requestID: requestID, sourceRoot: "/Music"),
      .setGlobalSettings(requestID: requestID, settings: globalSettings),
      .getInventory(requestID: requestID),
      .subscribeInventory(requestID: requestID),
      .unsubscribeInventory(requestID: requestID),
      .adoptDevice(
        deviceID: deviceID, requestID: requestID,
        selectionMutationID: mutationID, selection: selection,
        settingsMutationID: mutationID, settings: settings,
        subscriptionsMutationID: mutationID, subscriptions: subscriptions),
      .forgetDevice(deviceID: deviceID, requestID: requestID),
      .getDeviceConfig(deviceID: deviceID, requestID: requestID),
      .setSelection(
        deviceID: deviceID, requestID: requestID, mutationID: mutationID,
        selection: selection),
      .setSettings(
        deviceID: deviceID, requestID: requestID, mutationID: mutationID,
        settings: settings),
      .setSubscriptions(
        deviceID: deviceID, requestID: requestID, mutationID: mutationID,
        subscriptions: subscriptions),
      .triggerSync(deviceID: deviceID, requestID: requestID, trigger: .manual),
      .backfillRockbox(deviceID: deviceID, requestID: requestID),
      .replaceLibrary(deviceID: deviceID, requestID: requestID),
      .getHistory(requestID: requestID, limit: 50),
      .getLibrary(requestID: requestID),
      .scanLibrary(requestID: requestID),
      .retrySourceMount(requestID: requestID, allowUI: true),
      .previewSelection(deviceID: deviceID, requestID: requestID, selection: selection),
      .previewDevice(deviceID: deviceID, requestID: requestID),
      .resolveTracks(requestID: requestID, rules: []),
      .addSelectionToDevice(
        deviceID: deviceID, requestID: requestID, mutationID: mutationID, rules: []),
      .listPlaylists(requestID: requestID),
      .getPlaylist(requestID: requestID, slug: "mix"),
      .savePlaylist(
        requestID: requestID,
        playlist: .manual(slug: "mix", name: "Mix", tracks: [])),
      .deletePlaylist(requestID: requestID, slug: "mix"),
      .appendSelectionToPlaylist(requestID: requestID, slug: "mix", rules: []),
      .shutdown(requestID: requestID),
      .applyReview(route: route, requestID: requestID, noDelete: false),
      .dryRunReview(route: route, requestID: requestID),
      .quitReview(route: route, requestID: requestID),
      .promptDecision(route: route, requestID: requestID, promptID: 7, choice: 0),
      .formDecision(route: route, requestID: requestID, promptID: 7, value: "yes"),
      .cancelSync(route: route, requestID: requestID),
      .pauseSync(route: route, requestID: requestID),
    ]

    for command in commands {
      let object = try XCTUnwrap(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(command)) as? [String: Any])
      XCTAssertEqual(object["request_id"] as? String, requestID.uuidString.lowercased())
      for (key, value) in object where key.contains("mutation_id") {
        XCTAssertEqual(value as? String, mutationID.uuidString.lowercased(), key)
      }
    }
  }

  private func direction(_ stream: String, vector: Vector?) throws -> WireV3Direction {
    switch stream {
    case "desktop_to_daemon_commands": return .desktopToDaemonCommands
    case "daemon_to_desktop_events": return .daemonToDesktopEvents
    case "worker_to_daemon_events":
      return .workerToDaemonEvents(expected: try route(vector))
    case "daemon_to_worker_commands":
      let route = try route(vector)
      guard let promptID = vector?.promptID, let count = vector?.optionCount else {
        throw FixtureError.invalidManifest
      }
      return .daemonToWorkerCommands(
        expected: route, pending: .prompt(id: promptID, optionCount: count))
    default: throw FixtureError.invalidManifest
    }
  }

  private func event(_ data: Data) throws -> WireV3Event {
    guard
      case .event(let event) = try WireV3Codec.decode(
        data, direction: .daemonToDesktopEvents)
    else { throw FixtureError.invalidManifest }
    return event
  }

  private func route(_ vector: Vector?) throws -> WireV3Route {
    guard let raw = vector?.expectedDeviceID, let session = vector?.expectedSessionID else {
      throw FixtureError.invalidManifest
    }
    return WireV3Route(deviceID: try DeviceID(raw), sessionID: session)
  }

  private func loadManifest() throws -> Manifest {
    try JSONDecoder().decode(Manifest.self, from: fixture("manifest.json"))
  }

  private func fixture(_ path: String) throws -> Data {
    try Data(contentsOf: fixturesRoot.appendingPathComponent(path))
  }

  private func fixtureLines(_ path: String) throws -> [Data] {
    try String(decoding: fixture(path), as: UTF8.self).split(whereSeparator: \.isNewline)
      .map { Data($0.utf8) }
  }

  private var fixturesRoot: URL {
    URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .appendingPathComponent("../../../../crates/classick/tests/data/wire-v3")
      .standardizedFileURL
  }
}

private enum FixtureError: Error { case invalidManifest }

private struct Manifest: Decodable {
  let vectors: [Vector]
  let progress: CollectionGroup
  let device: CollectionGroup
  let operations: CollectionGroup
}

private struct CollectionGroup: Decodable {
  let positiveCollections: [Collection]
  let negativeVectors: [Vector]

  enum CodingKeys: String, CodingKey {
    case positiveCollections = "positive_collections"
    case negativeVectors = "negative_vectors"
  }
}

private struct Collection: Decodable {
  let path: String
  let stream: String
}

private struct Vector: Decodable {
  let path: String
  let expectation: String?
  let stream: String?
  let expectedRole: String?
  let expectedDeviceID: String?
  let expectedSessionID: UInt64?
  let promptID: UInt64?
  let optionCount: UInt64?

  enum CodingKeys: String, CodingKey {
    case path, expectation, stream
    case expectedRole = "expected_role"
    case expectedDeviceID = "expected_device_id"
    case expectedSessionID = "expected_session_id"
    case promptID = "prompt_id"
    case optionCount = "option_count"
  }
}
