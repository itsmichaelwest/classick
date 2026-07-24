import SwiftUI

/// The device Music page (Task 5, Figma frame `3:3773`) — THE canonical
/// surface where sync intent is displayed and edited (Global Constraints:
/// "sync intent is displayed/edited ONLY on device pages"; the Library page
/// carries zero checkbox affordances). A mode `Picker` (Entire library /
/// Selected items / All except selected) drives whether the shared
/// `LibraryBrowser` renders read-only (`.browse`) or with checkboxes
/// (`.select`, `.cascading` style — a device selection is meant to
/// auto-follow future library growth, same intuition the retired
/// `SelectionDraft`-based Library page had). The Playlists facet swaps the
/// browser for a subscriptions checklist. Capacity display lives in the
/// app-wide floating `DeviceRow` (one bar, not two stacked bottom strips) —
/// this page's debounced edits still drive it via `preview_device`, whose
/// reply reaches `DeviceRow` through `deviceConfigs[serial].preview`.
///
/// Edits a local draft and auto-saves it (debounced), mirroring the retired
/// LibraryView's `SelectionDraft` pattern: seed once from the daemon's
/// reply, never re-seed after the user starts editing (so a late/echoed
/// `device_config` can't clobber an in-progress edit).
struct DeviceMusicPage: View {
  var model: AppModel
  var serial: DeviceID
  var onLoadDeviceConfig: (DeviceID) -> Void
  var onSaveAndPreviewDeviceConfig:
    (_ serial: DeviceID, _ selection: SelectionState?, _ subscriptions: SubscriptionsWire?) ->
      DeviceMusicMutationReceipt?
  // Required (no no-op default) — see `MainWindow`'s doc comment on
  // `onSavePlaylist` for why a defaulted closure here would be exactly
  // how this action could ship silently dead.
  var onScan: () -> Void
  var onSubmitLibraryDrop: @MainActor @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void =
    {
      _, _, _ in
    }

  private struct MusicDraft: Equatable {
    var mode: SelectionMode = .all
    var checked: Set<SelectionKey> = []
    var subscriptions: Set<String> = []
  }

  private struct SelectionDraft: Equatable {
    var mode: SelectionMode = .all
    var checked: Set<SelectionKey> = []
  }

  @State private var draft = MusicDraft()
  /// Per-mode memory of checked sets, page-lifetime only: flipping modes
  /// stashes the departing mode's checks and restores the target's, so
  /// Selected → Entire → Selected round-trips the user's original picks
  /// instead of re-seeding everything. First-ever entry to a mode (no
  /// memory) keeps the zero-diff seeding behavior.
  @State private var rememberedRules: [SelectionMode: Set<SelectionKey>] = [:]
  @State private var hasCanonicalDraft = false
  @State private var facet: LibraryBrowser.Facet = .artists
  @State private var expandedDisclosures: Set<LibraryBrowser.DisclosureKey> = []
  @State private var saveTask: Task<Void, Never>?

  private var deviceState: DeviceViewState? {
    DeviceSurfaceLogic.state(serial: serial, in: model.devices)
  }
  private var config: DeviceConfigState? {
    model.editableDeviceConfig(for: serial) ?? deviceState?.config
  }
  private var configStatus: DeviceConfigComponentStatus {
    DeviceConfigStatusLogic.mostImportant([
      model.deviceConfigStatus(for: serial, component: .selection),
      model.deviceConfigStatus(for: serial, component: .subscriptions),
    ])
  }
  private var canEditDevice: Bool { model.canSendDeviceCommand(to: serial) }
  private var isConnected: Bool { deviceState?.connected == true }
  private var surfacePhase: Phase {
    DeviceSurfaceLogic.phase(for: deviceState, globalPhase: model.phase)
  }
  private var deviceName: String {
    guard let deviceState else { return "iPod" }
    return DeviceIdentityLogic.title(identity: deviceState.identity, hardware: deviceState.hardware)
  }
  private var readinessGuidance: DeviceReadinessGuidance? {
    deviceState.flatMap { DeviceReadinessLogic.guidance(for: $0.readiness) }
  }

