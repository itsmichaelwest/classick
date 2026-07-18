import SwiftUI

/// `MainWindow`'s source-list sidebar (Task 3, Figma frame `3:3773`):
/// Library / Devices / Playlists / History sections. Devices are a
/// `DisclosureGroup` with Music/Settings children; Playlists owns the
/// "+ New Playlist" flow (see `SidebarDestination.destinationForNewlyCreatedPlaylist`).
///
/// Canonical-surface rule: this view carries NO sync-intent affordances
/// (checkboxes, mode pickers) — those live only on the device Music page.
struct Sidebar: View {
    var model: AppModel
    @Binding var selection: SidebarDestination?
    /// TRUE disk eject (unmount so the iPod is safe to unplug) — NOT
    /// forget-iPod, which lives on the device Settings page. These were
    /// once conflated: the eject glyph used to call `onForgetIpod`, so
    /// clicking it unpaired the device and never touched the volume.
    var onEjectIpod: () -> Void
    var onSavePlaylist: (PlaylistPayload) -> Void

    /// Snapshot of playlist slugs taken the moment "+" is tapped, so the
    /// `onChange` below can recognize the newly assigned slug once the
    /// daemon's `playlists_update` reply arrives. `nil` when no creation is
    /// in flight.
    @State private var priorSlugsAwaitingNewPlaylist: Set<String>?
    /// Count of `playlists_update` bumps observed since `priorSlugsAwaitingNewPlaylist`
    /// was snapshotted — feeds `SidebarDestination.shouldClearPendingNewPlaylist`'s
    /// bound so an unrelated interleaved update can't clear pending before
    /// this client's own creation reply arrives (Fix: premature-clear
    /// regression), while still guaranteeing the "+" button can never wedge
    /// disabled forever.
    @State private var newPlaylistRevisionsElapsed = 0

    /// One device row's disclosure state. Only one iPod is ever paired at a
    /// time (see `AppModel.device`/`config.ipod`), so a single optional is
    /// enough: `nil` means "use the default for the current connection
    /// state" (expanded while connected, collapsed while disconnected —
    /// Global Constraints "Disconnected device: … collapsed by default");
    /// once the user manually toggles it, that explicit choice sticks.
    @State private var deviceManuallyExpanded: Bool?

    var body: some View {
        // Native system selection throughout — `List(selection:)` + `.tag`.
        // On macOS 15 the selected row is accent-tinted while the sidebar
        // has key focus and the subtle gray otherwise (standard platform
        // behavior, same as Notes/Mail); on macOS 26 the system default is
        // the subtle Liquid Glass highlight the design frames show. Don't
        // replace this with custom highlight drawing — it costs arrow-key
        // navigation and diverges from platform conventions.
        List(selection: $selection) {
            Label("Library", systemImage: "music.note.square.stack")
                .tag(SidebarDestination.library)

            if let device = sidebarDevice {
                Section("Devices") {
                    deviceRow(device)
                }
            }

            Section {
                ForEach(model.playlists, id: \.slug) { summary in
                    playlistRow(summary)
                }
            } header: {
                HStack {
                    Text("Playlists")
                    Spacer()
                    Button(action: createPlaylist) {
                        Image(systemName: "plus")
                    }
                    .buttonStyle(.plain)
                    .help("New Playlist")
                    // Section headers extend closer to the column edge than
                    // row content does, so a Spacer-pushed accessory hugs
                    // the scrollbar gutter. This aligns the "+" with the
                    // rows' trailing content margin (design: uniform inset).
                    // Tune in the preview canvas if the metrics shift.
                    .padding(.trailing, 10)
                    // Review finding #2: without this guard, two quick taps
                    // send two `save_playlist` commands — the daemon's
                    // unique_slug disambiguates the second to
                    // "new-playlist-2", which is silently orphaned (never
                    // selected, easy to lose track of). Disabled for the
                    // whole time a creation is in flight.
                    .disabled(priorSlugsAwaitingNewPlaylist != nil)
                }
            }

            Section("History") {
                Label("Sync History", systemImage: "clock.arrow.circlepath")
                    .tag(SidebarDestination.history)
            }
        }
        .navigationSplitViewColumnWidth(min: 200, ideal: 210, max: 260)
        // Watches the revision counter (not `model.playlists` directly) so
        // this fires on EVERY `playlists_update` reply while a creation is
        // pending — including one that's content-identical to the prior
        // list (e.g. the daemon's error path) — otherwise a plain
        // `onChange(of: playlists)` wouldn't fire and the "+" button would
        // wedge disabled forever (review finding #2).
        //
        // Fix (premature-clear regression): `priorSlugsAwaitingNewPlaylist`
        // must NOT clear on the first bump regardless of match — an
        // unrelated interleaved update (another connected client's own
        // change) would otherwise drop the pending snapshot before this
        // client's own creation reply arrives, so the new playlist gets
        // created but never auto-selected. Clear only on a match, or once
        // `SidebarDestination.shouldClearPendingNewPlaylist`'s bound is
        // exceeded (wedge-forever guard).
        .onChange(of: model.playlistsUpdateRevision) { _, _ in
            guard let priorSlugs = priorSlugsAwaitingNewPlaylist else { return }
            let destination = SidebarDestination.destinationForNewlyCreatedPlaylist(
                priorSlugs: priorSlugs, updated: model.playlists)
            if let destination {
                selection = destination
            }
            newPlaylistRevisionsElapsed += 1
            if SidebarDestination.shouldClearPendingNewPlaylist(
                matched: destination != nil, revisionsElapsed: newPlaylistRevisionsElapsed)
            {
                priorSlugsAwaitingNewPlaylist = nil
                newPlaylistRevisionsElapsed = 0
            }
        }
    }

