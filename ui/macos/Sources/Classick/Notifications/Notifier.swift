import Foundation
import UserNotifications
import os

/// Thin wrapper around `UserNotificationCenter` for sync-completion banners.
/// Not sandboxed, so authorization still has to be requested explicitly like
/// any other Mac app; ad-hoc signing (see bundle.sh) may prevent macOS from
/// registering the app with Notification Center at all — see AGENTS.md /
/// the SP2 plan's Risk 1 note. A stable signing identity resolves that.
enum Notifier {
  private static let logger = Logger(subsystem: "com.classick.app", category: "Notifier")

  static func requestAuth() {
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
      granted, error in
      if let error {
        logger.error(
          "notification auth request failed: \(error.localizedDescription, privacy: .public)")
      } else {
        logger.info("notification auth granted: \(granted, privacy: .public)")
      }
    }
  }

  /// Whether a sync-finished banner should fire for the given `notify_on`
  /// wire value ("all" | "errors_only" | "none"). Unknown/nil defaults to
  /// "all" so a missing preference still notifies — matching the daemon's
  /// `NotifyLevel::All` default. Pure so the policy is unit-testable.
  ///
  /// Library scans never reach this policy: `SyncNotificationCoordinator`
  /// accepts only serial/session-scoped device attempts.
  static func shouldPostSyncFinished(notifyOn: String?, success: Bool) -> Bool {
    switch notifyOn {
    case "none": return false
    case "errors_only": return !success
    default: return true
    }
  }

  /// The banner body for a finished sync — pure for testability. A
  /// playlists/settings-only sync adds zero tracks; "0 added" read like
  /// the sync did nothing, which is exactly the wrong message for a run
  /// that DID apply changes.
  nonisolated static func successBody(added: Int) -> String {
    added > 0
      ? "\(added) track\(added == 1 ? "" : "s") added"
      : "Your iPod is up to date — playlists and settings applied."
  }

  nonisolated static func title(for notification: SyncFinishedNotification) -> String {
    notification.success
      ? "Sync complete — \(notification.displayName)"
      : "Sync failed — \(notification.displayName)"
  }

  static func syncFinished(_ notification: SyncFinishedNotification) {
    let content = UNMutableNotificationContent()
    content.title = title(for: notification)
    if notification.success {
      content.body = successBody(added: notification.added)
    }
    content.sound = .default

    let request = UNNotificationRequest(
      identifier: "classick.sync.\(notification.serial).\(notification.sessionID)",
      content: content,
      trigger: nil)
    UNUserNotificationCenter.current().add(request) { error in
      if let error {
        logger.error("failed to post notification: \(error.localizedDescription, privacy: .public)")
      }
    }
  }
}
