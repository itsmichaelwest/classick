import SwiftUI

/// Smart playlist rule builder (Task 7): name, match all/any, rule rows
/// (field/op/value), limit (none/bytes/tracks), order, plus a live preview
/// footer and the same rename/delete chrome as `ManualPlaylistEditor`.
///
/// **Live preview, v1:** the daemon has no `preview_smart`-style dry-run —
/// the only way to learn a rule set's resolved track/byte count is to save
/// it and read the count back off the next `playlists_update` broadcast
/// (`model.playlists`'s matching `PlaylistSummary`). Since every edit here
/// already round-trips through a debounced `save_playlist` (plan Task 7:
/// "All edits send `.savePlaylist` (debounced)"), the footer is simply
/// `model.playlists`'s entry for this slug — no separate request needed,
/// at the cost of the preview lagging by one debounce+round-trip. Flagged
/// here per the plan's "acceptable latency, note in code."
struct SmartRulesEditor: View {
  var model: AppModel
  var slug: String
  var detail: PlaylistDetail
  var onSavePlaylist: (PlaylistPayload) -> String
  var onDeletePlaylist: (String) -> Void

  struct SmartDraft: Equatable {
    var name: String = ""
    var matching: SmartMatching = .all
    var rules: [SmartRuleWire] = []
    var limitKind: LimitKind = .none
    var limitValueText: String = ""
    var order: SmartOrder = .alpha
  }

  enum LimitKind: String, CaseIterable, Identifiable, Equatable, Sendable {
    case none, bytes, tracks
    var id: String { rawValue }
  }

  @State private var draft = SmartDraft()
  @State private var acknowledgedDraft = AcknowledgedDraft(canonical: SmartDraft(), revision: 0)
  @State private var hasCanonicalDraft = false
  @State private var saveTask: Task<Void, Never>?
  @State private var showDeleteConfirm = false

  private var summary: PlaylistSummary? { model.playlists.first { $0.slug == slug } }

