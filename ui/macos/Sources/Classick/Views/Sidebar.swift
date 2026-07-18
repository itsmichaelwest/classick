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
    var onForgetIpod: () -> Void
    var onSavePlaylist: (PlaylistPayload) -> Void

    /// Snapshot of playlist slugs taken the moment "+" is tapped, so the
    /// `onChange` below can recognize the newly assigned slug once the
    /// daemon's `playlists_update` reply arrives. `nil` when no creation is
    /// in flight.
    @State private var priorSlugsAwaitingNewPlaylist: Set<String>?

    /// One device row's disclosure state. Only one iPod is ever paired at a
    /// time (see `AppModel.device`/`config.ipod`), so a single optional is
    /// enough: `nil` means "use the default for the current connection
    /// state" (expanded while connected, collapsed while disconnected —
    /// Global Constraints "Disconnected device: … collapsed by default");
    /// once the user manually toggles it, that explicit choice sticks.
    @State private var deviceManuallyExpanded: Bool?

    var body: some View {
        List(selection: $selection) {
            Section("Library") {
                Label("Music Library", systemImage: "music.note.list")
                    .tag(SidebarDestination.library)
            }

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
                }
            }

            Section("History") {
                Label("Sync History", systemImage: "clock.arrow.circlepath")
                    .tag(SidebarDestination.history)
            }
        }
        .navigationSplitViewColumnWidth(min: 200, ideal: 210, max: 260)
        .onChange(of: model.playlists) { _, updated in
            guard let priorSlugs = priorSlugsAwaitingNewPlaylist,
                  let destination = SidebarDestination.destinationForNewlyCreatedPlaylist(
                      priorSlugs: priorSlugs, updated: updated)
            else { return }
            selection = destination
            priorSlugsAwaitingNewPlaylist = nil
        }
    }

    private func createPlaylist() {
        priorSlugsAwaitingNewPlaylist = Set(model.playlists.map(\.slug))
        onSavePlaylist(.manual(slug: nil, name: "New Playlist", tracks: []))
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
                .padding(.leading, 12)
            Label("Settings", systemImage: "gear")
                .tag(SidebarDestination.device(serial: device.serial, page: .settings))
                .padding(.leading, 12)
        } label: {
            // The label content is wrapped in a plain-style Button so taps
            // on it are consumed here (selecting the Music page) rather than
            // falling through to DisclosureGroup's own label-tap-to-toggle
            // gesture; the disclosure triangle DisclosureGroup renders
            // outside this closure is unaffected and remains the only way
            // to expand/collapse — see the Global Constraints rule this
            // implements.
            Button {
                selection = SidebarDestination.destinationForDeviceRowClick(serial: device.serial)
            } label: {
                HStack {
                    Image(systemName: "ipod")
                    Text(device.name)
                    Spacer()
                    if device.isConnected {
                        Button(action: onForgetIpod) {
                            Image(systemName: "eject")
                        }
                        .buttonStyle(.plain)
                        .help("Remove this iPod")
                    }
                }
            }
            .buttonStyle(.plain)
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