    private var isSyncing: Bool {
        if case .syncing = model.phase { return true }
        return false
    }

    private func createPlaylist() {
        priorSlugsAwaitingNewPlaylist = Set(model.playlists.map(\.slug))
        newPlaylistRevisionsElapsed = 0
        onSavePlaylist(.manual(slug: nil, name: SidebarDestination.newPlaylistDefaultName, tracks: []))
    }

    /// The one paired device's sidebar identity, derived from either the
    /// live `device_connected` state (when plugged in) or the persisted
    /// `config.ipod` identity (when disconnected but still paired) — the
    /// latter is what lets the row keep showing, dimmed, per the
    /// disconnected-device behavior in the plan's Global Constraints.
    private var sidebarDevice: (serial: String, name: String, isConnected: Bool)? {
        if let device = model.device {
            return (device.serial, device.name ?? device.model, true)
        }
        if let ipod = model.config?.ipod {
            return (ipod.serial, ipod.name ?? ipod.modelLabel, false)
        }
        return nil
    }

    @ViewBuilder
    private func deviceRow(_ device: (serial: String, name: String, isConnected: Bool)) -> some View {
        let expandedBinding = Binding<Bool>(
            get: { deviceManuallyExpanded ?? device.isConnected },
            set: { deviceManuallyExpanded = $0 }
        )
        DisclosureGroup(isExpanded: expandedBinding) {
            Label("Music", systemImage: "music.note")
                .tag(SidebarDestination.device(serial: device.serial, page: .music))
            Label("Settings", systemImage: "gear")
                .tag(SidebarDestination.device(serial: device.serial, page: .settings))
        } label: {
            // Review finding #4: this used to be a Button wrapping the whole
            // label (including the eject Button below), which nests a Button
            // inside another Button's label — undefined hit-testing for a
            // destructive control. `.onTapGesture` + `.contentShape` gets the
            // same tap-consuming effect (taps on the row select the Music
            // page rather than falling through to DisclosureGroup's own
            // label-tap-to-toggle gesture; the disclosure triangle
            // DisclosureGroup renders outside this closure is unaffected and
            // remains the only way to expand/collapse — see the Global
            // Constraints rule this implements) while leaving the eject
            // Button as a real, non-nested, top-level control.
            HStack {
                Image(systemName: "ipod")
                Text(device.name)
                Spacer()
                if device.isConnected {
                    Button(action: onEjectIpod) {
                        Image(systemName: "eject")
                    }
                    .buttonStyle(.plain)
                    .help("Eject")
                    // Unmount would fail (volume busy) mid-sync anyway —
                    // the daemon's subprocess holds the iTunesDB open.
                    .disabled(isSyncing)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                selection = SidebarDestination.destinationForDeviceRowClick(serial: device.serial)
            }
        }
        .foregroundStyle(device.isConnected ? .primary : .secondary)
    }

    private func playlistRow(_ summary: PlaylistSummary) -> some View {
        HStack {
            Label(summary.name, systemImage: "music.note.list")
            if summary.error != nil {
                Spacer()
                Image(systemName: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
            }
        }
        .tag(SidebarDestination.playlist(slug: summary.slug))
    }
}

#if DEBUG
/// `Sidebar.selection` is a `Binding`, which `#Preview` can't hand in
/// directly — this host owns the `@State` the binding needs.
private struct SidebarPreviewHost: View {
    var model: AppModel
    @State private var selection: SidebarDestination?

    var body: some View {
        NavigationSplitView {
            Sidebar(model: model, selection: $selection, onEjectIpod: {}, onSavePlaylist: { _ in })
        } detail: {
            Text("Detail").foregroundStyle(.secondary)
        }
        .frame(width: 500, height: 480)
    }
}

#Preview("Populated") {
    SidebarPreviewHost(model: PreviewFixtures.connectedSyncedModel())
}

#Preview("Device disconnected") {
    SidebarPreviewHost(model: PreviewFixtures.disconnectedModel())
}

#Preview("Playlist error badge") {
    SidebarPreviewHost(model: PreviewFixtures.noDeviceModel())
}
#endif
