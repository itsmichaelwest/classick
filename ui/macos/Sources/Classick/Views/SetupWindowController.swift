import AppKit
import SwiftUI

/// Hosts `SetupWindow` in an AppKit `NSWindow` owned by the app delegate.
///
/// We deliberately do NOT use a SwiftUI `Window(id:)` scene: that scene's
/// content is materialized lazily (only when `openWindow` runs), which is
/// exactly why first-run setup never presented itself — nothing opened the
/// scene on launch. An AppKit-hosted window can be shown deterministically
/// from the delegate's event loop the moment we learn the user is
/// unconfigured, and reused for the manual "Set Up Classick…" menu action.
@MainActor
final class SetupWindowController {
  private var window: NSWindow?

  /// Shows the setup window (creating it on first use), bringing this
  /// `LSUIElement` accessory app to the front so the window can't open
  /// behind whatever currently has focus.
  func show(
    model: AppModel, preferredSerial: DeviceID?,
    onDone: @escaping (_ source: String, _ autoSync: Bool, _ serial: DeviceID) -> Void
  ) {
    NSApp.activate(ignoringOtherApps: true)

    if let window {
      window.makeKeyAndOrderFront(nil)
      return
    }

    let root = SetupWindow(
      model: model,
      preferredSerial: preferredSerial,
      onDone: onDone,
      onClose: { [weak self] in self?.window?.close() })
    let hosting = NSHostingController(rootView: root)
    let win = NSWindow(contentViewController: hosting)
    win.title = "Set Up Classick"
    win.styleMask = [.titled, .closable]
    win.isReleasedWhenClosed = false
    win.center()
    window = win
    win.makeKeyAndOrderFront(nil)
  }
}
