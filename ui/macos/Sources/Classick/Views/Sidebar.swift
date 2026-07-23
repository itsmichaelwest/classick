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
  var onEjectIpod: (DeviceID) -> Void
  var onSavePlaylist: (PlaylistPayload) -> Void
  var onSubmitLibraryDrop: @MainActor @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void =
    {
      _, _, _ in
    }

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

  /// Explicit disclosure choices are keyed by serial so adding or removing
  /// another iPod cannot transfer one row's expansion state to another.
  @State private var manuallyExpanded: [DeviceID: Bool] = [:]

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

      if !sidebarDevices.isEmpty {
        Section("Devices") {
          ForEach(sidebarDevices) { device in
            deviceRow(device)
          }
        }
      }

      if !model.unidentifiedDevices.isEmpty {
        Section("Detected iPods") {
          ForEach(model.unidentifiedDevices.values.sorted { $0.observationID < $1.observationID }, id: \.observationID) { _ in
            let guidance = DeviceReadinessLogic.identityUnavailableGuidance
            HStack(alignment: .top, spacing: 8) {
              Image(systemName: guidance.systemImage)
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
              VStack(alignment: .leading, spacing: 2) {
                Text(guidance.title)
                Text(guidance.message)
                  .font(.caption)
                  .foregroundStyle(.secondary)
                  .lineLimit(2)
              }
            }
            .accessibilityElement(children: .combine)
          }
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

  private func createPlaylist() {
    priorSlugsAwaitingNewPlaylist = Set(model.playlists.map(\.slug))
    newPlaylistRevisionsElapsed = 0
    onSavePlaylist(.manual(slug: nil, name: SidebarDestination.newPlaylistDefaultName, tracks: []))
  }

  private var sidebarDevices: [SidebarDeviceRow] {
    SidebarInventory.rows(from: model.devices)
  }

  @ViewBuilder
  private func deviceRow(_ device: SidebarDeviceRow) -> some View {
    let expandedBinding = Binding<Bool>(
      get: { manuallyExpanded[device.serial] ?? device.connected },
      set: { manuallyExpanded[device.serial] = $0 }
    )
    DisclosureGroup(isExpanded: expandedBinding) {
      Label("Music", systemImage: "music.note")
        .tag(SidebarDestination.device(serial: device.serial, page: .music))
        .libraryDropDestination(
          target: libraryDropTarget(for: device.serial),
          launchNonce: model.libraryDragLaunchNonce,
          feedback: libraryDropFeedback(for: libraryDropTarget(for: device.serial)),
          submit: onSubmitLibraryDrop)
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
        HStack(spacing: 8) {
          DeviceIcon(hardware: device.hardware, size: 22, serial: device.serial)
          Text(device.name)
            .lineLimit(1)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(device.accessibilityLabel)
        Spacer()
        if device.connected {
          Button {
            onEjectIpod(device.serial)
          } label: {
            Image(systemName: "eject")
          }
          .buttonStyle(.plain)
          .help("Eject")
          // Unmount would fail (volume busy) mid-sync anyway —
          // the daemon's subprocess holds the iTunesDB open.
          .disabled(device.phase == .syncing)
        }
      }
      .contentShape(Rectangle())
      .onTapGesture {
        selection = SidebarDestination.destinationForDeviceRowClick(serial: device.serial)
      }
      .libraryDropDestination(
        target: libraryDropTarget(for: device.serial),
        launchNonce: model.libraryDragLaunchNonce,
        feedback: libraryDropFeedback(for: libraryDropTarget(for: device.serial)),
        submit: onSubmitLibraryDrop)
    }
    .foregroundStyle(device.connected ? .primary : .secondary)
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
    .libraryDropDestination(
      target: LibraryDropEligibility.targetForPlaylist(summary),
      launchNonce: model.libraryDragLaunchNonce,
      feedback: libraryDropFeedback(for: LibraryDropEligibility.targetForPlaylist(summary)),
      submit: onSubmitLibraryDrop)
  }

  private func libraryDropTarget(for serial: DeviceID) -> LibraryDropTarget? {
    model.devices[serial].flatMap(LibraryDropEligibility.targetForDevice)
  }

  private func libraryDropFeedback(for target: LibraryDropTarget?) -> String? {
    guard let target, LibraryDropFeedback.belongs(model.dropOutcome, to: target) else { return nil }
    return model.dropOutcome?.accessibleMessage
  }
}

struct SidebarDeviceRow: Identifiable, Equatable {
  var serial: DeviceID
  var name: String
  var connected: Bool
  var configured: Bool
  var phase: DevicePhase
  var hardware: WireV3Hardware
  var accessibilityLabel: String

  var id: DeviceID { serial }
}

enum SidebarInventory {
  static func rows(from devices: [DeviceID: DeviceViewState]) -> [SidebarDeviceRow] {
    devices.values
      .map { device in
        SidebarDeviceRow(
          serial: device.deviceID,
          name: DeviceIdentityLogic.title(identity: device.identity, hardware: device.hardware),
          connected: device.connected,
          configured: device.configured,
          phase: device.phase,
          hardware: device.hardware,
          accessibilityLabel: DeviceIdentityLogic.accessibilityLabel(
            identity: device.identity, hardware: device.hardware))
      }
      .sorted { lhs, rhs in
        let lhsName = lhs.name.lowercased()
        let rhsName = rhs.name.lowercased()
        return lhsName == rhsName
          ? lhs.serial.rawValue < rhs.serial.rawValue
          : lhsName < rhsName
      }
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
        Sidebar(
          model: model, selection: $selection, onEjectIpod: { _ in }, onSavePlaylist: { _ in },
          onSubmitLibraryDrop: { _, _, _ in })
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

  #Preview("Identity unavailable") {
    SidebarPreviewHost(model: PreviewFixtures.unidentifiedDeviceModel())
  }
#endif
