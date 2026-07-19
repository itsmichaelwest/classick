import AppKit
import Darwin
import Foundation
import os

/// Locates, spawns, and owns the `classick --daemon` subprocess.
///
/// `ensureRunning()` first checks whether something is already listening on
/// the daemon's Unix socket (another instance of the app, or a daemon
/// started manually for debugging) — if so it attaches without spawning a
/// second daemon, since only one process may bind that socket. Otherwise it
/// locates and launches the `classick` binary in `--daemon` mode and retains
/// the `Process` handle so a stalled shutdown can force-terminate only the
/// daemon this app owns.
///
/// Runs on the main actor: it's only ever driven from app-lifecycle events
/// (`applicationDidFinishLaunching`/`applicationShouldTerminate`), and
/// `Process` itself is not `Sendable`, so keeping this off any background
/// queue sidesteps the isolation question entirely.
@MainActor
final class DaemonProcess {
  private let logger = Logger(subsystem: "com.classick.app", category: "DaemonProcess")
  private let socketPath: String
  private var process: Process?

  init(socketPath: String = NSTemporaryDirectory() + "classick.sock") {
    self.socketPath = socketPath
  }

  private var ownsDaemon: Bool {
    process?.isRunning == true
  }

  /// Stops an owned daemon, but only detaches from a daemon started elsewhere.
  func stopForQuit(
    client: DaemonClient,
    ownedShutdownTimeout: Duration = .seconds(130)
  ) async -> Bool {
    guard ownsDaemon else {
      await client.stop()
      return true
    }
    return await client.shutdownAndWait(timeout: ownedShutdownTimeout)
  }

  /// Attaches to an already-running daemon if one answers on the socket;
  /// otherwise locates and spawns a new `classick --daemon` process.
  func ensureRunning() async {
    guard !Task.isCancelled else { return }
    let priorApplications = Self.priorApplications()
    if !priorApplications.isEmpty {
      await Self.waitForPriorApplicationAndSocket(
        priorApplicationIsRunning: {
          priorApplications.contains { !$0.isTerminated }
        },
        socketExists: {
          FileManager.default.fileExists(atPath: self.socketPath)
        })
    }
    guard !Task.isCancelled else { return }

    if Self.socketIsAnswering(path: socketPath) {
      logger.info(
        "daemon already listening on \(self.socketPath, privacy: .public) — attaching, not spawning"
      )
      return
    }

    guard let binaryURL = Self.locateBinary(logger: logger) else {
      logger.fault(
        "could not locate the classick binary in the app bundle or the dev tree — nothing to spawn")
      return
    }

    let proc = Process()
    proc.executableURL = binaryURL
    proc.arguments = Self.arguments(parentPID: getpid())
    proc.standardInput = FileHandle.nullDevice

    do {
      try proc.run()
      process = proc
      logger.info("spawned owned daemon at pid \(proc.processIdentifier)")
    } catch {
      logger.fault(
        "failed to spawn \(binaryURL.path, privacy: .public): \(String(describing: error), privacy: .public)"
      )
    }
  }

  /// Terminates the daemon process this instance spawned, if any. A no-op
  /// if we merely attached to a pre-existing daemon — we don't own that
  /// process's lifecycle in that case, and killing it would be surprising
  /// (e.g. it might belong to another running instance of the app).
  func forceTerminateOwnedDaemon() {
    guard let proc = process, proc.isRunning else { return }
    if kill(proc.processIdentifier, SIGKILL) != 0 {
      logger.error(
        "failed to force-terminate owned daemon pid \(proc.processIdentifier): \(String(cString: strerror(errno)), privacy: .public)"
      )
    }
    process = nil
  }

  static func arguments(parentPID: pid_t) -> [String] {
    ["--daemon", "--daemon-parent-pid", String(parentPID)]
  }

  static func waitForPriorApplicationAndSocket(
    priorApplicationIsRunning: @escaping @MainActor () -> Bool,
    socketExists: @escaping @MainActor () -> Bool,
    pollInterval: Duration = .milliseconds(100)
  ) async {
    while priorApplicationIsRunning() || socketExists() {
      guard !Task.isCancelled else { return }
      try? await Task.sleep(for: pollInterval)
    }
  }

  private static func priorApplications() -> [NSRunningApplication] {
    guard let bundleIdentifier = Bundle.main.bundleIdentifier else { return [] }
    let currentPID = getpid()
    return NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
      .filter { $0.processIdentifier != currentPID && !$0.isTerminated }
  }

  // MARK: - Binary location

  /// Bundled release layout first (`Classick.app/Contents/Resources/classick`,
  /// staged there by `bundle.sh`); failing that, walk upward from the
  /// running executable's directory looking for the workspace's
  /// `target/release/classick` — covers `swift run`/`swift build` dev
  /// loops where `Bundle.main` resolves to `.build/<triple>/debug/`.
  private static func locateBinary(logger: Logger) -> URL? {
    if let bundled = Bundle.main.url(forResource: "classick", withExtension: nil) {
      logger.debug("using bundled classick binary at \(bundled.path, privacy: .public)")
      return bundled
    }

    var dir = Bundle.main.bundleURL.standardizedFileURL
    for _ in 0..<8 {
      let candidate = dir.appendingPathComponent("target/release/classick").standardizedFileURL
      if FileManager.default.isExecutableFile(atPath: candidate.path) {
        logger.debug("using dev-tree classick binary at \(candidate.path, privacy: .public)")
        return candidate
      }
      let parent = dir.deletingLastPathComponent()
      if parent == dir { break }
      dir = parent
    }

    // Last resort: the literal offset called out for the common case of
    // running straight out of `ui/macos/.build/<config>/`.
    let fallback = Bundle.main.bundleURL
      .appendingPathComponent("../../target/release/classick")
      .standardizedFileURL
    if FileManager.default.isExecutableFile(atPath: fallback.path) {
      logger.debug("using hardcoded fallback classick binary at \(fallback.path, privacy: .public)")
      return fallback
    }

    return nil
  }

  // MARK: - Socket probe

  /// Best-effort "is something already listening here" check: opens a
  /// throwaway connection and immediately closes it. False negatives
  /// (stale socket file, nothing listening) fall through to spawning,
  /// which is the safe default — a spurious extra daemon loses a race for
  /// the socket and simply can't bind it.
  private static func socketIsAnswering(path: String) -> Bool {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }

    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    let pathBytes = Array(path.utf8)
    guard pathBytes.count < MemoryLayout.size(ofValue: addr.sun_path) else { return false }
    withUnsafeMutableBytes(of: &addr.sun_path) { raw in
      let buf = raw.bindMemory(to: UInt8.self)
      for (i, byte) in pathBytes.enumerated() { buf[i] = byte }
      buf[pathBytes.count] = 0
    }

    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let result = withUnsafePointer(to: &addr) { ptr in
      ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
        Darwin.connect(fd, sockPtr, len)
      }
    }
    return result == 0
  }
}