  var body: some View {
    VStack(spacing: 0) {
      Form {
        Section("Match") {
          Picker("", selection: Binding(get: { draft.matching }, set: { value in edit { $0.matching = value } })) {
            Text("All of the following").tag(SmartMatching.all)
            Text("Any of the following").tag(SmartMatching.any)
          }
          .pickerStyle(.segmented)
          .labelsHidden()
          ForEach(draft.rules.indices, id: \.self) { index in
            ruleRow(index)
          }
          Button("Add Rule") {
            edit { $0.rules.append(SmartRuleWire(field: .artist, op: .is, value: "")) }
          }
        }
        Section("Limit") {
          Picker(
            "Limit", selection: Binding(get: { draft.limitKind }, set: { value in edit { $0.limitKind = value } })
          ) {
            Text("No limit").tag(LimitKind.none)
            Text("File size").tag(LimitKind.bytes)
            Text("Track count").tag(LimitKind.tracks)
          }
          if draft.limitKind != .none {
            TextField(
              draft.limitKind == .bytes ? "Bytes" : "Tracks",
              text: Binding(get: { draft.limitValueText }, set: { value in edit { $0.limitValueText = value } }))
          }
          Picker("Order", selection: Binding(get: { draft.order }, set: { value in edit { $0.order = value } })) {
            Text("Alphabetical").tag(SmartOrder.alpha)
            Text("Recently Modified").tag(SmartOrder.recentlyModified)
            Text("Random (Stable)").tag(SmartOrder.randomStable)
          }
        }
        Section {
          Text(SmartRulesLogic.previewLine(summary: summary))
            .font(.caption).foregroundStyle(.secondary)
        }
      }
      .formStyle(.grouped)
    }
    // Same editable-titlebar treatment as `ManualPlaylistEditor` — see
    // its doc comment; the old in-page header duplicated the titlebar.
    .navigationTitle(Binding(get: { draft.name }, set: { value in edit { $0.name = value } }))
    // See ManualPlaylistEditor: toolbarTitleMenu supplies the chevron,
    // RenameButton triggers inline titlebar editing.
    .toolbarTitleMenu {
      RenameButton()
    }
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Menu {
          RenameButton()
          Button("Delete Playlist", role: .destructive) { showDeleteConfirm = true }
        } label: {
          Image(systemName: "ellipsis.circle")
        }
      }
    }
    .task { seedIfNeeded() }
    .onChange(of: detail) { _, _ in seedIfNeeded() }
    .onChange(of: model.playlistRevision) { _, _ in
      reconcileSaveAcknowledgement()
    }
    .onDisappear { saveTask?.cancel() }
    .confirmationDialog(
      "Delete “\(draft.name)”?", isPresented: $showDeleteConfirm, titleVisibility: .visible
    ) {
      Button("Delete Playlist", role: .destructive) {
        onDeletePlaylist(slug)
        model.selectedDestination = .library
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        PlaylistEditorLogic.deleteConfirmMessage(
          subscribedDeviceCount: PlaylistEditorLogic.subscribedDeviceCount(
            slug: slug, deviceConfigs: model.deviceConfigs)))
    }
  }

  @ViewBuilder
  private func ruleRow(_ index: Int) -> some View {
    HStack {
      Picker(
        "",
        selection: Binding(
          get: { draft.rules[index].field }, set: { value in edit { $0.rules[index].field = value } })
      ) {
        ForEach(SmartField.allCases, id: \.self) { field in
          Text(field.rawValue.capitalized).tag(field)
        }
      }
      .labelsHidden()
      .frame(width: 90)
      Picker(
        "", selection: Binding(get: { draft.rules[index].op }, set: { value in edit { $0.rules[index].op = value } })
      ) {
        ForEach(SmartOp.allCases, id: \.self) { op in
          Text(SmartRulesLogic.opLabel(op)).tag(op)
        }
      }
      .labelsHidden()
      .frame(width: 110)
      TextField(
        "Value",
        text: Binding(get: { draft.rules[index].value }, set: { value in edit { $0.rules[index].value = value } }))
      Button {
        edit { $0.rules.remove(at: index) }
      } label: {
        Image(systemName: "minus.circle")
      }
      .buttonStyle(.plain)
    }
  }

  /// Reconciles canonical rules without replacing a newer local generation.
  private func seedIfNeeded() {
    guard let rules = detail.rules else { return }
    let canonical = SmartDraft(
      name: detail.name ?? "",
      matching: rules.matching,
      rules: rules.rules,
      limitKind: SmartRulesLogic.limitKind(for: rules.limit),
      limitValueText: SmartRulesLogic.limitValueText(for: rules.limit),
      order: rules.order)
    acknowledgedDraft.reconcile(
      canonical: canonical, revision: detail.playlistRevision,
      acknowledgedRequestID: detail.acknowledgedRequestID)
    draft = acknowledgedDraft.value
    hasCanonicalDraft = true
  }

  /// Debounced auto-save, gated on validity so an incomplete mid-edit rule
  /// (blank value, non-numeric limit) is never persisted — the save is
  /// simply skipped for that tick and retried on the next edit.
  private func scheduleSave(_ d: SmartDraft) {
    saveTask?.cancel()
    saveTask = Task {
      try? await Task.sleep(for: .milliseconds(400))
      guard !Task.isCancelled else { return }
      guard PlaylistEditorLogic.isNameValid(d.name),
        SmartRulesLogic.rulesAreValid(d.rules),
        SmartRulesLogic.isLimitValid(kind: d.limitKind, valueText: d.limitValueText)
      else { return }
      let rules = SmartRulesWire(
        matching: d.matching, rules: d.rules,
        limit: SmartRulesLogic.limit(kind: d.limitKind, valueText: d.limitValueText),
        order: d.order)
      let requestID = onSavePlaylist(.smart(slug: slug, name: d.name, rules: rules))
      acknowledgedDraft.markSubmitted(requestID: requestID)
    }
  }

  private func edit(_ mutation: (inout SmartDraft) -> Void) {
    guard hasCanonicalDraft else { return }
    var edited = draft
    mutation(&edited)
    guard edited != draft else { return }
    acknowledgedDraft.edit(edited)
    draft = acknowledgedDraft.value
    scheduleSave(draft)
  }

  private func reconcileSaveAcknowledgement() {
    guard let requestID = model.playlistAcknowledgedRequestID,
      let submitted = acknowledgedDraft.submitted[requestID]
    else { return }
    acknowledgedDraft.reconcile(
      canonical: submitted.value, revision: model.playlistRevision,
      acknowledgedRequestID: requestID)
    draft = acknowledgedDraft.value
  }
}

