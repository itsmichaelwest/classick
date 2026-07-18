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
/// browser for a subscriptions checklist. A floating capacity bar reflects
/// the live `preview_device` reply.
///
/// Edits a local draft and auto-saves it (debounced), mirroring the retired
/// LibraryView's `SelectionDraft` pattern: seed once from the daemon's
/// reply, never re-seed after the user starts editing (so a late/echoed
/// `device_config_update` can't clobber an in-progress edit).
struct DeviceMusicPage: View {
    var model: AppModel
    var serial: String
    var onSyncNow: () -> Void
    var onLoadDeviceConfig: (String) -> Void
    var onPreviewDevice: (String) -> Void
    var onSaveDeviceConfig: (_ serial: String, _ selection: SelectionState?, _ subscriptions: SubscriptionsWire?) -> Void
    // Required (no no-op default) — see `MainWindow`'s doc comment on
    // `onSavePlaylist` for why a defaulted closure here would be exactly
    // how this action could ship silently dead.
    var onScan: () -> Void

    private struct MusicDraft: Equatable {
        var mode: SelectionMode = .all
        var checked: Set<SelectionKey> = []
        var subscriptions: Set<String> = []
    }

    @State private var draft = MusicDraft()
    @State private var seededFromModel = false
    @State private var userEdited = false
    @State private var facet: LibraryBrowser.Facet = .artists
    @State private var search = ""
    @State private var saveTask: Task<Void, Never>?

    private var config: DeviceConfigState? { model.deviceConfigs[serial] }
    private var isConnected: Bool { model.device?.serial == serial }
    private var deviceName: String {
        DeviceIdentityLogic.deviceName(serial: serial, isConnected: isConnected, connectedDevice: model.device, pairedIpod: model.config?.ipod)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { capacityBar }
        // `.task(id:)` covers a config already cached from a prior visit
        // this launch (seed fires immediately); the `.onChange`s below cover
        // the reply arriving after this view appears. Mirrors the retired
        // LibraryView's onAppear+onChange dual-coverage comment.
        .task(id: serial) {
            seedIfNeeded()
            onLoadDeviceConfig(serial)
        }
        .onChange(of: config?.selection) { _, _ in seedIfNeeded() }
        .onChange(of: config?.subscriptions) { _, _ in seedIfNeeded() }
        .onChange(of: draft) { _, newDraft in
            // The one-time seed assignment also trips this (draft just
            // changed from its default), but `seededFromModel` already
            // blocks re-seeding by then — the resulting harmless re-save +
            // preview is a cosmetic round-trip, not a clobber.
            userEdited = true
            scheduleSave(newDraft)
        }
        // Belt-and-suspenders alongside the `pendingPreviewSerials` queue
        // fix: cancels an in-flight debounce the instant this page is
        // navigated away from, so a stale `saveTask` firing after teardown
        // is minimized rather than merely tolerated. (A stale
        // `save_device_config` itself is harmless — it's serial-keyed — it
        // was only the paired `previewDevice` request that could
        // misattribute without the queue fix.)
        .onDisappear { saveTask?.cancel() }
    }

