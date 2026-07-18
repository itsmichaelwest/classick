import Foundation

/// Which page of a device's disclosure group the sidebar is showing.
enum DevicePage: Hashable, Sendable {
    case music
    case settings
}

/// The macOS app restructure's single navigation-selection model — one
/// `NavigationSplitView` sidebar selection, spanning Library, per-device
/// pages, playlist editors, and History. See
/// `docs/superpowers/plans/2026-07-17-macos-app-restructure.md` Global
/// Constraints: "Parent device row click selects its Music child; the
/// chevron alone toggles disclosure."
enum SidebarDestination: Hashable, Sendable {
    case library
    case device(serial: String, page: DevicePage)
    case playlist(slug: String)
    case history

    /// Clicking a device row's label (not its disclosure chevron) selects
    /// that device's Music page — never Settings, and never merely
    /// expanding/collapsing the row.
    static func destinationForDeviceRowClick(serial: String) -> SidebarDestination {
        .device(serial: serial, page: .music)
    }
}
