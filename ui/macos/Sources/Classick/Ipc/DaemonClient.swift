import Darwin
import Foundation
import os

enum SendDisposition: Equatable, Sendable {
  case sent
  case queued
  case dropped
}

struct DurableIntent: Sendable {
  let key: DurableIntentKey
  let requestID: String
  let command: DaemonCommand
  let bytes: Data
  fileprivate var lastWrittenConnection: Int?
}

struct DurableIntentOutbox: Sendable {
  private var intents: [DurableIntent] = []

  var requestIDs: [String] { intents.map(\.requestID) }

  mutating func upsert(_ command: DaemonCommand) throws {
    let command = command.normalizedForDurableEncoding()
    guard let key = command.durableIntentKey, let requestID = command.requestID else { return }
    guard !intents.contains(where: { $0.requestID == requestID }) else { return }

    intents.removeAll { $0.key == key && $0.lastWrittenConnection == nil }
    var bytes = try JSONEncoder().encode(command)
    bytes.append(0x0A)
    intents.append(
      DurableIntent(
        key: key, requestID: requestID, command: command, bytes: bytes,
        lastWrittenConnection: nil))
  }

  func nextIntent(for connectionGeneration: Int) -> DurableIntent? {
    for (index, intent) in intents.enumerated() {
      if intent.lastWrittenConnection == connectionGeneration { continue }
      let hasInFlightPredecessor = intents[..<index].contains {
        $0.key == intent.key && $0.lastWrittenConnection != nil
      }
      guard !hasInFlightPredecessor else { return nil }
      return intent
    }
    return nil
  }

  mutating func markWritten(requestID: String, connectionGeneration: Int) {
    guard let index = intents.firstIndex(where: { $0.requestID == requestID }) else { return }
    intents[index].lastWrittenConnection = connectionGeneration
  }

  func wasWritten(requestID: String, connectionGeneration: Int) -> Bool {
    intents.first(where: { $0.requestID == requestID })?.lastWrittenConnection
      == connectionGeneration
  }

  @discardableResult
  mutating func acknowledge(_ acknowledgement: DurableAcknowledgement) -> Bool {
    guard
      let index = intents.firstIndex(where: {
        $0.requestID == acknowledgement.requestID && $0.lastWrittenConnection != nil
      })
    else { return false }
    guard intents[index].command.isCommitted(by: acknowledgement) else { return false }
    intents.remove(at: index)
    return true
  }
}

