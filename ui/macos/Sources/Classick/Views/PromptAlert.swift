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
  enum Response: Equatable {
    case choice(UInt32)
    case form(String?)
  }

  /// Activates the app (required to bring a modal to the front from an
  /// `LSUIElement` accessory app) and blocks until the interaction is answered.
  static func present(_ prompt: PendingPrompt) -> Response {
    NSApp.activate(ignoringOtherApps: true)

    let alert = NSAlert()
    alert.messageText = prompt.message
    alert.alertStyle = .informational

    switch prompt.kind {
    case .choice(let suppliedOptions):
      let options = suppliedOptions.isEmpty ? ["OK"] : suppliedOptions
      for option in options {
        alert.addButton(withTitle: option)
      }
      let response = alert.runModal()
      let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
      return .choice(UInt32(clamping: index))

    case .form(let initial, let hint):
      if let hint, !hint.isEmpty {
        alert.informativeText = hint
      }
      let field = NSTextField(string: initial ?? "")
      field.placeholderString = hint
      field.frame = NSRect(x: 0, y: 0, width: 320, height: 24)
      alert.accessoryView = field
      alert.addButton(withTitle: "Submit")
      alert.addButton(withTitle: "Cancel")
      let response = alert.runModal()
      return .form(response == .alertFirstButtonReturn ? field.stringValue : nil)
    }
  }

  nonisolated static func command(
    for prompt: PendingPrompt, response: Response, requestID: UUID
  ) -> WireV3Command {
    switch response {
    case .choice(let choice):
      .promptDecision(
        route: prompt.route, requestID: requestID, promptID: prompt.id, choice: choice)
    case .form(let value):
      .formDecision(
        route: prompt.route, requestID: requestID, promptID: prompt.id, value: value)
    }
  }
}