  var body: some View {
    seededContent
      .hardTopScrollEdge()
      // Title/subtitle/controls live in the WINDOW chrome, not the page
      // body: `navigationTitle` + the macOS-only `navigationSubtitle` put
      // the device name and last-synced line in the titlebar, the
      // centered `.principal` slot holds the facet picker (same slot the
      // Library page uses, so it doesn't move between pages), and
      // `.primaryAction` holds the Sync mode picker. This page is the
      // NavigationSplitView detail, so these land in the unified toolbar;
      // on macOS 26+ the items pick up the glass capsule treatment
      // automatically.
      //
      // There is deliberately NO Sync Now here. The app-wide device bar
      // at the bottom of the window shows Sync Now for the selected
      // device — which, on this page, is always this device — so a
      // toolbar copy was the same action twice, ~120pt apart, with two
      // different disabled rules. The bar's copy wins: it is the one
      // that's present on every page.
      .navigationTitle(deviceName)
      .navigationSubtitle(lastSyncedSubtitle)
      .toolbar {
        if readinessGuidance == nil {
          ToolbarItem(placement: .principal) {
            FacetPicker(facet: $facet, facets: LibraryBrowser.Facet.allCases)
          }
          ToolbarItem(placement: .primaryAction) {
            Picker(selection: Binding(get: { draft.mode }, set: setMode)) {
              Text("Entire library").tag(SelectionMode.all)
              Text("Selected items").tag(SelectionMode.include)
              Text("All except selected").tag(SelectionMode.exclude)
            } label: {
              Text("Sync")
            }
            .pickerStyle(.menu)
            .help("What syncs to this iPod")
            // Disabled until the persisted config seeds the draft
            // (sweep finding #1): before that this picker shows the
            // compiled-in `.all` default — and worse, touching it in
            // that window ran the seeding fn against an EMPTY draft,
            // latched `userEdited`, blocked the real config from ever
            // seeding, and debounced-saved the wrong selection over
            // the persisted one.
            .disabled(!hasCanonicalDraft || !canEditDevice)
          }
        }
      }
      // `.task(id:)` covers a config already cached from a prior visit
      // this launch (seed fires immediately); the `.onChange`s below cover
      // the reply arriving after this view appears. Mirrors the retired
      // LibraryView's onAppear+onChange dual-coverage comment.
      .task(id: serial) {
        guard canEditDevice else { return }
        seedIfNeeded()
        onLoadDeviceConfig(serial)
        submitPendingChanges()
      }
      .onChange(of: canEditDevice) { _, isAvailable in
        handleDeviceAvailabilityChange(isAvailable)
      }
      .onChange(of: config?.selection) { _, _ in seedIfNeeded() }
      .onChange(of: config?.subscriptions) { _, _ in seedIfNeeded() }
      .onChange(of: deviceState?.selectionRevision) { _, _ in seedIfNeeded() }
      .onChange(of: deviceState?.subscriptionsRevision) { _, _ in seedIfNeeded() }
      .onChange(of: model.playlistRevision) { _, _ in scrubDeletedSubscriptions() }
      // Belt-and-suspenders alongside request-generation correlation:
      // cancels an in-flight debounce the instant this page is
      // navigated away from. Reconnects use the same cancellation path
      // through `handleDeviceAvailabilityChange`.
      .onDisappear { submitPendingChanges() }
      .libraryDropDestination(
        target: libraryDropTarget,
        launchNonce: model.libraryDragLaunchNonce,
        feedback: libraryDropFeedback,
        submit: onSubmitLibraryDrop)
  }

  private var libraryDropTarget: LibraryDropTarget? {
    deviceState.flatMap(LibraryDropEligibility.targetForDevice)
  }

  private var libraryDropFeedback: String? {
    guard let target = libraryDropTarget,
      LibraryDropFeedback.belongs(model.dropOutcome, to: target)
    else { return nil }
    return model.dropOutcome?.accessibleMessage
  }

  // (Title/subtitle, the facet picker and the Sync mode picker are all
  // toolbar chrome — see body. The search field and the caption line were
  // removed per design; the caption copy survives in
  // `DeviceMusicLogic.caption` + its tests should a home for it return.)

  private var lastSyncedSubtitle: String {
    // Shown for the KNOWN (paired) device even while disconnected —
    // the last sync is a fact on disk, not a live connection property.
    // Only a page for some OTHER device gets the placeholder.
    guard let last = model.latestSuccessfulSync(for: serial) else { return "Never synced" }
    return "Last synced \(shortDate(last.timestamp))"
  }

  private func shortDate(_ iso: String) -> String {
    guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
    return d.formatted(date: .abbreviated, time: .shortened)
  }

  // MARK: - Content: browser (browse/select) or subscriptions checklist

  /// Renders the real content only once the persisted device config has
  /// seeded the draft (sweep finding #1 — the flash class): before that,
  /// `draft.mode` is the compiled-in `.all` default, so the page showed a
  /// read-only browse list that snapped into checkbox mode a beat later.
  @ViewBuilder
  private var seededContent: some View {
    if let readinessGuidance {
      DeviceReadinessView(guidance: readinessGuidance)
    } else if hasCanonicalDraft {
      VStack(spacing: 0) {
        DeviceConfigStatusView(status: configStatus)
        content
      }
    } else {
      LibraryStateView.loading()
    }
  }

