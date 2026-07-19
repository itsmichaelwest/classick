import Foundation

/// Which page of a device's disclosure group the sidebar is showing.
enum DevicePage: Hashable, Sendable {
    case music
    case settings
}

/// The macOS app restructure's single navigation-selection model — one
/// `NavigationSplitView` sidebar selection, spanning Library, per-device
/// pages, playlist editors, and History. A parent device-row click selects its
/// Music child; the chevron alone toggles disclosure.
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

    /// The name the sidebar's "+" button always sends as a new playlist's
    /// initial name (see `Sidebar.createPlaylist`). Slugified, this is the
    /// prefix `destinationForNewlyCreatedPlaylist` looks for among new slugs.
    static let newPlaylistDefaultName = "New Playlist"

    /// The sidebar's "+ New Playlist" flow (Task 3): the caller snapshots
    /// the playlist slugs that existed right before sending
    /// `.savePlaylist(.manual(slug: nil, name: newPlaylistDefaultName, ...))`,
    /// then calls this on every subsequent `playlists_update` until it
    /// returns non-nil.
    ///
    /// The daemon broadcasts `playlists_update` to every connected client and
    /// sorts it alphabetically by slug — there is no per-request correlation
    /// id on the wire (adding one is out of scope for this fix). That means
    /// `updated` can legitimately contain more than one slug absent from
    /// `priorSlugs`: this client's own new playlist AND/OR another client's
    /// concurrently-created one, in alphabetical (NOT arrival) order. Picking
    /// "the first new slug" naively would let an alphabetically-earlier
    /// concurrent creation from another client steal this client's selection.
    ///
    /// This is mitigated, not solved: among the new slugs, one that starts
    /// with the slugified `newPlaylistDefaultName` ("new-playlist") is
    /// preferred, since that's what THIS client's request would produce
    /// (modulo the daemon's own `-2`/`-3` disambiguation suffix). If no new
    /// slug carries that prefix, this falls back to the first new slug in
    /// `updated`'s order. Best-effort by design — a genuine fix needs a wire
    /// change (a correlation id on `save_playlist`/`playlists_update`).
    ///
    /// Returns `nil` if this update doesn't contain any new slug yet (an
    /// unrelated update arrived first, or the daemon hasn't replied yet).
    static func destinationForNewlyCreatedPlaylist(
        priorSlugs: Set<String>,
        updated: [PlaylistSummary]
    ) -> SidebarDestination? {
        let newSlugs = updated.map(\.slug).filter { !priorSlugs.contains($0) }
        guard !newSlugs.isEmpty else { return nil }
        let expectedPrefix = Self.slugify(newPlaylistDefaultName)
        let chosen = newSlugs.first(where: { $0.hasPrefix(expectedPrefix) }) ?? newSlugs[0]
        return .playlist(slug: chosen)
    }

    /// Bound on how many `playlists_update` bumps the sidebar's "+ New
    /// Playlist" flow waits through before giving up (Fix: premature-clear
    /// regression). `Sidebar` used to clear `priorSlugsAwaitingNewPlaylist`
    /// on the FIRST bump regardless of whether it matched — an unrelated
    /// interleaved update (e.g. another connected client's own change)
    /// would drop the pending snapshot before this client's own creation
    /// reply arrived, so the new playlist would be created but never
    /// auto-selected. Clearing only on a match risks wedging the "+" button
    /// disabled forever if a reply is somehow lost, so a small bound caps
    /// the wait either way.
    static let maxRevisionsToWaitForNewPlaylist = 3

    /// Whether the sidebar should drop `priorSlugsAwaitingNewPlaylist` after
    /// observing one more `playlists_update` bump. `matched` is whether
    /// `destinationForNewlyCreatedPlaylist` found a new slug on THIS bump;
    /// `revisionsElapsed` is the count of bumps observed (including this
    /// one) since the "+" was tapped. Clears on a match (the normal path) or
    /// once the bound is exceeded (the wedge-forever guard) — never on an
    /// unrelated, unmatched bump before the bound.
    static func shouldClearPendingNewPlaylist(matched: Bool, revisionsElapsed: Int) -> Bool {
        matched || revisionsElapsed >= maxRevisionsToWaitForNewPlaylist
    }

    /// Mirrors the daemon's `PlaylistStore::slugify` (`crates/classick/src/
    /// playlist.rs`): lowercase, alphanumerics kept, runs of anything else
    /// collapse to a single `-`, leading/trailing `-` trimmed. Only used here
    /// to compute the expected-name prefix for the new-playlist selection
    /// heuristic above — NOT a general-purpose slug generator for the app
    /// (the daemon remains the sole source of truth for actual slugs).
    private static func slugify(_ name: String) -> String {
        var out = ""
        var lastWasSeparator = true
        for char in name {
            if char.isASCII, char.isLetter || char.isNumber {
                out += char.lowercased()
                lastWasSeparator = false
            } else if !lastWasSeparator {
                out.append("-")
                lastWasSeparator = true
            }
        }
        while out.hasSuffix("-") {
            out.removeLast()
        }
        return out.isEmpty ? "playlist" : out
    }
}
