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
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, error in
            if let error {
                logger.error("notification auth request failed: \(error.localizedDescription, privacy: .public)")
            } else {
                logger.info("notification auth granted: \(granted, privacy: .public)")
            }
        }
    }

    static func syncFinished(success: Bool, added: Int) {
        let content = UNMutableNotificationContent()
        if success {
            content.title = "Sync complete"
            content.body = "\(added) added"
        } else {
            content.title = "Sync failed"
        }
        content.sound = .default

        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request) { error in
            if let error {
                logger.error("failed to post notification: \(error.localizedDescription, privacy: .public)")
            }
        }
    }
}
