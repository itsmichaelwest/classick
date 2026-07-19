import AppKit
import Darwin
import XCTest

@testable import Classick

private final class LockedCounter: @unchecked Sendable {
  private let lock = NSLock()
  private var value = 0

  func increment() {
    lock.lock()
    value += 1
    lock.unlock()
  }

  var count: Int {
    lock.lock()
    defer { lock.unlock() }
    return value
  }
}

private final class ShutdownCommandSink: @unchecked Sendable {
  private let lock = NSLock()
  private var buffer = ""

  func append(_ text: String) {
    lock.lock()
    buffer += text
    lock.unlock()
  }

  var text: String {
    lock.lock()
    defer { lock.unlock() }
    return buffer
  }
}

private func makeShutdownListener(path: String) -> Int32 {
  unlink(path)
  let listenFd = socket(AF_UNIX, SOCK_STREAM, 0)
  precondition(listenFd >= 0, "socket() failed")

  var address = sockaddr_un()
  address.sun_family = sa_family_t(AF_UNIX)
  let pathBytes = Array(path.utf8)
  precondition(pathBytes.count < MemoryLayout.size(ofValue: address.sun_path))
  withUnsafeMutableBytes(of: &address.sun_path) { raw in
    let buffer = raw.bindMemory(to: UInt8.self)
    for (index, byte) in pathBytes.enumerated() { buffer[index] = byte }
    buffer[pathBytes.count] = 0
  }
  let length = socklen_t(MemoryLayout<sockaddr_un>.size)
  let bindResult = withUnsafePointer(to: &address) { pointer in
    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketPointer in
      bind(listenFd, socketPointer, length)
    }
  }
  precondition(bindResult == 0, "bind() failed: \(String(cString: strerror(errno)))")
  precondition(listen(listenFd, 4) == 0, "listen() failed")
  return listenFd
}

private func writeShutdownLine(_ line: String, to fd: Int32) {
  _ = line.withCString { pointer in
    Darwin.write(fd, pointer, strlen(pointer))
  }
}

private func readCommandsUntilShutdown(from fd: Int32, into sink: ShutdownCommandSink) {
  var timeout = timeval(tv_sec: 2, tv_usec: 0)
  setsockopt(
    fd, SOL_SOCKET, SO_RCVTIMEO, &timeout,
    socklen_t(MemoryLayout<timeval>.size))
  var buffer = [UInt8](repeating: 0, count: 4096)
  while !sink.text.contains(#"{"type":"shutdown"}"#) {
    let count = Darwin.read(fd, &buffer, buffer.count)
    guard count > 0 else { return }
    sink.append(String(decoding: buffer[0..<count], as: UTF8.self))
  }
}

