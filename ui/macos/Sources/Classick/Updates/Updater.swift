// `Sparkle` is an app-target-only dependency (added to `Classick` in
// project.yml / the generated Xcode project), NOT to Package.swift — the SPM
// package only exists to drive `swift test`, and pulling a binary XCFramework
// dependency into it isn't worth the extra resolve on every `swift test`.
// Guarding the whole file on `canImport` lets it compile to nothing under
// `swift test` and be fully live under `xcodebuild`, instead of forking the
// build graph.
#if canImport(Sparkle)
import AppKit
import Sparkle

/// Thin wrapper around Sparkle's standard updater controller. Owned by
/// `AppDelegate` for the app's whole lifetime (mirrors `DaemonClient` /
/// `DaemonProcess` — Sparkle needs a single long-lived controller instance).
@MainActor
final class Updater {
    private let controller = SPUStandardUpdaterController(
        startingUpdater: true,
        updaterDelegate: nil,
        userDriverDelegate: nil)

    /// This is an `LSUIElement` (accessory) app with no Dock icon, so Sparkle's
    /// "checking for updates…" / update-available window can open behind
    /// whatever app currently has focus unless we activate first — same
    /// footgun as `openSetupWindow`/`openSettingsWindow` in `ClassickApp`.
    func checkForUpdates() {
        NSApp.activate(ignoringOtherApps: true)
        controller.checkForUpdates(nil)
    }
}
#endif
