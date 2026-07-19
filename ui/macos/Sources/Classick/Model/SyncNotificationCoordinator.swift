import Foundation

struct TerminalStateConsumer {
  private struct Attempt: Hashable {
    var sessionID: UInt64?
    var timestamp: String
    var outcome: String

    init(_ entry: HistoryEntry) {
      sessionID = entry.sessionID
      timestamp = entry.timestamp
      outcome = entry.outcome
    }
  }

  private struct RetainedError {
    var attempt: Attempt
    var message: String
  }

  private var retainedErrors: [DeviceSerial: RetainedError] = [:]
  private var dismissedErrors: [DeviceSerial: Attempt] = [:]

  mutating func reset() {
    retainedErrors.removeAll()
    dismissedErrors.removeAll()
  }

  mutating func reconcile(
    devices: inout [DeviceSerial: DeviceViewState],
    previous: [DeviceSerial: DeviceViewState]
  ) {
    for serial in devices.keys {
      guard var state = devices[serial] else { continue }
      state.latestSuccessfulSync =
        state.latestSuccessfulSync ?? previous[serial]?.latestSuccessfulSync

      let attempt = state.latestAttempt.map(Attempt.init)
      if !state.connected {
        if let attempt {
          dismissedErrors[serial] = attempt
        }
        retainedErrors.removeValue(forKey: serial)
        state.lastTerminalError = nil
        devices[serial] = state
        continue
      }

      if state.sessionID != nil || state.latestAttempt?.outcome == "ok" {
        retainedErrors.removeValue(forKey: serial)
        dismissedErrors.removeValue(forKey: serial)
      }

      if case .error(let message) = state.phase, let attempt {
        if dismissedErrors[serial] == attempt {
          state.phase = Self.restingPhase(for: state)
          state.lastTerminalError = nil
        } else {
          retainedErrors[serial] = RetainedError(attempt: attempt, message: message)
        }
      } else if let retained = retainedErrors[serial],
        dismissedErrors[serial] != retained.attempt,
        attempt == retained.attempt
      {
        state.phase = .error(retained.message)
        state.lastTerminalError = retained.message
      }
      devices[serial] = state
    }

    let present = Set(devices.keys)
    retainedErrors = retainedErrors.filter { present.contains($0.key) }
    dismissedErrors = dismissedErrors.filter { present.contains($0.key) }
  }

  mutating func dismiss(
    serial: DeviceSerial,
    devices: inout [DeviceSerial: DeviceViewState]
  ) {
    guard var state = devices[serial] else { return }
    let attempt = retainedErrors[serial]?.attempt ?? state.latestAttempt.map(Attempt.init)
    if let attempt {
      dismissedErrors[serial] = attempt
    }
    retainedErrors.removeValue(forKey: serial)
    state.lastTerminalError = nil
    if case .error = state.phase {
      state.phase = Self.restingPhase(for: state)
    }
    devices[serial] = state
  }

  private static func restingPhase(for state: DeviceViewState) -> DevicePhase {
    guard state.connected else { return .disconnected }
    guard state.configured else { return .unconfigured }
    return .idle
  }
}

extension AppModel {
  func latestSuccessfulSync(for serial: DeviceSerial) -> HistoryEntry? {
    devices[serial]?.latestSuccessfulSync
  }

  /// The complete history plus a just-published successful attempt that may
  /// precede the daemon's separate `history_update` broadcast.
  var authoritativeHistory: [HistoryEntry] {
    let latestSuccesses = devices.values.compactMap(\.latestSuccessfulSync)
    let authoritativeSessions = Set(latestSuccesses.compactMap(HistorySessionIdentity.init))
    var entries = history.filter { entry in
      guard let identity = HistorySessionIdentity(entry) else { return true }
      return !authoritativeSessions.contains(identity)
    }
    entries.append(contentsOf: latestSuccesses)
    return entries.sorted { $0.timestamp < $1.timestamp }
  }
}

private struct HistorySessionIdentity: Hashable {
  var serial: DeviceSerial
  var sessionID: UInt64

  init?(_ entry: HistoryEntry) {
    guard let sessionID = entry.sessionID else { return nil }
    serial = entry.serial
    self.sessionID = sessionID
  }
}

struct SyncFinishedNotification: Equatable, Sendable {
  var serial: DeviceSerial
  var sessionID: UInt64
  var success: Bool
  var added: Int
}

/// Converts routed sync streams plus authoritative device snapshots into
/// notification intents. Raw subprocess completion is only provisional: the
/// matching device snapshot must publish the terminal attempt before anything
/// user-visible is emitted.
struct SyncNotificationCoordinator {
  private struct Session: Hashable {
    var serial: DeviceSerial
    var id: UInt64
  }

  private var observedSessions: Set<Session> = []
  private var notifiedSessions: Set<Session> = []
  private var cancelledSessions: Set<Session> = []
  private var addedBySession: [Session: Int] = [:]
  private let decoder = JSONDecoder()

  mutating func consume(
    _ event: DaemonEvent,
    devices: [DeviceSerial: DeviceViewState]
  ) -> [SyncFinishedNotification] {
    switch event {
    case .hello:
      observedSessions.removeAll()
      notifiedSessions.removeAll()
      cancelledSessions.removeAll()
      addedBySession.removeAll()

    case .syncEvent(let line, let serial, let sessionID):
      guard let serial else { return [] }
      guard devices[serial]?.sessionID == sessionID else { return [] }
      let session = Session(serial: serial, id: sessionID)
      observedSessions.insert(session)
      guard let data = line.data(using: .utf8),
        let inner = try? decoder.decode(SyncEvent.self, from: data)
      else { return [] }
      switch inner {
      case .summary(let add, _, _, _, _, _):
        addedBySession[session] = add
      case .cancelled:
        cancelledSessions.insert(session)
      default:
        break
      }

    case .deviceInventorySnapshot:
      for state in devices.values {
        if let sessionID = state.sessionID {
          observedSessions.insert(Session(serial: state.identity.serial, id: sessionID))
        }
      }
      return terminalNotifications(in: devices)

    default:
      break
    }
    return []
  }

  private mutating func terminalNotifications(
    in devices: [DeviceSerial: DeviceViewState]
  ) -> [SyncFinishedNotification] {
    var notifications: [SyncFinishedNotification] = []
    for state in devices.values {
      guard state.sessionID == nil,
        let attempt = state.latestAttempt,
        let sessionID = attempt.sessionID
      else { continue }
      let session = Session(serial: state.identity.serial, id: sessionID)
      guard observedSessions.contains(session), !notifiedSessions.contains(session) else {
        continue
      }

      notifiedSessions.insert(session)
      observedSessions.remove(session)
      defer {
        cancelledSessions.remove(session)
        addedBySession.removeValue(forKey: session)
      }

      guard attempt.outcome != "cancelled", !cancelledSessions.contains(session) else { continue }
      if case .paused = state.phase { continue }

      let success: Bool
      switch state.phase {
      case .error:
        success = false
      default:
        success = attempt.outcome == "ok"
      }
      notifications.append(
        SyncFinishedNotification(
          serial: session.serial,
          sessionID: session.id,
          success: success,
          added: success ? (addedBySession[session] ?? 0) : 0))
    }
    return notifications.sorted {
      ($0.serial, $0.sessionID) < ($1.serial, $1.sessionID)
    }
  }
}
