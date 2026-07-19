import AppKit

@MainActor
final class DaemonShutdownCoordinator {
  private var shutdownTask: Task<Void, Never>?
  private var didReply = false

  func begin(
    shutdown: @escaping @Sendable () async -> Bool,
    forceTerminateOwnedDaemon: @escaping @MainActor () -> Void,
    reply: @escaping @MainActor (Bool) -> Void
  ) -> NSApplication.TerminateReply {
    guard shutdownTask == nil else { return .terminateLater }

    shutdownTask = Task {
      let didExitCleanly = await shutdown()
      guard !didReply else { return }
      if !didExitCleanly {
        forceTerminateOwnedDaemon()
      }
      didReply = true
      reply(true)
    }
    return .terminateLater
  }
}
