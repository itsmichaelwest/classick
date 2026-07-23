import Darwin
import Foundation
import XCTest

@testable import Classick

private actor EventCollector {
  private(set) var events: [WireV3Event] = []
  func append(_ event: WireV3Event) { events.append(event) }
  var count: Int { events.count }
}

private final class CommandSink: @unchecked Sendable {
  private let lock = NSLock()
  private var buffer = ""

  func append(_ value: String) {
    lock.withLock { buffer += value }
  }

  var text: String { lock.withLock { buffer } }
}

private func makeUnixListener(path: String) -> Int32 {
  unlink(path)
  let listener = socket(AF_UNIX, SOCK_STREAM, 0)
  precondition(listener >= 0)
  var address = sockaddr_un()
  address.sun_family = sa_family_t(AF_UNIX)
  let pathBytes = Array(path.utf8)
  precondition(pathBytes.count < MemoryLayout.size(ofValue: address.sun_path))
  withUnsafeMutableBytes(of: &address.sun_path) { raw in
    let bytes = raw.bindMemory(to: UInt8.self)
    for (index, byte) in pathBytes.enumerated() { bytes[index] = byte }
    bytes[pathBytes.count] = 0
  }
  let result = withUnsafePointer(to: &address) { pointer in
    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
      bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
    }
  }
  precondition(result == 0)
  precondition(listen(listener, 4) == 0)
  return listener
}

private func writeLine(_ line: String, to descriptor: Int32) {
  _ = line.withCString { Darwin.write(descriptor, $0, strlen($0)) }
}

private let daemonHello =
  #"{"type":"hello","protocol_version":"3.0.0","role":"daemon","software_version":"0.0.1","capabilities":["device_inventory","portable_profile","typed_sync_progress"]}"#

private func drainCommands(
  _ descriptor: Int32, into sink: CommandSink, until marker: String
) {
  var timeout = timeval(tv_sec: 0, tv_usec: 500_000)
  setsockopt(
    descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout,
    socklen_t(MemoryLayout<timeval>.size))
  var buffer = [UInt8](repeating: 0, count: 4096)
  let deadline = Date().addingTimeInterval(2)
  while Date() < deadline {
    let count = Darwin.read(descriptor, &buffer, buffer.count)
    if count <= 0 { break }
    sink.append(String(decoding: buffer[0..<count], as: UTF8.self))
    if sink.text.contains(marker) { break }
  }
}

final class DaemonClientTests: XCTestCase {
  private let deviceID = DeviceID("000A27002138B0A8")

  func testSingleStreamYieldsHelloAndFlatTypedProgress() async throws {
    let (path, listener) = socketFixture()
    defer { unlink(path); close(listener) }
    Thread {
      let client = accept(listener, nil, nil)
      guard client >= 0 else { return }
      defer { close(client) }
      writeLine(daemonHello + "\n", to: client)
      writeLine(
        #"{"type":"track_done","device_id":"000A27002138B0A8","session_id":42,"result":"applied"}"# + "\n",
        to: client)
      Thread.sleep(forTimeInterval: 0.2)
    }.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task { for await event in stream { await collector.append(event) } }
    await client.start()
    await waitForEvents(collector, count: 2)
    await client.stop()
    consumer.cancel()

    let events = await collector.events
    guard events.count == 2, case .hello = events[0], case .progress(let progress) = events[1]
    else { return XCTFail("unexpected event sequence: \(events)") }
    XCTAssertEqual(progress.kind, .trackDone)
    XCTAssertEqual(progress.route, WireV3Route(deviceID: deviceID, sessionID: 42))
  }