final class DaemonClientShutdownTests: XCTestCase {
  func testAttachedShutdownRequestedBeforeHelloSendsOnceAndWaitsForEOF() async {
    let path = NSTemporaryDirectory() + "cdsh_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeShutdownListener(path: path)
    defer { close(listenFd) }
    let sink = ShutdownCommandSink()
    let accepted = expectation(description: "socket accepted before hello")
    let releaseHello = DispatchSemaphore(value: 0)

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      accepted.fulfill()
      releaseHello.wait()
      writeShutdownLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      readCommandsUntilShutdown(from: clientFd, into: sink)
      close(clientFd)
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await fulfillment(of: [accepted], timeout: 1)
    try? await Task.sleep(for: .milliseconds(20))

    let shutdown = Task { await client.shutdownAndWait(timeout: .seconds(1)) }
    try? await Task.sleep(for: .milliseconds(20))
    releaseHello.signal()
    let cleanExit = await shutdown.value
    consumer.cancel()

    XCTAssertTrue(cleanExit)
    XCTAssertEqual(
      sink.text.split(separator: "\n").filter { $0 == #"{"type":"shutdown"}"# }.count,
      1)
  }

  func testShutdownAndWaitSendsShutdownToAttachedDaemonAndSucceedsAtEOF() async {
    let path = NSTemporaryDirectory() + "cdsd_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeShutdownListener(path: path)
    defer { close(listenFd) }
    let sink = ShutdownCommandSink()

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      writeShutdownLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      readCommandsUntilShutdown(from: clientFd, into: sink)
      close(clientFd)
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await waitForText("get_config", in: sink)

    let cleanExit = await client.shutdownAndWait(timeout: .seconds(1))
    consumer.cancel()

    XCTAssertTrue(cleanExit)
    XCTAssertEqual(
      sink.text.split(separator: "\n").filter { $0 == #"{"type":"shutdown"}"# }.count,
      1)
  }

  func testShutdownAndWaitFailsAfterBoundedInactivity() async {
    let path = NSTemporaryDirectory() + "cdst_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeShutdownListener(path: path)
    defer { close(listenFd) }
    let sink = ShutdownCommandSink()

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      defer { close(clientFd) }
      writeShutdownLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      readCommandsUntilShutdown(from: clientFd, into: sink)
      Thread.sleep(forTimeInterval: 0.4)
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await waitForText("get_config", in: sink)
    let clock = ContinuousClock()
    let started = clock.now

    let cleanExit = await client.shutdownAndWait(timeout: .milliseconds(60))
    let elapsed = started.duration(to: clock.now)
    consumer.cancel()

    XCTAssertFalse(cleanExit)
    XCTAssertGreaterThanOrEqual(elapsed, .milliseconds(50))
    XCTAssertLessThan(elapsed, .seconds(1))
  }

  func testDisconnectedShutdownWaitsForBoundedInactivityBeforeFailure() async {
    let client = DaemonClient(socketPath: "/tmp/classick-missing-\(UUID().uuidString).sock")
    let clock = ContinuousClock()
    let started = clock.now

    let cleanExit = await client.shutdownAndWait(timeout: .milliseconds(50))
    let elapsed = started.duration(to: clock.now)

    XCTAssertFalse(cleanExit)
    XCTAssertGreaterThanOrEqual(elapsed, .milliseconds(40))
    XCTAssertLessThan(elapsed, .seconds(1))
  }

  func testShutdownProgressExtendsTheInactivityWaitUntilEOF() async {
    let path = NSTemporaryDirectory() + "cdsp_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeShutdownListener(path: path)
    defer { close(listenFd) }
    let sink = ShutdownCommandSink()

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      writeShutdownLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      readCommandsUntilShutdown(from: clientFd, into: sink)
      for index in 0..<4 {
        Thread.sleep(forTimeInterval: 0.04)
        let line =
          #"{"type":"sync_event","line":"{\"type\":\"log\",\"message\":\"finalizing \#(index)\"}","serial":"RAW-A","session_id":7}"#
        writeShutdownLine(line + "\n", to: clientFd)
      }
      close(clientFd)
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await waitForText("get_config", in: sink)
    let clock = ContinuousClock()
    let started = clock.now

    let cleanExit = await client.shutdownAndWait(timeout: .milliseconds(70))
    let elapsed = started.duration(to: clock.now)
    consumer.cancel()

    XCTAssertTrue(cleanExit)
    XCTAssertGreaterThan(elapsed, .milliseconds(140))
  }

  private func waitForText(
    _ text: String, in sink: ShutdownCommandSink, timeout: TimeInterval = 5
  ) async {
    let deadline = Date().addingTimeInterval(timeout)
    while !sink.text.contains(text) && Date() < deadline {
      try? await Task.sleep(for: .milliseconds(10))
    }
  }
}

final class DaemonShutdownCoordinatorTests: XCTestCase {
  @MainActor
  func testOwnedShutdownRequestedBeforeHelloAvoidsFallbackAfterEOF() async {
    let path = NSTemporaryDirectory() + "cdso_\(UUID().uuidString.prefix(8)).sock"
    defer { unlink(path) }
    let listenFd = makeShutdownListener(path: path)
    defer { close(listenFd) }
    let sink = ShutdownCommandSink()
    let accepted = expectation(description: "socket accepted before hello")
    let releaseHello = DispatchSemaphore(value: 0)

    Thread {
      let clientFd = accept(listenFd, nil, nil)
      guard clientFd >= 0 else { return }
      accepted.fulfill()
      releaseHello.wait()
      writeShutdownLine(
        #"{"type":"hello","protocol_version":"2.0.0","core_version":"2.0.0"}"# + "\n",
        to: clientFd)
      readCommandsUntilShutdown(from: clientFd, into: sink)
      close(clientFd)
    }.start()

    let client = DaemonClient(socketPath: path)
    let stream = await client.events()
    let consumer = Task { for await _ in stream {} }
    await client.start()
    await fulfillment(of: [accepted], timeout: 1)
    try? await Task.sleep(for: .milliseconds(20))

    let coordinator = DaemonShutdownCoordinator()
    let fallbacks = LockedCounter()
    let replied = expectation(description: "termination reply")
    XCTAssertEqual(
      coordinator.begin(
        shutdown: { await client.shutdownAndWait(timeout: .seconds(1)) },
        forceTerminateOwnedDaemon: { fallbacks.increment() },
        reply: { _ in replied.fulfill() }),
      .terminateLater)
    try? await Task.sleep(for: .milliseconds(20))
    releaseHello.signal()

    await fulfillment(of: [replied], timeout: 1)
    consumer.cancel()
    XCTAssertEqual(fallbacks.count, 0)
    XCTAssertEqual(
      sink.text.split(separator: "\n").filter { $0 == #"{"type":"shutdown"}"# }.count,
      1)
  }

  @MainActor
  func testSuccessfulShutdownRepliesOnceWithoutFallback() async {
    let coordinator = DaemonShutdownCoordinator()
    let replies = LockedCounter()
    let fallbacks = LockedCounter()
    let replied = expectation(description: "termination reply")
    replied.assertForOverFulfill = true

    let result = coordinator.begin(
      shutdown: { true },
      forceTerminateOwnedDaemon: { fallbacks.increment() },
      reply: { shouldTerminate in
        XCTAssertTrue(shouldTerminate)
        replies.increment()
        replied.fulfill()
      })

    XCTAssertEqual(result, .terminateLater)
    await fulfillment(of: [replied], timeout: 1)
    try? await Task.sleep(for: .milliseconds(20))
    XCTAssertEqual(replies.count, 1)
    XCTAssertEqual(fallbacks.count, 0)
  }

  @MainActor
  func testFailedShutdownFallsBackAndRepliesExactlyOnce() async {
    let coordinator = DaemonShutdownCoordinator()
    let replies = LockedCounter()
    let fallbacks = LockedCounter()
    let replied = expectation(description: "termination reply")
    replied.assertForOverFulfill = true

    XCTAssertEqual(
      coordinator.begin(
        shutdown: { false },
        forceTerminateOwnedDaemon: { fallbacks.increment() },
        reply: { _ in
          replies.increment()
          replied.fulfill()
        }),
      .terminateLater)

    await fulfillment(of: [replied], timeout: 1)
    try? await Task.sleep(for: .milliseconds(20))
    XCTAssertEqual(fallbacks.count, 1)
    XCTAssertEqual(replies.count, 1)
  }

  @MainActor
  func testRepeatedBeginStartsOneShutdownOneFallbackAndOneReply() async {
    let coordinator = DaemonShutdownCoordinator()
    let shutdowns = LockedCounter()
    let replies = LockedCounter()
    let fallbacks = LockedCounter()
    let replied = expectation(description: "termination reply")
    replied.assertForOverFulfill = true
    let shutdown: @Sendable () async -> Bool = {
      shutdowns.increment()
      try? await Task.sleep(for: .milliseconds(30))
      return false
    }
    let fallback: @MainActor () -> Void = { fallbacks.increment() }
    let reply: @MainActor (Bool) -> Void = { _ in
      replies.increment()
      replied.fulfill()
    }

    XCTAssertEqual(
      coordinator.begin(
        shutdown: shutdown, forceTerminateOwnedDaemon: fallback, reply: reply),
      .terminateLater)
    XCTAssertEqual(
      coordinator.begin(
        shutdown: shutdown, forceTerminateOwnedDaemon: fallback, reply: reply),
      .terminateLater)

    await fulfillment(of: [replied], timeout: 1)
    try? await Task.sleep(for: .milliseconds(20))
    XCTAssertEqual(shutdowns.count, 1)
    XCTAssertEqual(fallbacks.count, 1)
    XCTAssertEqual(replies.count, 1)
  }

  @MainActor
  func testDaemonSpawnArgumentsCarryOwningAppPID() {
    XCTAssertEqual(
      DaemonProcess.arguments(parentPID: 1_234),
      ["--daemon", "--daemon-parent-pid", "1234"])
  }

  @MainActor
  func testFastRelaunchWaitsForPriorAppAndSocketToDisappear() async {
    let priorChecks = LockedCounter()
    let socketChecks = LockedCounter()

    await DaemonProcess.waitForPriorApplicationAndSocket(
      priorApplicationIsRunning: {
        priorChecks.increment()
        return priorChecks.count < 3
      },
      socketExists: {
        socketChecks.increment()
        return socketChecks.count < 4
      },
      pollInterval: .milliseconds(1))

    XCTAssertGreaterThanOrEqual(priorChecks.count, 3)
    XCTAssertGreaterThanOrEqual(socketChecks.count, 4)
  }
}
