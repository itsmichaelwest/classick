import AppKit
import SwiftUI

/// Hosts `ChooseMusicWindow` in an AppKit `NSWindow` owned by the app
/// delegate — same deterministic-presentation rationale as
/// `SetupWindowController`.
@MainActor
final class ChooseMusicWindowController {
    private var window: NSWindow?

    func show(
        model: AppModel,
        onAppear: @escaping () -> Void,
        onScan: @escaping () -> Void,
        onPreview: @escaping (SelectionMode, [SelectionRule]) -> Void,
        onSave: @escaping (SelectionMode, [SelectionRule]) -> Void
    ) {
        NSApp.activate(ignoringOtherApps: true)
        if let window {
            window.makeKeyAndOrderFront(nil)
            return
        }
        let root = ChooseMusicWindow(
            model: model,
            onAppear: onAppear,
            onScan: onScan,
            onPreview: onPreview,
            onSave: onSave,
            onClose: { [weak self] in self?.window?.close() })
        let hosting = NSHostingController(rootView: root)
        let win = NSWindow(contentViewController: hosting)
        win.title = "Choose Music"
        win.styleMask = [.titled, .closable, .resizable]
        win.setContentSize(NSSize(width: 560, height: 620))
        win.isReleasedWhenClosed = false
        win.center()
        window = win
        win.makeKeyAndOrderFront(nil)
    }
}
