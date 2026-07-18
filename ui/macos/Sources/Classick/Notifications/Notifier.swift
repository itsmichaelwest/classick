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

    /// Whether a sync-finished banner should fire for the given `notify_on`
    /// wire value ("all" | "errors_only" | "none"). Unknown/nil defaults to
    /// "all" so a missing preference still notifies — matching the daemon's
    /// `NotifyLevel::All` default. Pure so the policy is unit-testable.
    ///
    /// `isScanning`: the daemon drives `--scan-library` subprocesses through
    /// the SAME sync-wire pipeline as a real sync, so a library scan also
    /// emits a `finish` line — which used to fire a bogus "Sync complete /
    /// N added" banner for tracks added to the INDEX, not the iPod. The
    /// daemon's `status_update.state == "scanning"` (tracked as
    /// `AppModel.isScanning`, set before the subprocess streams and cleared
    /// only after it completes) is the authoritative discriminator: no
    /// banner for scan finishes.
    static func shouldPostSyncFinished(notifyOn: String?, success: Bool, isScanning: Bool) -> Bool {
        guard !isScanning else { return false }
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

    static func syncFinished(success: Bool, added: Int) {
        let content = UNMutableNotificationContent()
        if success {
            content.title = "Sync complete"
            content.body = successBody(added: added)
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