/// Pure logic backing `SmartRulesEditor` — no SwiftUI, fully unit-testable
/// (see `PlaylistEditorLogicTests`).
enum SmartRulesLogic {
  /// "rule-row validity" — every row needs a non-blank value. An empty
  /// rule SET is valid (spec: "Smart rules matching zero tracks → valid
  /// empty playlist" — zero rules under `matching: .all` matches
  /// everything, which is a legitimate, if unusual, smart playlist).
  nonisolated static func rulesAreValid(_ rules: [SmartRuleWire]) -> Bool {
    rules.allSatisfy { !$0.value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
  }

  nonisolated static func limitKind(for limit: SmartLimitWire?) -> SmartRulesEditor.LimitKind {
    switch limit {
    case nil: return .none
    case .bytes: return .bytes
    case .tracks: return .tracks
    }
  }

  nonisolated static func limitValueText(for limit: SmartLimitWire?) -> String {
    switch limit {
    case nil: return ""
    case .bytes(let n): return String(n)
    case .tracks(let n): return String(n)
    }
  }

  /// `.none` is always valid (no value to check); `.bytes`/`.tracks`
  /// require a positive integer.
  nonisolated static func isLimitValid(kind: SmartRulesEditor.LimitKind, valueText: String) -> Bool
  {
    switch kind {
    case .none: return true
    case .bytes: return (UInt64(valueText).map { $0 > 0 }) ?? false
    case .tracks: return (Int(valueText).map { $0 > 0 }) ?? false
    }
  }

  /// Only call once `isLimitValid` holds — returns `nil` for `.none` (no
  /// limit) as well as for an unparseable/non-positive value, so an
  /// invalid in-progress edit never gets promoted into a saved limit.
  nonisolated static func limit(kind: SmartRulesEditor.LimitKind, valueText: String)
    -> SmartLimitWire?
  {
    switch kind {
    case .none: return nil
    case .bytes: return UInt64(valueText).map { .bytes($0) }
    case .tracks: return Int(valueText).map { .tracks($0) }
    }
  }

  nonisolated static func opLabel(_ op: SmartOp) -> String {
    switch op {
    case .is: return "is"
    case .contains: return "contains"
    case .gte: return "≥"
    case .lte: return "≤"
    }
  }

  /// The preview footer's text — "Calculating…" until the daemon's next
  /// `playlists_update` carries this slug's resolved count (see this
  /// type's doc comment for why there's no dedicated preview request).
  nonisolated static func previewLine(summary: PlaylistSummary?) -> String {
    guard let summary else { return "Calculating…" }
    return
      "\(summary.tracks) track\(summary.tracks == 1 ? "" : "s") · \(formatBytes(summary.bytes))"
  }
}

#if DEBUG
  #Preview("Rules configured") {
    SmartRulesEditor(
      model: PreviewFixtures.playlistDetailModel(PreviewFixtures.smartPlaylistDetail),
      slug: PreviewFixtures.smartPlaylistDetail.slug,
      detail: PreviewFixtures.smartPlaylistDetail,
      onSavePlaylist: { _ in "preview" }, onDeletePlaylist: { _ in }
    )
    .frame(width: 640, height: 560)
  }

  #Preview("No rules") {
    let detail = PlaylistDetail(
      slug: "fresh-smart-playlist", name: "New Smart Playlist", kind: .smart,
      tracks: nil,
      rules: SmartRulesWire(matching: .all, rules: [], limit: nil, order: .alpha, seed: 0),
      error: nil,
      acknowledgedRequestID: "preview-request")
    SmartRulesEditor(
      model: PreviewFixtures.playlistDetailModel(detail), slug: detail.slug, detail: detail,
      onSavePlaylist: { _ in "preview" }, onDeletePlaylist: { _ in }
    )
    .frame(width: 640, height: 560)
  }
#endif
