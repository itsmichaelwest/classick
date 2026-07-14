import Foundation
import Darwin
import os

/// Owns the Unix-domain-socket connection to the classick daemon.
///
/// Blocking `read()` must never run inside actor-isolated code — it would
/// stall every other call on this actor (including `send`) for as long as
/// the daemon stays silent. So the read loop runs on a dedicated background
/// `Thread`, which only ever touches its own local copy of the fd and
/// reports decoded lines back onto the actor via `Task { await ... }`. The
/// only state shared across the isolation boundary is the fd itself (a
/// plain, Sendable `Int32`); the kernel already serializes concurrent
/// read/write on it from different threads, so no additional Swift-side
/// synchronization is needed.
actor DaemonClient {
    private let logger = Logger(subsystem: "com.classick.app", category: "DaemonClient")
    private let socketPath: String

    private var continuation: AsyncStream<DaemonEvent>.Continuation?
    private var fd: Int32 = -1
    private var isRunning = false
    private var generation = 0

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
        closeSocket()
        continuation?.finish()
    }

    func send(_ cmd: DaemonCommand) async {
        guard fd >= 0 else {
            logger.warning("send(\(String(describing: cmd), privacy: .public)) dropped — not connected")
            return
        }
        writeCommand(cmd, to: fd)
    }

    // MARK: - Connect / reconnect loop

    private func runLoop(generation: Int) async {
        var backoff: Duration = .milliseconds(250)
        let maxBackoff: Duration = .seconds(10)

        while isRunning && generation == self.generation {
            if let connectedFd = connectSocket(path: socketPath) {
                fd = connectedFd
                backoff = .milliseconds(250)
                await readUntilDisconnected(connectionFd: connectedFd, generation: generation)
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
        if fd >= 0 {
            // `shutdown(SHUT_RDWR)` first: the background reader thread is
            // blocked in a plain `read()` on this fd, and on Darwin a bare
            // `close()` doesn't reliably unblock a concurrent read on the
            // same descriptor from another thread. `shutdown` forces that
            // read to return (EOF/error) so the reader thread's loop exits
            // and its `resume.resume()` fires instead of hanging forever.
            Darwin.shutdown(fd, SHUT_RDWR)
            close(fd)
            fd = -1
        }
    }

    /// Spawns a background thread that blocks on `read()`, splits the
    /// stream into newline-delimited lines, decodes each into a
    /// `DaemonEvent`, and reports it back onto the actor. Suspends until
    /// the thread observes EOF or a read error.
    private func readUntilDisconnected(connectionFd: Int32, generation: Int) async {
        await withCheckedContinuation { (resume: CheckedContinuation<Void, Never>) in
            let thread = Thread { [weak self] in
                var buffer = Data()
                var isFirstLine = true
                var readBuffer = [UInt8](repeating: 0, count: 4096)

                readLoop: while true {
                    let n = readBuffer.withUnsafeMutableBytes { ptr -> Int in
                        Darwin.read(connectionFd, ptr.baseAddress, ptr.count)
                    }
                    if n <= 0 { break readLoop }
                    buffer.append(contentsOf: readBuffer[0..<n])

                    while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                        let lineData = buffer.subdata(in: buffer.startIndex..<newlineIndex)
                        buffer.removeSubrange(buffer.startIndex...newlineIndex)
                        guard !lineData.isEmpty, let self else { continue }
                        let wasFirst = isFirstLine
                        isFirstLine = false
                        Task { await self.handleLine(lineData, isFirstLine: wasFirst, generation: generation) }
                    }
                }
                resume.resume()
            }
            thread.name = "classick.DaemonClient.reader"
            thread.start()
        }
    }

    private func handleLine(_ data: Data, isFirstLine: Bool, generation: Int) async {
        guard generation == self.generation else { return }

        let event: DaemonEvent
        do {
            event = try JSONDecoder().decode(DaemonEvent.self, from: data)
        } catch {
            logger.error("failed to decode daemon line: \(error.localizedDescription, privacy: .public)")
            return
        }

        if isFirstLine {
            guard case let .hello(protocolVersion, _) = event,
                  protocolVersion.split(separator: ".").first == "1" else {
                let message = "daemon handshake failed: expected hello major version 1, got \(event)"
                logger.fault("\(message, privacy: .public)")
                lastFatalError = message
                isRunning = false
                closeSocket()
                continuation?.finish()
                return
            }
            continuation?.yield(event)
            await send(.subscribeDeviceEvents)
            await send(.getStatus)
            // Explicitly pull config on every (re)connect. The daemon only
            // *pushes* config_update on a name change or after a save, so
            // without this the app never learns the persisted iPod identity on
            // a cached-name plug-in — leaving `configuredSerial` nil, the menu
            // stuck on "Set Up", and Settings showing defaults.
            await send(.getConfig)
            // Warm the selection so the menu's "Selection active" line and the
            // Choose Music window seed correctly on (re)connect.
            await send(.getSelection)
            return
        }

        continuation?.yield(event)
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

    private func writeCommand(_ command: DaemonCommand, to fd: Int32) {
        do {
            var data = try JSONEncoder().encode(command)
            data.append(0x0A)
            data.withUnsafeBytes { ptr in
                var offset = 0
                while offset < ptr.count {
                    let n = Darwin.write(fd, ptr.baseAddress!.advanced(by: offset), ptr.count - offset)
                    if n <= 0 { break }
                    offset += n
                }
            }
        } catch {
            logger.error("failed to encode command: \(error.localizedDescription, privacy: .public)")
        }
    }
}
