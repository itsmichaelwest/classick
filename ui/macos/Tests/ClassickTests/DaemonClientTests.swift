import XCTest
import Darwin
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

final class DaemonClientTests: XCTestCase {
    /// Polls `collector` until it has at least `minCount` events or the
    /// deadline passes, so the test never hangs forever if the client stalls.
    private func waitForEvents(_ collector: EventCollector, minCount: Int, timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while await collector.count < minCount && Date() < deadline {
            try? await Task.sleep(for: .milliseconds(20))
        }
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
            writeLine(#"{"type":"hello","protocol_version":"1.1.0","core_version":"1.1.0"}"# + "\n", to: clientFd)
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
        XCTAssertGreaterThanOrEqual(events.count, 2, "expected hello + device_connected, got \(events)")

        guard case let .hello(protocolVersion, coreVersion) = events[0] else {
            return XCTFail("expected .hello first, got \(events[0])")
        }
        XCTAssertEqual(protocolVersion, "1.1.0")
        XCTAssertEqual(coreVersion, "1.1.0")

        guard case let .deviceConnected(serial, modelLabel, drive, name) = events[1] else {
            return XCTFail("expected .deviceConnected second, got \(events[1])")
        }
        XCTAssertEqual(serial, "0x000A27002138B0A8")
        XCTAssertEqual(modelLabel, "iPod Classic (3rd gen)")
        XCTAssertEqual(drive, "/Volumes/IPOD")
        XCTAssertEqual(name, "Test iPod")
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
                writeLine(#"{"type":"hello","protocol_version":"1.1.0","core_version":"1.1.0"}"# + "\n", to: clientFd)
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
        XCTAssertGreaterThanOrEqual(helloCount, 2, "expected two hello events across reconnects, got \(events)")
    }
}
