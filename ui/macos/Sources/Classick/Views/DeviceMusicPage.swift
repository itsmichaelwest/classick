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
/// `device_config_update` can't clobber an in-progress edit).
struct DeviceMusicPage: View {
  var model: AppModel
  var serial: String
  var onSyncNow: (DeviceSerial) -> Void
  var onLoadDeviceConfig: (String) -> Void
  var onSaveAndPreviewDeviceConfig:
    (_ serial: String, _ selection: SelectionState?, _ subscriptions: SubscriptionsWire?) -> String?
  // Required (no no-op default) — see `MainWindow`'s doc comment on
  // `onSavePlaylist` for why a defaulted closure here would be exactly
  // how this action could ship silently dead.
  var onScan: () -> Void
  var onSubmitLibraryDrop: @MainActor @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void = {
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
  @State private var acknowledgedSelection = AcknowledgedDraft(
    canonical: SelectionDraft(), revision: 0)
  @State private var acknowledgedSubscriptions = AcknowledgedDraft(
    canonical: Set<String>(), revision: 0)
  /// Per-mode memory of checked sets, page-lifetime only: flipping modes
  /// stashes the departing mode's checks and restores the target's, so
  /// Selected → Entire → Selected round-trips the user's original picks
  /// instead of re-seeding everything. First-ever entry to a mode (no
  /// memory) keeps the zero-diff seeding behavior.
  @State private var rememberedRules: [SelectionMode: Set<SelectionKey>] = [:]
  @State private var hasCanonicalDraft = false
  @State private var facet: LibraryBrowser.Facet = .artists
  @State private var saveTask: Task<Void, Never>?

  private var deviceState: DeviceViewState? {
    DeviceSurfaceLogic.state(serial: serial, in: model.devices)
  }
  private var config: DeviceConfigState? { deviceState?.config }
  private var canEditDevice: Bool { model.canSendDeviceCommand(to: serial) }
  private var isConnected: Bool { deviceState?.connected == true }
  private var surfacePhase: Phase {
    DeviceSurfaceLogic.phase(for: deviceState, globalPhase: model.phase)
  }
  private var deviceName: String {
    deviceState?.identity.name ?? deviceState?.identity.modelLabel ?? serial
  }

  var body: some View {
    seededContent
      .facetBarBelowToolbar { facetBar }
      // Title/subtitle/actions live in the WINDOW chrome, not the page
      // body: `navigationTitle` + the macOS-only `navigationSubtitle` put
      // the device name and last-synced line in the titlebar, and the
      // `.primaryAction` toolbar group puts the Sync mode picker + Sync
      // Now in the trailing corner (design frame 3:3773). This page is
      // the NavigationSplitView detail, so these land in the unified
      // toolbar; on macOS 26+ the items pick up the glass capsule
      // treatment automatically.
      .navigationTitle(deviceName)
      .navigationSubtitle(lastSyncedSubtitle)
      .toolbar {
        ToolbarItemGroup(placement: .primaryAction) {
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
          // HIDDEN (not disabled) while disconnected: a permanently
          // washed-out prominent capsule reads as broken chrome, and
          // the bottom device bar already explains "<name> not
          // connected". Disabled is reserved for transient busy
          // states (sync/scan in flight) where the button will
          // shortly work again.
          if isConnected {
            Button("Sync Now") { onSyncNow(serial) }
              .buttonStyle(.borderedProminent)
              .disabled(
                DeviceMusicLogic.isSyncNowDisabled(phase: surfacePhase, isConnected: isConnected))
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
      .onDisappear { saveTask?.cancel() }
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

  // MARK: - Facet bar (below the toolbar, scroll-edge-aware)
  // (Title/subtitle and the Sync controls are toolbar chrome — see body.
  // The search field and the caption line were removed per design; the
  // caption copy survives in `DeviceMusicLogic.caption` + its tests
  // should a home for it return.)

  private var facetBar: some View {
    Picker("", selection: $facet) {
      ForEach(LibraryBrowser.Facet.allCases, id: \.self) { Text($0.rawValue).tag($0) }
    }
    .pickerStyle(.segmented)
    .frame(width: 320)
    .padding(.vertical, 14)
    .frame(maxWidth: .infinity)
  }

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
    if hasCanonicalDraft {
      content
    } else {
      VStack(spacing: 8) {
        Spacer()
        ProgressView().controlSize(.small)
        Text("Loading…").foregroundStyle(.secondary)
        Spacer()
      }
      .frame(maxWidth: .infinity)
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
        needsScanState
      case .scanning(let current, let total):
        scanningState(current: current, total: total)
      case .libraryEmpty(let path):
        libraryEmptyState(path: path)
      case .deviceEmpty:
        deviceEmptyState
      case .browser:
        if let library = model.library {
          LibraryBrowser(library: library, facet: facet, mode: browserMode, search: "")
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
      checked: Binding(get: { draft.checked }, set: { value in editSelection { $0.checked = value } }),
      style: .cascading)
  }

  private var needsScanState: some View {
    VStack(spacing: 12) {
      Spacer()
      Text("Classick needs to read your library's tags once").font(.headline)
      Button("Scan Library", action: onScan)
        .keyboardShortcut(.defaultAction)
      Spacer()
    }
    .frame(maxWidth: .infinity)
  }

  private func scanningState(current: Int, total: Int) -> some View {
    VStack(spacing: 12) {
      Spacer()
      ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
        .frame(maxWidth: 260)
      Text("Scanning… \(current) of \(total)").font(.caption).foregroundStyle(.secondary)
      Spacer()
    }
    .frame(maxWidth: .infinity)
  }

  /// Global Constraints: "library empty → 'No audio files found in
  /// <path>'". Shared copy/behavior with `LibraryView`'s equivalent state
  /// via `LibraryContentLogic`.
  private func libraryEmptyState(path: String) -> some View {
    VStack(spacing: 12) {
      Spacer()
      Text("No audio files found in \(path)")
        .font(.headline)
        .multilineTextAlignment(.center)
        .padding(.horizontal, 24)
      Button("Rescan Library", action: onScan)
      Spacer()
    }
    .frame(maxWidth: .infinity)
  }

  /// Global Constraints: "device empty → 'Nothing synced yet — press Sync
  /// Now.'". Only reachable in Entire-library mode (see
  /// `DeviceMusicLogic.contentState`) — in Selected/Except modes the
  /// browser IS the primary interactive UI regardless of whether a first
  /// sync has happened yet, so it must keep rendering there. No duplicate
  /// button here: "press Sync Now" refers to the toolbar's existing button.
  private var deviceEmptyState: some View {
    VStack(spacing: 12) {
      Spacer()
      Text("Nothing synced yet — press Sync Now.").font(.headline)
      Spacer()
    }
    .frame(maxWidth: .infinity)
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
          Text(formatBytes(playlist.bytes))
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
    guard let config, let deviceState else { return }
    let acknowledgement = model.deviceConfigAcknowledgedRequestIDs[serial]
    acknowledgedSelection.reconcile(
      canonical: SelectionDraft(
        mode: config.selection.mode, checked: Set(config.selection.rules)),
      revision: deviceState.selectionRevision, acknowledgedRequestID: acknowledgement)
    acknowledgedSubscriptions.reconcile(
      canonical: Set(config.subscriptions.playlists),
      revision: deviceState.subscriptionsRevision, acknowledgedRequestID: acknowledgement)
    copyAcknowledgedValuesToDraft()
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
  private func scheduleSave(_ d: MusicDraft) {
    saveTask?.cancel()
    saveTask = Task {
      guard
        await DeviceDraftSaveGate.waitUntilReady(
          serial: serial, model: model)
      else { return }
      guard let requestID = onSaveAndPreviewDeviceConfig(
        serial,
        SelectionState(mode: d.mode, rules: Array(d.checked)),
        SubscriptionsWire(playlists: Array(d.subscriptions).sorted()))
      else { return }
      acknowledgedSelection.markSubmitted(requestID: requestID)
      acknowledgedSubscriptions.markSubmitted(requestID: requestID)
    }
  }

  private func editSelection(_ mutation: (inout SelectionDraft) -> Void) {
    guard hasCanonicalDraft, canEditDevice else { return }
    var edited = acknowledgedSelection.value
    mutation(&edited)
    guard edited != acknowledgedSelection.value else { return }
    acknowledgedSelection.edit(edited)
    copyAcknowledgedValuesToDraft()
    scheduleSave(draft)
  }

  private func editSubscriptions(_ mutation: (inout Set<String>) -> Void) {
    guard hasCanonicalDraft, canEditDevice else { return }
    var edited = acknowledgedSubscriptions.value
    mutation(&edited)
    guard edited != acknowledgedSubscriptions.value else { return }
    acknowledgedSubscriptions.edit(edited)
    copyAcknowledgedValuesToDraft()
    scheduleSave(draft)
  }

  private func copyAcknowledgedValuesToDraft() {
    draft = MusicDraft(
      mode: acknowledgedSelection.value.mode,
      checked: acknowledgedSelection.value.checked,
      subscriptions: acknowledgedSubscriptions.value)
  }

  private func scrubDeletedSubscriptions() {
    guard hasCanonicalDraft else { return }
    let validSlugs = Set(model.playlists.map(\.slug))
    let scrubbed = DeviceMusicLogic.scrubbedSubscriptions(
      acknowledgedSubscriptions.value, validSlugs: validSlugs)
    guard scrubbed != acknowledgedSubscriptions.value else { return }
    editSubscriptions { $0 = scrubbed }
  }

  private func handleDeviceAvailabilityChange(_ isAvailable: Bool) {
    guard isAvailable else {
      saveTask?.cancel()
      hasCanonicalDraft = false
      rememberedRules.removeAll()
      return
    }
    seedIfNeeded()
    onLoadDeviceConfig(serial)
  }
}

@MainActor
enum DeviceDraftSaveGate {
  static func waitUntilReady(
    delay: Duration = .milliseconds(400),
    serial: DeviceSerial,
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
        model: model, serial: PreviewFixtures.pairedIpod.serial,
        onSyncNow: { _ in }, onLoadDeviceConfig: { _ in },
        onSaveAndPreviewDeviceConfig: { _, _, _ in "preview" }, onScan: {})
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