  @ViewBuilder
  private var content: some View {
    if facet == .playlists {
      subscriptionsChecklist
    } else {
      switch DeviceMusicLogic.contentState(
        library: model.library, phase: surfacePhase, configuredSource: model.config?.source,
        mode: draft.mode, isConnected: isConnected, syncedCount: deviceState?.syncedCount ?? 0)
      {
      case .needsScan:
        LibraryStateView.needsScan(onScan: onScan)
      case .scanning(let current, let total):
        LibraryStateView.scanning(current: current, total: total)
      case .libraryEmpty(let path):
        LibraryStateView.libraryEmpty(path: path, onScan: onScan)
      case .deviceEmpty:
        // Only reachable in Entire-library mode (see
        // `DeviceMusicLogic.contentState`) — in Selected/Except modes the
        // browser IS the primary interactive UI whether or not a first
        // sync has happened, so it must keep rendering there.
        LibraryStateView.deviceEmpty
      case .browser:
        if let library = model.library {
          LibraryBrowser(
            library: library, facet: facet, mode: browserMode, search: "",
            projectedProfile: config?.settings.transcodeProfile,
            expandedDisclosures: $expandedDisclosures)
        }
      }
    }
  }

  /// `.browse` (read-only) in Entire-library mode; `.select` bound to the
  /// draft's checked set otherwise. `.cascading` style so a checked artist
  /// also covers future albums — see this view's doc comment.
  private var browserMode: LibraryBrowser.Mode {
    guard canEditDevice, draft.mode != .all else { return .browse }
    return .select(
      checked: Binding(
        get: { draft.checked }, set: { value in editSelection { $0.checked = value } }),
      style: .cascading)
  }

