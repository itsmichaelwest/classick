import Darwin
import XCTest

@testable import Classick

/// Collects events off the actor's `AsyncStream` so the test's polling loop
/// can inspect progress without racing the stream's own isolation.
private actor EventCollector {
  private(set) var events: [DaemonEvent] = []

  func append(_ event: DaemonEvent) {
    events.append(event)
  }

  var count: Int { events.count }
}

/// Minimal AF_UNIX test server: bind + listen on a scratch path, hand back
/// the listener fd so the caller can `accept()` on a background thread.
private func makeUnixListener(path: String) -> Int32 {
  unlink(path)
  let listenFd = socket(AF_UNIX, SOCK_STREAM, 0)
  precondition(listenFd >= 0, "socket() failed")

  var addr = sockaddr_un()
  addr.sun_family = sa_family_t(AF_UNIX)
  let pathBytes = Array(path.utf8)
  precondition(pathBytes.count < MemoryLayout.size(ofValue: addr.sun_path))
  withUnsafeMutableBytes(of: &addr.sun_path) { raw in
    let buf = raw.bindMemory(to: UInt8.self)
    for (i, byte) in pathBytes.enumerated() { buf[i] = byte }
    buf[pathBytes.count] = 0
  }
  let len = socklen_t(MemoryLayout<sockaddr_un>.size)
  let bindResult = withUnsafePointer(to: &addr) { ptr in
    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
      bind(listenFd, sockPtr, len)
    }
  }
  precondition(bindResult == 0, "bind() failed: \(String(cString: strerror(errno)))")
  precondition(listen(listenFd, 4) == 0, "listen() failed")
  return listenFd
}

private func writeLine(_ line: String, to fd: Int32) {
  _ = line.withCString { cstr in
    Darwin.write(fd, cstr, strlen(cstr))
  }
}

/// Thread-safe accumulator for bytes the client writes back, so the server
/// thread can record them and the test's async body can inspect them.
private final class CommandSink: @unchecked Sendable {
  private let lock = NSLock()
  private var buffer = ""
  func append(_ s: String) {
    lock.lock()
    buffer += s
    lock.unlock()
  }
  var text: String {
    lock.lock()
    defer { lock.unlock() }
    return buffer
  }
}

