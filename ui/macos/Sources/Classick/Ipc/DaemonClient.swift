import Darwin
import Foundation
import os

enum SendDisposition: Equatable, Sendable {
  case sent
  case queued
  case dropped
}

struct DurableIntent: Sendable {
  let key: WireV3DurableIntentKey
  let requestID: UUID
  let command: WireV3Command
  let bytes: Data
  fileprivate var lastWrittenConnection: Int?
}

struct DurableIntentOutbox: Sendable {
  private var intents: [DurableIntent] = []

  var requestIDs: [String] { intents.map { $0.requestID.uuidString.lowercased() } }

  mutating func upsert(_ command: WireV3Command) throws {
    var command = command.normalizedForDurableEncoding()
    guard let key = command.durableIntentKey else { return }
    let requestID = command.requestID
    guard !intents.contains(where: { $0.requestID == requestID }) else { return }

    if let incomingRules = command.additiveRules,
      let queued = intents.last(where: { $0.key == key && $0.lastWrittenConnection == nil }),
      let queuedRules = queued.command.additiveRules
    {
      command = command.replacingAdditiveRules(
        WireV3Command.canonicalAdditiveRules(queuedRules + incomingRules))
    }
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

  mutating func markWritten(requestID: UUID, connectionGeneration: Int) {
    guard let index = intents.firstIndex(where: { $0.requestID == requestID }) else { return }
    intents[index].lastWrittenConnection = connectionGeneration
  }

  func wasWritten(requestID: UUID, connectionGeneration: Int) -> Bool {
    intents.first(where: { $0.requestID == requestID })?.lastWrittenConnection
      == connectionGeneration
  }

  @discardableResult
  mutating func acknowledge(_ acknowledgement: WireV3DurableAcknowledgement) -> Bool {
    guard
      let index = intents.firstIndex(where: {
        $0.requestID == acknowledgement.requestID && $0.lastWrittenConnection != nil
      })
    else { return false }
    if acknowledgement.terminalFailure {
      guard acknowledgement.target == intents[index].key else { return false }
    } else {
      guard intents[index].command.isCommitted(by: acknowledgement) else { return false }
    }
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

  private var continuation: AsyncStream<WireV3Event>.Continuation?
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
  private(set) var connectionCompatibility: WireV3ConnectionCompatibility?

  init(socketPath: String = NSTemporaryDirectory() + "classick.sock") {
    self.socketPath = socketPath
  }

  /// The stream of daemon events. Intended to be called once; the
  /// continuation is retained for the lifetime of the client.
  func events() -> AsyncStream<WireV3Event> {
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
  /// A connected socket gets only a short grace period to finish its handshake.
  func shutdownAndWait(
    timeout: Duration,
    connectionGrace: Duration = .seconds(1)
  ) async -> Bool {
    if let shutdownResult { return shutdownResult }

    return await withCheckedContinuation { continuation in
      shutdownWaiters.append(continuation)
      guard shutdownConnectionGeneration == nil else { return }

      isRunning = false
      shutdownConnectionGeneration = connectionGeneration
      shutdownInactivityTimeout = timeout
      guard fd >= 0 else {
        completeShutdown(result: false)
        return
      }
      if !sendPendingShutdownIfReady() {
        scheduleShutdownTimeout(after: connectionGrace)
      }
    }
  }

  @discardableResult
  func send(_ command: WireV3Command) async -> SendDisposition {
    sendCommand(command)
  }

  private func sendCommand(_ command: WireV3Command) -> SendDisposition {
    if command.durableIntentKey != nil {
      do {
        try durableOutbox.upsert(command)
      } catch {
        logger.error(
          "failed to encode durable command: \(error.localizedDescription, privacy: .public)")
        return .dropped
      }
      flushDurableIntents()
      let requestID = command.requestID
      return durableOutbox.wasWritten(
        requestID: requestID, connectionGeneration: connectionGeneration)
        ? .sent : .queued
    }

    guard fd >= 0, handshakeComplete else {
      logger.warning(
        "send(\(String(describing: command), privacy: .public)) dropped — not connected")
      return .dropped
    }
    guard let bytes = encodedLine(for: command), DaemonSocketIO.writeAll(bytes, to: fd) else {
      closeSocket()
      return .dropped
    }
    return .sent
  }

  @discardableResult
  func send(_ commands: [WireV3Command]) async -> [SendDisposition] {
    commands.map(sendCommand)
  }

  private func flushDurableIntents() {
    guard fd >= 0, handshakeComplete else { return }
    while let intent = durableOutbox.nextIntent(for: connectionGeneration) {
      guard DaemonSocketIO.writeAll(intent.bytes, to: fd) else {
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
      if let connectedFd = DaemonSocketIO.connect(path: socketPath) {
        fd = connectedFd
        connectionGeneration += 1
        handshakeComplete = false
        connectionCompatibility = nil
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
    for await line in DaemonSocketIO.lines(from: connectionFd) {
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

    let event: WireV3Event
    if isFirstLine {
      let compatibility = WireV3Codec.admitDaemonHello(data)
      connectionCompatibility = compatibility
      guard case .compatible(let hello) = compatibility else {
        let message = "daemon handshake failed: \(compatibility)"
        logger.fault("\(message, privacy: .public)")
        lastFatalError = message
        isRunning = false
        self.connectionGeneration += 1
        closeSocket()
        continuation?.finish()
        return
      }
      event = .hello(hello)
    } else {
      do {
        let decoded = try WireV3Codec.decode(data, direction: .daemonToDesktopEvents)
        guard case .event(let typedEvent) = decoded else { return }
        event = typedEvent
      } catch {
        let raw = String(data: data.prefix(300), encoding: .utf8) ?? "<non-utf8>"
        logger.error(
          "rejected protocol 3 daemon line: \(String(describing: error), privacy: .public) line=\(raw, privacy: .public)"
        )
        return
      }
    }

    if shutdownCommandSent, shutdownConnectionGeneration == connectionGeneration {
      scheduleShutdownInactivityTimeout()
    }

    if isFirstLine {
      handshakeComplete = true
      if sendPendingShutdownIfReady() {
        continuation?.yield(event)
        return
      }
      continuation?.yield(event)
      await send(.subscribeInventory(requestID: WireV3Command.newRequestID()))
      await send(.getInventory(requestID: WireV3Command.newRequestID()))
      // Explicitly pull config on every (re)connect. The daemon only
      // Without an explicit query the app may not learn persisted global
      // settings on a cached-name plug-in, leaving Settings at defaults.
      await send(.getGlobalConfig(requestID: WireV3Command.newRequestID()))
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
    version.split(separator: ".").first == "3"
  }

  @discardableResult
  private func sendPendingShutdownIfReady() -> Bool {
    guard !shutdownCommandSent,
      shutdownConnectionGeneration == connectionGeneration,
      fd >= 0,
      handshakeComplete
    else { return false }

    shutdownCommandSent = sendCommand(.shutdown(requestID: WireV3Command.newRequestID())) == .sent
    if shutdownCommandSent {
      scheduleShutdownInactivityTimeout()
    }
    return shutdownCommandSent
  }

  private func scheduleShutdownInactivityTimeout() {
    guard let timeout = shutdownInactivityTimeout else { return }
    scheduleShutdownTimeout(after: timeout)
  }

  private func scheduleShutdownTimeout(after timeout: Duration) {
    guard let shutdownConnectionGeneration else { return }

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

  private func encodedLine(for command: WireV3Command) -> Data? {
    do {
      var data = try JSONEncoder().encode(command)
      data.append(0x0A)
      return data
    } catch {
      logger.error("failed to encode command: \(error.localizedDescription, privacy: .public)")
      return nil
    }
  }

}