    // MARK: - Header: title, Sync Now, mode picker, facet, caption

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(deviceName).font(.title2.bold())
                    Text(lastSyncedSubtitle).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Sync Now", action: onSyncNow)
                    .buttonStyle(.borderedProminent)
                    .disabled(DeviceMusicLogic.isSyncNowDisabled(phase: model.phase, isConnected: isConnected))
            }
            Picker("Sync", selection: Binding(get: { draft.mode }, set: setMode)) {
                Text("Entire library").tag(SelectionMode.all)
                Text("Selected items").tag(SelectionMode.include)
                Text("All except selected").tag(SelectionMode.exclude)
            }
            .pickerStyle(.menu)
            .frame(maxWidth: 260, alignment: .leading)
            HStack {
                Picker("", selection: $facet) {
                    ForEach(LibraryBrowser.Facet.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .frame(width: 320)
                if facet != .playlists {
                    TextField("Search", text: $search)
                        .textFieldStyle(.roundedBorder)
                }
            }
            Text(captionLine).font(.caption).foregroundStyle(.secondary)
        }
        .padding(12)
    }

    private var captionLine: String {
        facet == .playlists
            ? "Subscribed playlists always sync to this iPod, shown on the iPod's Music app."
            : DeviceMusicLogic.caption(mode: draft.mode, isConnected: isConnected)
    }

    private var lastSyncedSubtitle: String {
        // `model.lastSync` is the CONNECTED device's last sync — showing it
        // on a different device's page would misattribute it (finding #2).
        guard isConnected else { return DeviceIdentityLogic.placeholder }
        guard let last = model.lastSync else { return "Never synced" }
        return "Last synced \(shortDate(last.timestamp))"
    }

    private func shortDate(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }

    // MARK: - Content: browser (browse/select) or subscriptions checklist

    @ViewBuilder
    private var content: some View {
        if facet == .playlists {
            subscriptionsChecklist
        } else {
            switch DeviceMusicLogic.contentState(
                library: model.library, phase: model.phase, configuredSource: model.config?.source,
                mode: draft.mode, isConnected: isConnected, syncedCount: model.syncedCount)
            {
            case .needsScan:
                needsScanState
            case let .scanning(current, total):
                scanningState(current: current, total: total)
            case let .libraryEmpty(path):
                libraryEmptyState(path: path)
            case .deviceEmpty:
                deviceEmptyState
            case .browser:
                if let library = model.library {
                    LibraryBrowser(library: library, facet: facet, mode: browserMode, search: search)
                }
            }
        }
    }

    /// `.browse` (read-only) in Entire-library mode; `.select` bound to the
    /// draft's checked set otherwise. `.cascading` style so a checked artist
    /// also covers future albums — see this view's doc comment.
    private var browserMode: LibraryBrowser.Mode {
        guard draft.mode != .all else { return .browse }
        return .select(checked: Binding(get: { draft.checked }, set: { draft.checked = $0 }), style: .cascading)
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
    /// button here: "press Sync Now" refers to the header's existing button.
    private var deviceEmptyState: some View {
        VStack(spacing: 12) {
            Spacer()
            Text("Nothing synced yet — press Sync Now.").font(.headline)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private var subscriptionsChecklist: some View {
        List {
            if model.playlists.isEmpty {
                Text("No playlists yet — create one from the sidebar.")
                    .foregroundStyle(.secondary)
            }
            ForEach(model.playlists, id: \.slug) { playlist in
                Toggle(isOn: Binding(
                    get: { draft.subscriptions.contains(playlist.slug) },
                    set: { on in
                        if on { draft.subscriptions.insert(playlist.slug) }
                        else { draft.subscriptions.remove(playlist.slug) }
                    }
                )) {
                    HStack {
                        Text(playlist.name)
                        if let error = playlist.error {
                            Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange).help(error)
                        }
                        Spacer()
                        Text("\(playlist.tracks) track\(playlist.tracks == 1 ? "" : "s") · \(formatBytes(playlist.bytes))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                .toggleStyle(.checkbox)
            }
            if let line = DeviceMusicLogic.unresolvedSubscriptionsLine(config?.preview?.unresolvedSubscriptions) {
                Text(line).font(.caption).foregroundStyle(.orange)
            }
        }
        .listStyle(.inset)
    }

    // MARK: - Floating capacity bar

    @ViewBuilder
    private var capacityBar: some View {
        // `model.deviceStorage` is the CONNECTED device's live capacity
        // reading — on a different device's page it describes the wrong
        // iPod entirely, so the bar simply doesn't render there (finding #2;
        // the floating bar has no natural place for a text placeholder, so
        // "omit" is the right call per the review, unlike the text fields
        // above which use `DeviceIdentityLogic.placeholder`).
        if isConnected, let bar = DeviceMusicLogic.capacityBar(storage: model.deviceStorage, preview: config?.preview) {
            VStack(alignment: .leading, spacing: 4) {
                GeometryReader { proxy in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 3).fill(.quaternary)
                        RoundedRectangle(cornerRadius: 3).fill(.orange.opacity(0.55))
                            .frame(width: proxy.size.width * bar.projectedFraction)
                        RoundedRectangle(cornerRadius: 3).fill(Color.accentColor)
                            .frame(width: proxy.size.width * bar.usedFraction)
                    }
                }
                .frame(height: 6)
                HStack {
                    Text(DeviceMusicLogic.capacitySummary(bar))
                    Spacer()
                    if let line = DeviceRowFormatting.skippedForSpaceLine(syncedSummary: syncedSummary, skipped: model.lastRunSkippedForSpace) {
                        Text(line)
                    }
                }
                .font(.caption).foregroundStyle(.secondary)
                if let artLine = DeviceRowFormatting.artworkMissingLine(model.lastRunArtwork) {
                    Text(artLine).font(.caption).foregroundStyle(.orange)
                }
            }
            .padding(10)
            .background(.bar)
            .overlay(alignment: .top) { Divider() }
        }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
    }

    // MARK: - Draft seeding + mode switch + debounced save

    /// Seeds the local draft from the persisted device config exactly once,
    /// and never after the user has started editing.
    private func seedIfNeeded() {
        guard !seededFromModel, !userEdited, let config else { return }
        draft = MusicDraft(
            mode: config.selection.mode,
            checked: Set(config.selection.rules),
            subscriptions: Set(config.subscriptions.playlists))
        seededFromModel = true
    }

    /// The mode `Picker`'s edit path: recompute the checked set via the
    /// trust-critical seeding function (see `DeviceMusicLogic.seededSelection`)
    /// so an Entire->Selected switch is zero-diff, then apply the new mode.
    private func setMode(_ newMode: SelectionMode) {
        let seeded = DeviceMusicLogic.seededSelection(
            fromDeviceContents: model.library?.artists ?? [],
            previousMode: draft.mode, newMode: newMode, current: Array(draft.checked))
        draft.mode = newMode
        draft.checked = Set(seeded)
    }

    /// Debounced auto-save: every selection/subscription/mode edit sends
    /// `save_device_config` (settings untouched — that's the device Settings
    /// page, Task 6) followed by a fresh `preview_device` so the capacity
    /// bar tracks the edit. Mirrors `DeviceView.scheduleSave`/the retired
    /// LibraryView's `scheduleSave` (400ms).
    private func scheduleSave(_ d: MusicDraft) {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            onSaveDeviceConfig(
                serial,
                SelectionState(mode: d.mode, rules: Array(d.checked)),
                SubscriptionsWire(playlists: Array(d.subscriptions).sorted()))
            onPreviewDevice(serial)
        }
    }
}

/// Pure logic backing `DeviceMusicPage` — no SwiftUI, fully unit-testable.
enum DeviceMusicLogic {
    /// Caption line per the restructure plan's Global Constraints (exact
    /// strings). Disconnected overrides the mode caption entirely — the
    /// page stays editable, but changes won't reach the iPod until the next
    /// sync (Global Constraints: "Disconnected device: … pages editable
    /// with caption 'Not connected — changes apply on next sync'").
    static func caption(mode: SelectionMode, isConnected: Bool) -> String {
        guard isConnected else { return "Not connected — changes apply on next sync" }
        switch mode {
        case .all: return "Everything in your library syncs to this iPod."
        case .include: return "Checked items sync to this iPod."
        case .exclude: return "Checked items are left off this iPod."
        }
    }

    /// This page's four Global Constraints empty states, minus "no source
    /// configured" (handled earlier, app-wide, by
    /// `AppModel.needsFirstRunSetup` — see `LibraryContentLogic`'s doc
    /// comment). Layers ONE more case, `.deviceEmpty`, onto
    /// `LibraryContentLogic.state`'s three: Global Constraints' "device
    /// empty → 'Nothing synced yet — press Sync Now.'" only when the mode
    /// is Entire library (the browser is read-only there — nothing for the
    /// user to configure before a first sync) AND this page's device is the
    /// connected one AND it has never synced anything. In Selected/Except
    /// modes the browser stays the primary interactive UI regardless of
    /// sync history, so `.deviceEmpty` never overrides those.
    enum MusicPageContentState: Equatable {
        case needsScan
        case scanning(current: Int, total: Int)
        case libraryEmpty(path: String)
        case deviceEmpty
        case browser
    }

    static func contentState(
        library: LibraryInfo?, phase: Phase, configuredSource: String?,
        mode: SelectionMode, isConnected: Bool, syncedCount: Int
    ) -> MusicPageContentState {
        switch LibraryContentLogic.state(library: library, phase: phase, configuredSource: configuredSource) {
        case .needsScan: return .needsScan
        case let .scanning(current, total): return .scanning(current: current, total: total)
        case let .libraryEmpty(path): return .libraryEmpty(path: path)
        case .browse:
            guard mode == .all, isConnected, syncedCount == 0 else { return .browser }
            return .deviceEmpty
        }
    }

    /// "prominent Sync Now (disabled while syncing/scanning/disconnected)"
    /// — per the plan, only these three phase conditions disable it; idle,
    /// paused, notConfigured, and error all leave it enabled. On top of
    /// that (review finding #2), `isConnected` — whether THIS page's device
    /// is the one `phase` actually describes — must also hold: `phase` is
    /// global connected-device state, so a page for some OTHER (or no)
    /// connected device must stay disabled regardless of how idle that
    /// phase looks, or Sync Now would sync the wrong iPod.
    static func isSyncNowDisabled(phase: Phase, isConnected: Bool) -> Bool {
        guard isConnected else { return true }
        switch phase {
        case .syncing, .scanning, .noDevice: return true
        default: return false
        }
    }

    /// THE trust-critical function in this plan (see the plan's
    /// self-review notes): reproduces the device's current contents exactly
    /// on an Entire->Selected mode switch, so the switch is zero-diff —
    /// nothing gets silently removed by merely changing the mode picker.
    ///
    /// **Wire-data gap**: the daemon doesn't expose a distinct "device's
    /// current synced contents" — only `get_device_config` (mode + rules)
    /// and `preview_device` (aggregate track/byte counts, no per-album
    /// breakdown). When `previousMode == .all`, the device's current
    /// contents ARE (by definition of Entire-library mode) the whole known
    /// source library, so `fromDeviceContents` is fed `model.library?.artists`
    /// at the call site — an exact proxy in the common case, but NOT
    /// necessarily byte-for-byte identical to what's physically on the iPod
    /// if a prior sync deferred some albums via the fit-pass
    /// (`skipped_for_space`) or is otherwise still catching up. A
    /// byte-for-byte-accurate seed would need a new wire event carrying the
    /// manifest-backed synced set; out of scope here per the plan (flagged,
    /// not silently faked).
    ///
    /// Full 3x3 `SelectionMode` transition table (same-mode pairs are a
    /// no-op, kept rather than special-cased so every cell is testable
    /// independently):
    /// - `.all -> .include`: seed album-level include rules reproducing
    ///   `fromDeviceContents` (the zero-diff case above). A snapshot, not an
    ///   auto-following rule — deliberately album-level, not one `.artist`
    ///   rule, so future library growth stays opt-in via Entire mode.
    /// - `.all -> .exclude`: empty rules. An empty exclude set already means
    ///   "exclude nothing" == the entire library, so this is zero-diff
    ///   without seeding anything. Any rule dormant in `current` from an
    ///   earlier selected/except session is discarded here — keeping it
    ///   would reactivate it as a live removal the instant this switch
    ///   fires, which is exactly the silent-removal bug zero-diff guards
    ///   against.
    /// - `.include <-> .exclude`: `current` kept verbatim. The same rule
    ///   list is reinterpreted under the opposite mode's semantics
    ///   (only-these vs. everything-but-these) — an explicit content flip
    ///   the user asked for by picking a different mode, not something this
    ///   function should mask.
    /// - `* -> .all`: `current` kept verbatim ("dormant") — not cleared —
    ///   so switching straight back to Selected/Except later restores the
    ///   user's previous rules instead of starting from empty.
    static func seededSelection(
        fromDeviceContents artists: [LibraryArtist],
        previousMode: SelectionMode,
        newMode: SelectionMode,
        current: [SelectionRule]
    ) -> [SelectionRule] {
        guard previousMode != newMode else { return current }
        switch (previousMode, newMode) {
        case (.all, .include):
            return artists.flatMap { artist in
                artist.albums.map { SelectionRule.album(artist: artist.name, album: $0.name) }
            }
        case (.all, .exclude):
            return []
        case (.include, .exclude), (.exclude, .include):
            return current
        case (_, .all):
            return current
        default:
            return current
        }
    }

    struct CapacityBar: Equatable {
        var usedFraction: Double
        var projectedFraction: Double
        var usedBytes: UInt64
        var projectedBytes: UInt64
        var totalBytes: UInt64
    }

    /// `nil` until both a device-capacity reading and a live `preview_device`
    /// reply are available — the bar simply doesn't render until then (see
    /// `DeviceMusicPage.capacityBar`). "Used" is this edit's resulting sync
    /// footprint (`selectedBytes` + `playlistExtraBytes`, both from the
    /// live preview); "projected" layers in `projectedFreeBytes` when the
    /// daemon supplies it (accounts for fit-pass deferrals), falling back to
    /// the same used-bytes figure when it doesn't.
    static func capacityBar(storage: (free: Int64, total: Int64)?, preview: DevicePreview?) -> CapacityBar? {
        guard let storage, storage.total > 0, let preview else { return nil }
        let total = UInt64(storage.total)
        let used = preview.selectedBytes + preview.playlistExtraBytes
        let projectedFree = preview.projectedFreeBytes ?? (total > used ? total - used : 0)
        let projectedUsed = total > projectedFree ? total - projectedFree : total
        return CapacityBar(
            usedFraction: min(1, Double(used) / Double(total)),
            projectedFraction: min(1, Double(projectedUsed) / Double(total)),
            usedBytes: used, projectedBytes: projectedUsed, totalBytes: total)
    }

    static func capacitySummary(_ bar: CapacityBar) -> String {
        "\(DeviceRowFormatting.gbString(bar.usedBytes)) of \(DeviceRowFormatting.gbString(bar.totalBytes)) used"
    }

    /// `nil` when there's nothing to report (absent or empty) — mirrors the
    /// wire's own absent-means-nothing-to-flag convention for
    /// `unresolvedSubscriptions`.
    static func unresolvedSubscriptionsLine(_ unresolved: [String]?) -> String? {
        guard let unresolved, !unresolved.isEmpty else { return nil }
        return "\(unresolved.count) subscribed playlist\(unresolved.count == 1 ? "" : "s") couldn't be resolved"
    }
}
