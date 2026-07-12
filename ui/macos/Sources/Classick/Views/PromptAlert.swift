import AppKit

/// Presents `AppModel.pendingPrompt` as a native modal `NSAlert`, one button
/// per option.
///
/// Driven from `AppDelegate`'s daemon-event loop (mirroring
/// `observeForNotification`) rather than a SwiftUI view: `MenuBarExtra`'s
/// `.menu` content is only materialized when the user opens the menu (see
/// the doc comment on `AppDelegate` in `ClassickApp.swift`), so a modal that
/// depended on that view tree existing would never fire while the menu is
/// closed. A plain AppKit alert triggered straight from the always-running
/// event loop has no such dependency.
@MainActor
enum PromptAlert {
    /// Activates the app (required to bring a modal to the front from an
    /// `LSUIElement` accessory app) and blocks — via `NSAlert.runModal()` —
    /// until the user picks an option. Returns the chosen option's index.
    static func present(_ prompt: PendingPrompt) -> Int32 {
        NSApp.activate(ignoringOtherApps: true)

        let alert = NSAlert()
        alert.messageText = prompt.message
        alert.alertStyle = .informational

        let options = prompt.options.isEmpty ? ["OK"] : prompt.options
        for option in options {
            alert.addButton(withTitle: option)
        }

        // NSAlert.runModal() response codes are assigned in the order
        // buttons were added, starting at .alertFirstButtonReturn — so the
        // offset from that base is exactly the option's index.
        let response = alert.runModal()
        let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
        return Int32(index)
    }
}