/// Owns the Unix-domain-socket connection to the classick daemon.
///
/// Blocking `read()` must never run inside actor-isolated code — it would
/// stall every other call on this actor (including `send`) for as long as
/// the daemon stays silent. One dedicated background `Thread` therefore
/// produces raw lines, and one actor-isolated loop decodes and yields them.
actor DaemonClient {
  private let logger = Logger(subsystem: "com.classick.app", category: "DaemonClient")
  private let socketPath: String

  private var continuation: AsyncStream<DaemonEvent>.Continuation?
  private var fd: Int32 = -1
  private var isRunning = false
  private var generation = 0
  private var connectionGeneration = 0
  private var handshakeComplete = false
  private var durableOutbox = DurableIntentOutbox()
  private var shutdownConnectionGeneration: Int?
  private var shutdownCommandSent = false
  private var shutdownInactivityTimeout: Duration?
  private var shutdownTimeoutTask: Task<Void, Never>?
  private var shutdownWaiters: [CheckedContinuation<Bool, Never>] = []
  private var shutdownResult: Bool?

  /// Set once if the daemon's handshake fails the protocol-version check.
  /// Non-nil means the client has permanently stopped (no more reconnects).
  private(set) var lastFatalError: String?

  init(socketPath: String = NSTemporaryDirectory() + "classick.sock") {
    self.socketPath = socketPath
  }

  /// The stream of daemon events. Intended to be called once; the
  /// continuation is retained for the lifetime of the client.
  func events() -> AsyncStream<DaemonEvent> {
    AsyncStream { continuation in
      self.continuation = continuation
    }
  }

  /// Connects, performs the handshake, and reconnects with backoff
  /// (re-subscribing each time) until `stop()` is called.
  func start() async {
    guard !isRunning else { return }
    isRunning = true
    generation += 1
    let myGeneration = generation
    Task { await self.runLoop(generation: myGeneration) }
  }

  /// Stops reconnecting and tears down any live connection.
  func stop() async {
    isRunning = false
    generation += 1
    connectionGeneration += 1
    if shutdownConnectionGeneration != nil {
      completeShutdown(result: false)
    }
    closeSocket()
    continuation?.finish()
  }

  /// Disables reconnect, writes one graceful shutdown request, and waits for
  /// EOF on that exact connection. Valid daemon events reset the inactivity
  /// deadline so a healthy finalization is never cut off while progressing.
  func shutdownAndWait(timeout: Duration) async -> Bool {
    if let shutdownResult { return shutdownResult }

    return await withCheckedContinuation { continuation in
      shutdownWaiters.append(continuation)
      guard shutdownConnectionGeneration == nil else { return }

      isRunning = false
      shutdownConnectionGeneration = connectionGeneration
      shutdownInactivityTimeout = timeout
      scheduleShutdownTimeout()
      sendPendingShutdownIfReady()
    }
  }

  @discardableResult
  func send(_ command: DaemonCommand) async -> SendDisposition {
    sendCommand(command)
  }

  private func sendCommand(_ command: DaemonCommand) -> SendDisposition {
    if command.durableIntentKey != nil {
      guard command.requestID != nil else { return .dropped }
      do {
        try durableOutbox.upsert(command)
      } catch {
        logger.error(
          "failed to encode durable command: \(error.localizedDescription, privacy: .public)")
        return .dropped
      }
      flushDurableIntents()
      guard let requestID = command.requestID else { return .dropped }
      return durableOutbox.wasWritten(
        requestID: requestID, connectionGeneration: connectionGeneration)
        ? .sent : .queued
    }

    guard fd >= 0, handshakeComplete else {
      logger.warning(
        "send(\(String(describing: command), privacy: .public)) dropped — not connected")
      return .dropped
    }
    guard let bytes = encodedLine(for: command), writeAll(bytes, to: fd) else {
      closeSocket()
      return .dropped
    }
    return .sent
  }

  @discardableResult
  func send(_ commands: [DaemonCommand]) async -> [SendDisposition] {
    commands.map(sendCommand)
  }

  private func flushDurableIntents() {
    guard fd >= 0, handshakeComplete else { return }
    while let intent = durableOutbox.nextIntent(for: connectionGeneration) {
      guard writeAll(intent.bytes, to: fd) else {
        closeSocket()
        return
      }
      durableOutbox.markWritten(
        requestID: intent.requestID, connectionGeneration: connectionGeneration)
    }
  }

  // MARK: - Connect / reconnect loop

  private func runLoop(generation: Int) async {
    var backoff: Duration = .milliseconds(250)
    let maxBackoff: Duration = .seconds(10)

    while isRunning && generation == self.generation {
      if let connectedFd = connectSocket(path: socketPath) {
        fd = connectedFd
        connectionGeneration += 1
        handshakeComplete = false
        let currentConnectionGeneration = connectionGeneration
        backoff = .milliseconds(250)
        await readUntilDisconnected(
          connectionFd: connectedFd,
          runGeneration: generation,
          connectionGeneration: currentConnectionGeneration)
        closeSocket()
      } else {
        logger.debug("connect to \(self.socketPath, privacy: .public) failed")
      }

      guard isRunning && generation == self.generation else { break }
      try? await Task.sleep(for: backoff)
      backoff = min(backoff * 2, maxBackoff)
    }
  }

  private func closeSocket() {
    handshakeComplete = false
    if fd >= 0 {
      // `shutdown(SHUT_RDWR)` first: the background reader thread is
      // blocked in a plain `read()` on this fd, and on Darwin a bare
      // `close()` doesn't reliably unblock a concurrent read on the
      // same descriptor from another thread. `shutdown` forces that
      // read to return (EOF/error) so the reader thread's loop exits
      // and finishes the raw-line stream instead of hanging forever.
      Darwin.shutdown(fd, SHUT_RDWR)
      close(fd)
      fd = -1
    }
  }

  /// Consumes the one raw-line stream sequentially on this actor. No line gets
  /// its own task, so decoding, acknowledgement, and yielding cannot overtake.
  private func readUntilDisconnected(
    connectionFd: Int32,
    runGeneration: Int,
    connectionGeneration: Int
  ) async {
    var isFirstLine = true
    for await line in Self.lineStream(from: connectionFd) {
      await handleLine(
        line, isFirstLine: isFirstLine,
        runGeneration: runGeneration,
        connectionGeneration: connectionGeneration)
      isFirstLine = false
    }
    if shutdownCommandSent, shutdownConnectionGeneration == connectionGeneration {
      completeShutdown(result: true)
    }
  }

  nonisolated private static func lineStream(from connectionFd: Int32) -> AsyncStream<Data> {
    AsyncStream { continuation in
      let thread = Thread {
        var buffer = Data()
        var readBuffer = [UInt8](repeating: 0, count: 4096)

        readLoop: while true {
          let count: Int = readBuffer.withUnsafeMutableBytes { pointer in
            while true {
              let result = Darwin.read(connectionFd, pointer.baseAddress, pointer.count)
              if result < 0, errno == EINTR { continue }
              return result
            }
          }
          guard count > 0 else { break readLoop }
          buffer.append(contentsOf: readBuffer[0..<count])

          while let newlineIndex = buffer.firstIndex(of: 0x0A) {
            let line = buffer.subdata(in: buffer.startIndex..<newlineIndex)
            buffer.removeSubrange(buffer.startIndex...newlineIndex)
            if !line.isEmpty { continuation.yield(line) }
          }
        }
        continuation.finish()
      }
      thread.name = "classick.DaemonClient.reader"
      thread.start()
    }
  }

  private func handleLine(
    _ data: Data,
    isFirstLine: Bool,
    runGeneration: Int,
    connectionGeneration: Int
  ) async {
    guard
      Self.isCurrentLine(
        runGeneration: runGeneration,
        currentRunGeneration: generation,
        connectionGeneration: connectionGeneration,
        currentConnectionGeneration: self.connectionGeneration)
    else { return }

    let event: DaemonEvent
    do {
      event = try JSONDecoder().decode(DaemonEvent.self, from: data)
    } catch {
      // Log the FULL DecodingError (names the missing key/type and
      // coding path — localizedDescription is just "data missing")
      // plus a truncated raw line, so a wire-shape mismatch names
      // itself instead of requiring a socket probe to diagnose. A
      // dropped line here is silent data loss on the UI (the
      // status_update storage-keys mismatch hid a connected iPod's
      // entire status stream) — make it loud.
      let raw = String(data: data.prefix(300), encoding: .utf8) ?? "<non-utf8>"
      logger.error(
        "failed to decode daemon line: \(String(describing: error), privacy: .public) line=\(raw, privacy: .public)"
      )
      return
    }

    if shutdownCommandSent, shutdownConnectionGeneration == connectionGeneration {
      scheduleShutdownTimeout()
    }

    if isFirstLine {
      guard case .hello(let protocolVersion, _) = event,
        Self.supportsDaemonProtocol(protocolVersion)
      else {
        let message = "daemon handshake failed: expected hello major version 2, got \(event)"
        logger.fault("\(message, privacy: .public)")
        lastFatalError = message
        isRunning = false
        self.connectionGeneration += 1
        closeSocket()
        continuation?.finish()
        return
      }
      handshakeComplete = true
      if sendPendingShutdownIfReady() {
        continuation?.yield(event)
        return
      }
      continuation?.yield(event)
      await send(.subscribeDeviceEvents)
      await send(.getStatus(requestID: DaemonCommand.newRequestID()))
      // Explicitly pull config on every (re)connect. The daemon only
      // *pushes* config_update on a name change or after a save, so
      // without this the app never learns the persisted iPod identity on
      // a cached-name plug-in — leaving `configuredSerial` nil, the menu
      // stuck on "Set Up", and Settings showing defaults.
      await send(.getConfig(requestID: DaemonCommand.newRequestID()))
      flushDurableIntents()
      return
    }

    if let acknowledgement = event.durableAcknowledgement,
      durableOutbox.acknowledge(acknowledgement)
    {
      flushDurableIntents()
    }
    continuation?.yield(event)
  }

  nonisolated static func isCurrentLine(
    runGeneration: Int,
    currentRunGeneration: Int,
    connectionGeneration: Int,
    currentConnectionGeneration: Int
  ) -> Bool {
    runGeneration == currentRunGeneration
      && connectionGeneration == currentConnectionGeneration
  }

  nonisolated static func supportsDaemonProtocol(_ version: String) -> Bool {
    version.split(separator: ".").first == "2"
  }

  @discardableResult
  private func sendPendingShutdownIfReady() -> Bool {
    guard !shutdownCommandSent,
      shutdownConnectionGeneration == connectionGeneration,
      fd >= 0,
      handshakeComplete
    else { return false }

    shutdownCommandSent = sendCommand(.shutdown) == .sent
    if shutdownCommandSent {
      scheduleShutdownTimeout()
    }
    return shutdownCommandSent
  }

  private func scheduleShutdownTimeout() {
    guard let timeout = shutdownInactivityTimeout,
      let shutdownConnectionGeneration
    else { return }

    shutdownTimeoutTask?.cancel()
    shutdownTimeoutTask = Task {
      do {
        try await Task.sleep(for: timeout)
      } catch {
        return
      }
      guard self.shutdownConnectionGeneration == shutdownConnectionGeneration else { return }
      self.completeShutdown(result: false)
    }
  }

  private func completeShutdown(result: Bool) {
    shutdownTimeoutTask?.cancel()
    shutdownTimeoutTask = nil
    shutdownConnectionGeneration = nil
    shutdownCommandSent = false
    shutdownInactivityTimeout = nil
    shutdownResult = result
    if !result {
      closeSocket()
    }
    let waiters = shutdownWaiters
    shutdownWaiters.removeAll()
    for waiter in waiters {
      waiter.resume(returning: result)
    }
  }

  // MARK: - Raw POSIX socket I/O

  private func connectSocket(path: String) -> Int32? {
    let newFd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard newFd >= 0 else { return nil }

    var noSigPipe: Int32 = 1
    setsockopt(newFd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))

    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: addr.sun_path) else {
      close(newFd)
      return nil
    }
    withUnsafeMutableBytes(of: &addr.sun_path) { raw in
      let buf = raw.bindMemory(to: UInt8.self)
      for (i, byte) in pathBytes.enumerated() { buf[i] = byte }
      buf[pathBytes.count] = 0
    }

    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let result = withUnsafePointer(to: &addr) { ptr in
      ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
        Darwin.connect(newFd, sockPtr, len)
      }
    }
    guard result == 0 else {
      close(newFd)
      return nil
    }
    return newFd
  }

  private func encodedLine(for command: DaemonCommand) -> Data? {
    do {
      var data = try JSONEncoder().encode(command)
      data.append(0x0A)
      return data
    } catch {
      logger.error("failed to encode command: \(error.localizedDescription, privacy: .public)")
      return nil
    }
  }

  private func writeAll(_ data: Data, to fd: Int32) -> Bool {
    data.withUnsafeBytes { pointer in
      guard let baseAddress = pointer.baseAddress else { return true }
      var offset = 0
      while offset < pointer.count {
        let count = Darwin.write(
          fd, baseAddress.advanced(by: offset), pointer.count - offset)
        if count < 0, errno == EINTR { continue }
        guard count > 0 else { return false }
        offset += count
      }
      return true
    }
  }
}