  /// Same table pattern as the Artists/Albums/Genres facets: checkbox +
  /// name leading, right-aligned count/size columns (identical 84pt
  /// minimum widths so the columns rule up across facet switches).
  private var subscriptionsChecklist: some View {
    List {
      if model.playlists.isEmpty {
        Text("No playlists yet — create one from the sidebar.")
          .foregroundStyle(.secondary)
      }
      ForEach(model.playlists, id: \.slug) { playlist in
        HStack(spacing: 8) {
          Toggle(
            isOn: Binding(
              get: { draft.subscriptions.contains(playlist.slug) },
              set: { on in
                editSubscriptions {
                  if on { $0.insert(playlist.slug) } else { $0.remove(playlist.slug) }
                }
              }
            )
          ) { EmptyView() }
          .toggleStyle(.checkbox)
          .labelsHidden()
          .disabled(!canEditDevice)
          Text(playlist.name)
            .lineLimit(1)
            .truncationMode(.tail)
          if let error = playlist.error {
            Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange).help(error)
          }
          Spacer(minLength: 12)
          Text("\(playlist.tracks) track\(playlist.tracks == 1 ? "" : "s")")
            .foregroundStyle(.secondary)
            .monospacedDigit()
            .frame(minWidth: 84, alignment: .trailing)
          Text(
            formatBytes(
              DeviceMusicLogic.projectedBytes(
                sourceBytes: playlist.bytes, durationMS: playlist.durationMS,
                profile: config?.settings.transcodeProfile ?? .alac)))
            .foregroundStyle(.secondary)
            .monospacedDigit()
            .frame(minWidth: 84, alignment: .trailing)
        }
      }
      if let line = DeviceMusicLogic.unresolvedSubscriptionsLine(
        config?.preview?.unresolvedSubscriptions)
      {
        Text(line).font(.caption).foregroundStyle(.orange)
      }
    }
    .listStyle(.inset)
    .environment(\.defaultMinListRowHeight, LibraryBrowser.rowHeight)
  }

  // MARK: - Draft seeding + mode switch + debounced save

  /// Reconciles persisted selection and subscriptions independently because
  /// their daemon revisions advance separately.
  private func seedIfNeeded() {
    guard let config else { return }
    draft = MusicDraft(
      mode: config.selection.mode,
      checked: Set(config.selection.rules),
      subscriptions: Set(config.subscriptions.playlists))
    hasCanonicalDraft = true
  }

  /// The mode `Picker`'s edit path: stash the departing mode's checks in
  /// `rememberedRules`, then recompute via the trust-critical seeding
  /// function — which restores the target mode's remembered checks when
  /// it has any, and only zero-diff-seeds on first entry.
  private func setMode(_ newMode: SelectionMode) {
    let previousMode = draft.mode
    rememberedRules[previousMode] = draft.checked
    let seeded = DeviceMusicLogic.seededSelection(
      fromDeviceContents: model.library?.artists ?? [],
      previousMode: previousMode, newMode: newMode,
      current: Array(draft.checked),
      remembered: rememberedRules[newMode].map(Array.init))
    editSelection {
      $0.mode = newMode
      $0.checked = Set(seeded)
    }
  }

  /// Debounced auto-save: every selection/subscription/mode edit sends
  /// `save_device_config` (settings untouched — that's the device Settings
  /// page, Task 6) followed by a fresh `preview_device` so the capacity
  /// bar tracks the edit. Mirrors `DeviceView.scheduleSave`/the retired
  /// LibraryView's `scheduleSave` (400ms).
  private func scheduleSave() {
    saveTask?.cancel()
    saveTask = Task {
      guard
        await DeviceDraftSaveGate.waitUntilReady(
          serial: serial, model: model)
      else { return }
      submitPendingChanges()
    }
  }

  private func submitPendingChanges() {
    saveTask?.cancel()
    guard canEditDevice else { return }
    let selection = model.pendingDeviceSelection(for: serial)
    let subscriptions = model.pendingDeviceSubscriptions(for: serial)
    guard selection != nil || subscriptions != nil,
      let receipt = onSaveAndPreviewDeviceConfig(serial, selection, subscriptions)
    else { return }
    if let selectionReceipt = receipt.selection {
      model.markDeviceSelectionSubmitted(for: serial, receipt: selectionReceipt)
    }
    if let subscriptionsReceipt = receipt.subscriptions {
      model.markDeviceSubscriptionsSubmitted(for: serial, receipt: subscriptionsReceipt)
    }
  }

  private func editSelection(_ mutation: (inout SelectionDraft) -> Void) {
    guard hasCanonicalDraft, canEditDevice else { return }
    var edited = SelectionDraft(mode: draft.mode, checked: draft.checked)
    mutation(&edited)
    guard edited.mode != draft.mode || edited.checked != draft.checked else { return }
    draft.mode = edited.mode
    draft.checked = edited.checked
    model.editDeviceSelection(
      SelectionState(mode: edited.mode, rules: Array(edited.checked)), for: serial)
    scheduleSave()
  }

  private func editSubscriptions(_ mutation: (inout Set<String>) -> Void) {
    guard hasCanonicalDraft, canEditDevice else { return }
    var edited = draft.subscriptions
    mutation(&edited)
    guard edited != draft.subscriptions else { return }
    draft.subscriptions = edited
    model.editDeviceSubscriptions(
      SubscriptionsWire(playlists: Array(edited).sorted()), for: serial)
    scheduleSave()
  }

  private func scrubDeletedSubscriptions() {
    guard hasCanonicalDraft else { return }
    let validSlugs = Set(model.playlists.map(\.slug))
    let scrubbed = DeviceMusicLogic.scrubbedSubscriptions(
      draft.subscriptions, validSlugs: validSlugs)
    guard scrubbed != draft.subscriptions else { return }
    editSubscriptions { $0 = scrubbed }
  }

  private func handleDeviceAvailabilityChange(_ isAvailable: Bool) {
    guard isAvailable else {
      submitPendingChanges()
      return
    }
    seedIfNeeded()
    onLoadDeviceConfig(serial)
    submitPendingChanges()
  }
}

@MainActor
enum DeviceDraftSaveGate {
  static func waitUntilReady(
    delay: Duration = .milliseconds(400),
    serial: DeviceID,
    model: AppModel
  ) async -> Bool {
    try? await Task.sleep(for: delay)
    return !Task.isCancelled && model.canSendDeviceCommand(to: serial)
  }
}

#if DEBUG
  /// Wrapped in a `NavigationStack` so the titlebar chrome this page declares
  /// (`navigationTitle`/`navigationSubtitle`/`.toolbar`) actually renders in
  /// the preview canvas — a bare view preview has no navigation context and
  /// would silently drop all three.
  @MainActor
  private func musicPagePreview(_ model: AppModel) -> some View {
    NavigationStack {
      DeviceMusicPage(
        model: model, serial: try! DeviceID(PreviewFixtures.pairedIpod.serial),
        onLoadDeviceConfig: { _ in },
        onSaveAndPreviewDeviceConfig: { _, _, _ in .init() }, onScan: {})
    }
    .frame(width: 760, height: 560)
  }

  #Preview("Entire library") {
    musicPagePreview(PreviewFixtures.connectedSyncedModel())
  }

  #Preview("Selected items") {
    musicPagePreview(PreviewFixtures.connectedSelectedItemsModel())
  }

  #Preview("Disconnected") {
    musicPagePreview(PreviewFixtures.disconnectedModel())
  }

  #Preview("Nothing synced") {
    musicPagePreview(PreviewFixtures.connectedNothingSyncedModel())
  }

  #Preview("Over-full preview") {
    musicPagePreview(PreviewFixtures.connectedOverfullModel())
  }
#endif