  func testSharedDeviceGoldenFlowsThroughRealSocketInOrder() async throws {
    let lines = try fixtureLines("device/events.ndjson")
    let (path, listener) = socketFixture()
    defer { unlink(path); close(listener) }
    Thread {
      let client = accept(listener, nil, nil)
      guard client >= 0 else { return }
      defer { close(client) }
      writeLine(daemonHello + "\n", to: client)
      for line in lines.prefix(4) { writeLine(String(decoding: line, as: UTF8.self) + "\n", to: client) }
      Thread.sleep(forTimeInterval: 0.2)
    }.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task { for await event in stream { await collector.append(event) } }
    await client.start()
    await waitForEvents(collector, count: 5)
    await client.stop()
    consumer.cancel()

    let events = await collector.events
    XCTAssertEqual(events.count, 5)
    guard case .deviceInventory(let first) = events[1],
      case .deviceInventory(let second) = events[2],
      case .inventorySubscriptionChanged = events[3],
      case .deviceConfig = events[4]
    else { return XCTFail("shared golden order was not preserved: \(events)") }
    XCTAssertEqual(first.revision, 1)
    XCTAssertEqual(second.revision, 2)
  }

  func testHandshakeSendsV3StartupSubscriptionsAndQueries() async throws {
    let sink = CommandSink()
    let (path, listener) = socketFixture()
    defer { unlink(path); close(listener) }
    Thread {
      let client = accept(listener, nil, nil)
      guard client >= 0 else { return }
      defer { close(client) }
      writeLine(daemonHello + "\n", to: client)
      drainCommands(client, into: sink, until: "get_global_config")
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await waitUntil { sink.text.contains("get_global_config") }
    await client.stop()
    consumer.cancel()

    let objects = try commandObjects(sink.text)
    XCTAssertEqual(Set(objects.compactMap { $0["type"] as? String }), [
      "subscribe_inventory", "get_inventory", "get_global_config",
    ])
    for object in objects {
      let requestID = try XCTUnwrap(object["request_id"] as? String)
      XCTAssertEqual(requestID, requestID.lowercased())
      XCTAssertNotNil(UUID(uuidString: requestID))
    }
  }

  func testBatchSendWritesSelectionBeforePreview() async throws {
    let sink = CommandSink()
    let (path, listener) = socketFixture()
    defer { unlink(path); close(listener) }
    Thread {
      let client = accept(listener, nil, nil)
      guard client >= 0 else { return }
      defer { close(client) }
      writeLine(daemonHello + "\n", to: client)
      drainCommands(client, into: sink, until: "preview_device")
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await waitUntil { sink.text.contains("get_global_config") }
    await client.send([
      .setSelection(
        deviceID: deviceID, requestID: UUID(), mutationID: UUID(),
        selection: WireV3SelectionValue(.init(mode: .all, rules: []))),
      .previewDevice(deviceID: deviceID, requestID: UUID()),
    ])
    await waitUntil { sink.text.contains("preview_device") }
    await client.stop()
    consumer.cancel()

    let types = try commandObjects(sink.text).compactMap { $0["type"] as? String }
    let selection = try XCTUnwrap(types.firstIndex(of: "set_selection"))
    XCTAssertEqual(types.index(after: selection), types.firstIndex(of: "preview_device"))
  }

  func testInvalidHelloRejectsAllFollowingEvents() async throws {
    let (path, listener) = socketFixture()
    defer { unlink(path); close(listener) }
    Thread {
      let client = accept(listener, nil, nil)
      guard client >= 0 else { return }
      defer { close(client) }
      writeLine(#"{"type":"hello","protocol_version":"2.0.0","role":"daemon","software_version":"0.0.1","capabilities":[]}"# + "\n", to: client)
      writeLine(#"{"type":"track_done","device_id":"000A27002138B0A8","session_id":42,"result":"applied"}"# + "\n", to: client)
    }.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task { for await event in stream { await collector.append(event) } }
    await client.start()
    await waitUntil { await client.lastFatalError != nil }
    await client.stop()
    consumer.cancel()
    let events = await collector.events
    XCTAssertTrue(events.isEmpty)
  }

  func testDurableAdditionsCoalesceOnlyQueuedSuccessor() throws {
    var outbox = DurableIntentOutbox()
    let first = UUID(), second = UUID(), third = UUID()
    try outbox.upsert(.addSelectionToDevice(
      deviceID: deviceID, requestID: first, mutationID: UUID(), rules: [.artist(name: "Birdy")]))
    outbox.markWritten(requestID: first, connectionGeneration: 1)
    try outbox.upsert(.addSelectionToDevice(
      deviceID: deviceID, requestID: second, mutationID: UUID(), rules: [.genre(name: "Pop")]))
    try outbox.upsert(.addSelectionToDevice(
      deviceID: deviceID, requestID: third, mutationID: UUID(),
      rules: [.album(artist: "Birdy", album: "Fire Within")]))
    XCTAssertEqual(outbox.requestIDs, [first, third].map { $0.uuidString.lowercased() })
    XCTAssertNil(outbox.nextIntent(for: 1))
    XCTAssertTrue(outbox.acknowledge(.init(
      requestID: first, revision: 2, target: .deviceSelectionAddition(deviceID),
      terminalFailure: false)))
    XCTAssertEqual(outbox.nextIntent(for: 1)?.requestID, third)
  }

  func testWrittenDurableIntentReplaysByteIdenticallyAfterReconnect() throws {
    var outbox = DurableIntentOutbox()
    let requestID = UUID()
    try outbox.upsert(.deletePlaylist(requestID: requestID, slug: "mix"))
    let first = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: requestID, connectionGeneration: 1)
    XCTAssertNil(outbox.nextIntent(for: 1))
    XCTAssertEqual(outbox.nextIntent(for: 2)?.bytes, first.bytes)
  }

  func testDisconnectedSendQueuesOnlyDurableMutations() async throws {
    let client = DaemonClient(socketPath: "/tmp/classick-missing-\(UUID()).sock")
    let durable = await client.send(.setSelection(
      deviceID: deviceID, requestID: UUID(), mutationID: UUID(),
      selection: WireV3SelectionValue(.init(mode: .all, rules: []))))
    let read = await client.send(.getInventory(requestID: UUID()))
    XCTAssertEqual(durable, .queued)
    XCTAssertEqual(read, .dropped)
  }

  func testCompatibilityAndGenerationGates() {
    XCTAssertTrue(DaemonClient.supportsDaemonProtocol("3.0.0"))
    XCTAssertTrue(DaemonClient.supportsDaemonProtocol("3.9.7"))
    XCTAssertFalse(DaemonClient.supportsDaemonProtocol("2.0.0"))
    XCTAssertFalse(DaemonClient.isCurrentLine(
      runGeneration: 4, currentRunGeneration: 4,
      connectionGeneration: 8, currentConnectionGeneration: 9))
  }

  private func socketFixture() -> (String, Int32) {
    let path = NSTemporaryDirectory() + "cdv3_\(UUID().uuidString.prefix(8)).sock"
    return (path, makeUnixListener(path: path))
  }

  private func waitForEvents(_ collector: EventCollector, count: Int) async {
    await waitUntil { await collector.count >= count }
  }

  private func waitUntil(_ predicate: @escaping @Sendable () async -> Bool) async {
    let deadline = Date().addingTimeInterval(5)
    while !(await predicate()) && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(20))
    }
  }

  private func commandObjects(_ text: String) throws -> [[String: Any]] {
    try text.split(separator: "\n").map {
      try XCTUnwrap(JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any])
    }
  }

  private func fixtureLines(_ path: String) throws -> [Data] {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .appendingPathComponent("../../../../crates/classick/tests/data/wire-v3")
      .standardizedFileURL
    return try String(decoding: Data(contentsOf: root.appendingPathComponent(path)), as: UTF8.self)
      .split(whereSeparator: \.isNewline).map { Data($0.utf8) }
  }
}