final class DaemonClientTests: XCTestCase {
  func testAdditiveIntentKeepsWrittenPredecessorAndMergesQueuedSuccessor() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.addSelectionToDevice(
      requestID: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
      serial: "A", rules: [.artist(name: "Birdy")]))
    outbox.markWritten(requestID: "00000000-0000-0000-0000-000000000001", connectionGeneration: 1)
    try outbox.upsert(.addSelectionToDevice(
      requestID: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!,
      serial: "A", rules: [.genre(name: "Pop")]))
    try outbox.upsert(.addSelectionToDevice(
      requestID: UUID(uuidString: "00000000-0000-0000-0000-000000000003")!,
      serial: "A", rules: [.album(artist: "Birdy", album: "Fire Within")]))
    XCTAssertEqual(outbox.requestIDs, [
      "00000000-0000-0000-0000-000000000001",
      "00000000-0000-0000-0000-000000000003",
    ])
    XCTAssertEqual(outbox.nextIntent(for: 1)?.requestID, nil)

    XCTAssertTrue(outbox.acknowledge(.init(
      requestID: "00000000-0000-0000-0000-000000000001", revision: 2,
      configState: nil, target: .deviceSelectionAddition(serial: "A"))))
    XCTAssertEqual(
      outbox.nextIntent(for: 1)?.requestID,
      "00000000-0000-0000-0000-000000000003")
  }

  func testAdditiveCollisionRemovesOnlyExactTargetAndCommandFailureRetains() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.appendSelectionToPlaylist(
      requestID: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
      slug: "favorites", rules: [.genre(name: "Pop")]))
    outbox.markWritten(requestID: "00000000-0000-0000-0000-000000000001", connectionGeneration: 1)
    XCTAssertFalse(outbox.acknowledge(.init(
      requestID: "00000000-0000-0000-0000-000000000001", revision: nil,
      configState: nil, target: nil)))
    XCTAssertFalse(outbox.acknowledge(.init(
      requestID: "00000000-0000-0000-0000-000000000001", revision: nil,
      configState: nil, target: .deviceSelectionAddition(serial: "favorites"),
      terminalFailure: true)))
    XCTAssertTrue(outbox.acknowledge(.init(
      requestID: "00000000-0000-0000-0000-000000000001", revision: nil,
      configState: nil, target: .playlistAppend(slug: "favorites"), terminalFailure: true)))
    XCTAssertTrue(outbox.requestIDs.isEmpty)
  }
  func testHundredEventBurstPreservesExactWireOrder() async throws {
    let path = NSTemporaryDirectory() + "cdor_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      for index in 0..<100 {
        writeLine(
          #"{"type":"device_connected","serial":"S\#(index)","model_label":"iPod","drive":"/Volumes/IPOD"}"#
            + "\n", to: clientFd)
      }
      Thread.sleep(forTimeInterval: 0.2)
    }.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task { for await event in stream { await collector.append(event) } }
    await client.start()
    await waitForEvents(collector, minCount: 101)
    await client.stop()
    consumer.cancel()

    let serials = await collector.events.compactMap { event -> String? in
      guard case .deviceConnected(let serial, _, _, _) = event else { return nil }
      return serial
    }
    XCTAssertEqual(serials, (0..<100).map { "S\($0)" })
  }

  func testSameKeyQueuedIntentsCoalesceToNewest() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.saveConfig(source: "/old", daemon: nil, ipod: nil, requestID: "old"))
    try outbox.upsert(.saveConfig(source: "/new", daemon: nil, ipod: nil, requestID: "new"))
    XCTAssertEqual(outbox.requestIDs, ["new"])
  }

  func testCoalescingMovesNewestIntentBehindOtherKeys() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.saveConfig(source: "/old", daemon: nil, ipod: nil, requestID: "old"))
    try outbox.upsert(.deletePlaylist(slug: "mix", requestID: "playlist"))
    try outbox.upsert(.saveConfig(source: "/new", daemon: nil, ipod: nil, requestID: "new"))
    XCTAssertEqual(outbox.requestIDs, ["playlist", "new"])
  }

  func testFailedFlushLeavesIntentAtFront() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.deletePlaylist(slug: "mix", requestID: "save"))
    let attempted = try XCTUnwrap(outbox.nextIntent(for: 1))
    XCTAssertEqual(outbox.nextIntent(for: 1)?.requestID, attempted.requestID)
    XCTAssertEqual(outbox.nextIntent(for: 1)?.bytes, attempted.bytes)
  }

  func testWrittenIntentIsResentVerbatimAfterReconnect() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.deletePlaylist(slug: "mix", requestID: "save"))
    let firstWrite = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: firstWrite.requestID, connectionGeneration: 1)
    XCTAssertNil(outbox.nextIntent(for: 1))
    XCTAssertEqual(outbox.nextIntent(for: 2)?.bytes, firstWrite.bytes)
  }

  func testNilSlugPlaylistLostAcknowledgementReplaysOneStableOrderedCreate() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(
      .savePlaylist(
        .manual(
          slug: nil, name: "Gym Mix",
          tracks: ["B/02.flac", "A/01.flac", "B/03.flac"]),
        requestID: "request-a"))
    let firstWrite = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: firstWrite.requestID, connectionGeneration: 1)
    let replay = try XCTUnwrap(outbox.nextIntent(for: 2))

    XCTAssertEqual(replay.bytes, firstWrite.bytes, "reconnect must replay byte-identical intent")
    let firstObject = try playlistObject(from: firstWrite.bytes)
    let replayObject = try playlistObject(from: replay.bytes)
    let firstSlug = try XCTUnwrap(firstObject["slug"] as? String)
    let replaySlug = try XCTUnwrap(replayObject["slug"] as? String)
    XCTAssertEqual(Set([firstSlug, replaySlug]).count, 1, "both writes must target one playlist")
    XCTAssertEqual(firstSlug, "gym-mix-request-a")
    XCTAssertEqual(
      replayObject["tracks"] as? [String],
      ["B/02.flac", "A/01.flac", "B/03.flac"],
      "manual playlist order must remain stable")
  }

  func testAcknowledgementRemovesOnlyWrittenIntentExactlyOnce() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(.deletePlaylist(slug: "mix", requestID: "first"))
    let first = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: first.requestID, connectionGeneration: 1)
    try outbox.upsert(.deletePlaylist(slug: "mix", requestID: "successor"))
    XCTAssertNil(outbox.nextIntent(for: 1), "same-key successor must wait for acknowledgement")
    let revisionlessAcknowledgement = DurableAcknowledgement(
      requestID: "first", revision: nil, configState: nil)
    XCTAssertFalse(outbox.acknowledge(revisionlessAcknowledgement))
    XCTAssertNil(
      outbox.nextIntent(for: 1),
      "correlation without persistence evidence must keep the predecessor in flight")

    let event = try JSONDecoder().decode(
      DaemonEvent.self,
      from: Data(
        #"{"type":"playlists_update","playlists":[],"playlist_revision":1,"acknowledged_request_id":"first"}"#
          .utf8))
    let acknowledgement = try XCTUnwrap(event.durableAcknowledgement)
    XCTAssertTrue(outbox.acknowledge(acknowledgement))
    XCTAssertFalse(outbox.acknowledge(acknowledgement))
    XCTAssertEqual(outbox.nextIntent(for: 1)?.requestID, "successor")
  }

  func testRejectedConfigSaveAcknowledgementRetainsIntentForReconnect() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(
      .saveConfig(source: "/wanted", daemon: nil, ipod: nil, requestID: "save"))
    let written = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: written.requestID, connectionGeneration: 1)
    let failedReply = try JSONDecoder().decode(
      DaemonEvent.self,
      from: Data(
        #"{"type":"config_update","source":"/original","daemon":null,"ipod":null,"config_revision":7,"acknowledged_request_id":"save"}"#
          .utf8))
    let acknowledgement = try XCTUnwrap(failedReply.durableAcknowledgement)

    XCTAssertFalse(outbox.acknowledge(acknowledgement))
    XCTAssertEqual(outbox.nextIntent(for: 2)?.bytes, written.bytes)
  }

  func testCommittedConfigSaveAcknowledgementRemovesIntent() throws {
    var outbox = DurableIntentOutbox()
    try outbox.upsert(
      .saveConfig(source: "/wanted", daemon: nil, ipod: nil, requestID: "save"))
    let written = try XCTUnwrap(outbox.nextIntent(for: 1))
    outbox.markWritten(requestID: written.requestID, connectionGeneration: 1)
    let committedReply = try JSONDecoder().decode(
      DaemonEvent.self,
      from: Data(
        #"{"type":"config_update","source":"/wanted","daemon":null,"ipod":null,"config_revision":8,"acknowledged_request_id":"save"}"#
          .utf8))
    let acknowledgement = try XCTUnwrap(committedReply.durableAcknowledgement)

    XCTAssertTrue(outbox.acknowledge(acknowledgement))
    XCTAssertNil(outbox.nextIntent(for: 2))
  }

  func testDisconnectedSendQueuesOnlyDurableMutations() async {
    let client = DaemonClient(socketPath: "/tmp/classick-missing-\(UUID().uuidString).sock")
    let durable = await client.send(
      .saveDeviceConfig(
        serial: "A", selection: .init(mode: .all, rules: []), subscriptions: nil,
        settings: nil, requestID: "save"))
    let read = await client.send(.getStatus(requestID: "read"))
    XCTAssertEqual(durable, .queued)
    XCTAssertEqual(read, .dropped)
  }

  func testDaemonProtocolCompatibilityAcceptsOnlyMajorTwo() {
    XCTAssertTrue(DaemonClient.supportsDaemonProtocol("2.0.0"))
    XCTAssertTrue(DaemonClient.supportsDaemonProtocol("2.9.7"))
    XCTAssertFalse(DaemonClient.supportsDaemonProtocol("1.7.0"))
    XCTAssertFalse(DaemonClient.supportsDaemonProtocol("3.0.0"))
    XCTAssertFalse(DaemonClient.supportsDaemonProtocol("invalid"))
  }

  func testInvalidHelloRejectsBeforeYieldingFollowingEvents() async throws {
    let path = NSTemporaryDirectory() + "cdbad_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }

    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    let serverThread = Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeLine(
        #"{"type":"hello","protocol_version":"1.7.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      writeLine(
        #"{"type":"device_connected","serial":"RAW-A","model_label":"iPod Classic","drive":"/Volumes/IPOD"}"#
          + "\n",
        to: clientFd)
      Thread.sleep(forTimeInterval: 0.2)
    }
    serverThread.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task {
      for await event in stream { await collector.append(event) }
    }

    await client.start()
    let deadline = Date().addingTimeInterval(5)
    while await client.lastFatalError == nil && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(20))
    }
    let fatalError = await client.lastFatalError
    let events = await collector.events
    await client.stop()
    consumer.cancel()

    XCTAssertTrue(events.isEmpty, "invalid hello must gate every following event: \(events)")
    XCTAssertTrue(fatalError?.contains("expected hello major version 2") == true)
  }

  func testDelayedLineFromPreviousConnectionIsRejectedAfterReconnect() {
    XCTAssertFalse(
      DaemonClient.isCurrentLine(
        runGeneration: 4, currentRunGeneration: 4,
        connectionGeneration: 8, currentConnectionGeneration: 9))
    XCTAssertTrue(
      DaemonClient.isCurrentLine(
        runGeneration: 4, currentRunGeneration: 4,
        connectionGeneration: 9, currentConnectionGeneration: 9))
    XCTAssertFalse(
      DaemonClient.isCurrentLine(
        runGeneration: 3, currentRunGeneration: 4,
        connectionGeneration: 9, currentConnectionGeneration: 9))
  }

  /// Polls `collector` until it has at least `minCount` events or the
  /// deadline passes, so the test never hangs forever if the client stalls.
  private func waitForEvents(_ collector: EventCollector, minCount: Int, timeout: TimeInterval = 5)
    async
  {
    let deadline = Date().addingTimeInterval(timeout)
    while await collector.count < minCount && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(20))
    }
  }

  private func playlistObject(from line: Data) throws -> [String: Any] {
    let object = try JSONSerialization.jsonObject(with: line) as! [String: Any]
    return object["playlist"] as! [String: Any]
  }

  func testHandshakeThenDeviceConnectedEvent() async throws {
    // AF_UNIX paths are capped at ~104 bytes on Darwin — keep the scratch
    // suffix short since NSTemporaryDirectory() already eats a good chunk.
    let path = NSTemporaryDirectory() + "cdct_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }

    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    let serverThread = Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeLine(
        #"{"type":"hello","protocol_version":"2.3.4","core_version":"2.3.4"}"# + "\n", to: clientFd)
      writeLine(
        #"{"type":"device_connected","serial":"0x000A27002138B0A8","model_label":"iPod Classic (3rd gen)","drive":"/Volumes/IPOD","name":"Test iPod"}"#
          + "\n",
        to: clientFd)
      // Hold the connection open briefly so the client has time to read
      // before we tear it down.
      Thread.sleep(forTimeInterval: 0.3)
    }
    serverThread.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task {
      for await event in stream {
        await collector.append(event)
      }
    }

    await client.start()
    await waitForEvents(collector, minCount: 2)
    await client.stop()
    consumer.cancel()

    let events = await collector.events
    guard events.count >= 2 else {
      return XCTFail("expected hello + device_connected, got \(events)")
    }

    guard case .hello(let protocolVersion, let coreVersion) = events[0] else {
      return XCTFail("expected .hello first, got \(events[0])")
    }
    XCTAssertEqual(protocolVersion, "2.3.4")
    XCTAssertEqual(coreVersion, "2.3.4")

    guard case .deviceConnected(let serial, let modelLabel, let drive, let name) = events[1] else {
      return XCTFail("expected .deviceConnected second, got \(events[1])")
    }
    XCTAssertEqual(serial, "0x000A27002138B0A8")
    XCTAssertEqual(modelLabel, "iPod Classic (3rd gen)")
    XCTAssertEqual(drive, "/Volumes/IPOD")
    XCTAssertEqual(name, "Test iPod")
  }

  /// Regression for the "stuck on Set Up / Settings shows defaults" bug: the
  /// daemon only *pushes* config_update on a name change or after a save, so
  /// the client MUST pull config itself on every handshake. Asserts the
  /// client emits `get_config` after connecting.
  func testHandshakeSendsGetConfig() async throws {
    let path = NSTemporaryDirectory() + "cdgc_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }

    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    let sink = CommandSink()
    let serverThread = Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n", to: clientFd)

      // Bounded recv so the drain loop can't hang if the client stalls.
      var tv = timeval(tv_sec: 0, tv_usec: 500_000)
      setsockopt(clientFd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
      var buf = [UInt8](repeating: 0, count: 4096)
      let deadline = Date().addingTimeInterval(2)
      while Date() < deadline {
        let n = Darwin.read(clientFd, &buf, buf.count)
        if n <= 0 { break }
        sink.append(String(decoding: buf[0..<n], as: UTF8.self))
        if sink.text.contains("get_config") { break }
      }
      Thread.sleep(forTimeInterval: 0.1)
    }
    serverThread.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()

    let deadline = Date().addingTimeInterval(5)
    while !sink.text.contains("get_config") && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(20))
    }
    await client.stop()
    consumer.cancel()

    XCTAssertTrue(
      sink.text.contains("get_config"),
      "expected the client to send get_config on handshake, got: \(sink.text)")
  }

  func testBatchSendWritesSaveBeforePreview() async throws {
    let path = NSTemporaryDirectory() + "cdbt_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }

    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    let sink = CommandSink()
    let serverThread = Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n", to: clientFd)

      var tv = timeval(tv_sec: 0, tv_usec: 500_000)
      setsockopt(clientFd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
      var buf = [UInt8](repeating: 0, count: 4096)
      let deadline = Date().addingTimeInterval(2)
      while Date() < deadline {
        let n = Darwin.read(clientFd, &buf, buf.count)
        if n <= 0 { break }
        sink.append(String(decoding: buf[0..<n], as: UTF8.self))
        if sink.text.contains("preview_device") { break }
      }
    }
    serverThread.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    let handshakeDeadline = Date().addingTimeInterval(5)
    while !sink.text.contains("get_config") && Date() < handshakeDeadline {
      try? await Task.sleep(for: .milliseconds(20))
    }

    await client.send([
      .saveDeviceConfig(
        serial: "0xA", selection: .init(mode: .all, rules: []), subscriptions: nil,
        settings: nil, requestID: "save"),
      .previewDevice(serial: "0xA", requestID: "preview"),
    ])

    let deadline = Date().addingTimeInterval(5)
    while !sink.text.contains("preview_device") && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(20))
    }
    await client.stop()
    consumer.cancel()

    let types = sink.text.split(separator: "\n").compactMap { line -> String? in
      guard
        let data = line.data(using: .utf8),
        let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
      else { return nil }
      return object["type"] as? String
    }
    guard let saveIndex = types.firstIndex(of: "save_device_config"),
      let previewIndex = types.firstIndex(of: "preview_device")
    else { return XCTFail("missing save/preview commands: \(types)") }
    XCTAssertEqual(previewIndex, types.index(after: saveIndex), "batch must emit save then preview")
  }

  /// Bonus: after the server drops the first connection, the client should
  /// reconnect (to the same still-listening socket) and re-run the
  /// handshake, yielding a second `.hello`.
  func testReconnectsAndRehandshakesAfterDisconnect() async throws {
    let path = NSTemporaryDirectory() + "cdct_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }

    let listenFd = makeUnixListener(path: path)
    defer { close(listenFd) }

    let serverThread = Thread {
      for _ in 0..<2 {
        let clientFd = accept(listenFd, nil, nil)
        guard clientFd >= 0 else { return }
        writeLine(
          #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
          to: clientFd)
        Thread.sleep(forTimeInterval: 0.1)
        close(clientFd)
      }
    }
    serverThread.start()

    let client = DaemonClient(socketPath: path)
    let collector = EventCollector()
    let stream = await client.events()
    let consumer = Task {
      for await event in stream {
        await collector.append(event)
      }
    }

    await client.start()
    await waitForEvents(collector, minCount: 2, timeout: 8)
    await client.stop()
    consumer.cancel()

    let events = await collector.events
    let helloCount = events.filter {
      if case .hello = $0 { return true }
      return false
    }.count
    XCTAssertGreaterThanOrEqual(
      helloCount, 2, "expected two hello events across reconnects, got \(events)")
  }
}
