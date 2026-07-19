import SwiftUI

/// The persistent bottom device strip. iPod identity + capacity/progress +
/// status + the primary action, driven entirely by `model.phase`.
struct DeviceRow: View {
  var model: AppModel
  var onSyncNow: (DeviceSerial) -> Void
  var onPause: (DeviceSerial) -> Void
  var onCancelSync: (DeviceSerial) -> Void
  var onResume: (DeviceSerial) -> Void
  var onRetry: (DeviceSerial) -> Void
  /// Opens the pairing/setup flow — the recovery path from `.notConfigured`.
  var onSetUp: (DeviceSerial?) -> Void

  private var serial: DeviceSerial? { model.focusedDeviceSerial }
  private var deviceState: DeviceViewState? {
    serial.flatMap { DeviceSurfaceLogic.state(serial: $0, in: model.devices) }
  }
  private var phase: Phase {
    DeviceSurfaceLogic.phase(for: deviceState, globalPhase: model.phase)
  }

  var body: some View {
    // Floating card per the design: uniform 20pt inset from the left,
    // right, and bottom edges. Hosted via `safeAreaInset(edge: .bottom)`
    // in MainWindow, so scroll content slides underneath the bar but is
    // never permanently obscured. The glass-vs-material split lives
    // entirely in `floatingBarBackground()` below — keep the layout
    // shared across OS versions.
    HStack(spacing: 14) {
      content
    }
    .padding(.horizontal, 16).padding(.vertical, 10)
    .frame(maxWidth: .infinity)
    .floatingBarBackground()
    .padding([.horizontal, .bottom], 20)
  }

  @ViewBuilder
  private var content: some View {
    if deviceState?.finalization != nil {
      twoRowCard(subtitle: "Finishing sync…") {
        EmptyView()
      } bar: {
        VStack(alignment: .leading, spacing: 6) {
          ProgressView("Saving completed albums")
          Text("Keep the iPod connected")
            .font(.caption)
            .foregroundStyle(.secondary)
        }
      }
    } else {
      phaseContent
    }
  }

  @ViewBuilder
  private var phaseContent: some View {
    switch phase {
    case .idle:
      // Idle card, two rows (user's mock): identity header — device
      // image, name, "Last synced at …", prominent Sync Now trailing —
      // over the full-width capacity bar with "X GB used" / "Y GB
      // total" beneath. Falls back to the pre-redesign single row in
      // the rare connected-but-no-capacity-reading window.
      if let storage = DeviceSurfaceLogic.storage(deviceState), storage.total > 0 {
        twoRowCard(subtitle: lastSyncedLine) {
          Button("Sync Now") { withSerial(onSyncNow) }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
        } bar: {
          capacityCard(storage: storage)
        }
      } else {
        deviceIdentity
        Spacer()
        statusText("\(syncedSummary) synced", idleSubordinateLines)
        Button("Sync Now") { withSerial(onSyncNow) }.buttonStyle(.borderedProminent)
      }

    case .syncing(let current, let total, let label, let etaSecs):
      twoRowCard(
        subtitle: total > 0 ? "Adding \(total) track\(total == 1 ? "" : "s")" : "Preparing sync…"
      ) {
        Button("Pause") { withSerial(onPause) }.controlSize(.large)
        Button("Cancel") { withSerial(onCancelSync) }.controlSize(.large)
      } bar: {
        VStack(alignment: .leading, spacing: 6) {
          ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
          HStack {
            Text("\(current) of \(total)\(label.isEmpty ? "" : " · \(label)")")
              .lineLimit(1).truncationMode(.middle)
            Spacer()
            if let etaSecs {
              Text("~\(formatEta(etaSecs)) left").layoutPriority(1)
            }
          }
          .foregroundStyle(.secondary)
        }
      }

    case .paused(let synced, let total):
      twoRowCard(subtitle: "Sync paused") {
        Button("Resume") { withSerial(onResume) }
          .buttonStyle(.borderedProminent)
          .controlSize(.large)
      } bar: {
        VStack(alignment: .leading, spacing: 6) {
          ProgressView(value: total.map { $0 > 0 ? Double(synced) / Double($0) : 0 } ?? 0)
          HStack {
            Text("\(synced)\(total.map { " of \($0)" } ?? "") synced")
            Spacer()
          }
          .foregroundStyle(.secondary)
        }
      }

    case .scanning:
      deviceIdentity
      ProgressView().controlSize(.small)
      Text("Updating library…").font(.caption).foregroundStyle(.secondary)
      Spacer()

    case .noDevice:
      Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
      VStack(alignment: .leading) {
        // Name the PAIRED device when there is one — "Michael's
        // iPod not connected" is actionable; "No iPod connected"
        // reads like the app forgot it exists.
        if let state = deviceState {
          Text("\(state.identity.name ?? state.identity.modelLabel) not connected").foregroundStyle(
            .secondary)
          Text("Plug it in to sync").font(.caption).foregroundStyle(.tertiary)
        } else {
          Text("No iPod connected").foregroundStyle(.secondary)
          Text("Plug in your iPod to sync").font(.caption).foregroundStyle(.tertiary)
        }
      }
      Spacer()
      statusText("\(deviceState?.libraryCount ?? 0) tracks selected", [])
      Button("Sync Now") {}.disabled(true)

    case .notConfigured:
      Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
      Text("iPod not set up").foregroundStyle(.secondary)
      Spacer()
      // Recovery path: without this, an unpaired iPod (e.g. after
      // "Remove iPod") left the user with a dead-end status line and
      // no way back except the menu-bar's Set Up row.
      Button("Set Up…") { onSetUp(serial) }.buttonStyle(.borderedProminent)

    case .error(let message):
      Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.title3)
      VStack(alignment: .leading) {
        Text("Sync failed").foregroundStyle(.red).fontWeight(.semibold)
        Text(message).font(.caption).foregroundStyle(.secondary).lineLimit(2)
      }
      Spacer()
      Button("Retry") { withSerial(onRetry) }.buttonStyle(.borderedProminent)
    }
  }

  /// The one card shape every resting/active state shares (user decision:
  /// consistent layouts): identity header — device image, name, one
  /// subtitle line, trailing action buttons — over a full-width bar row.
  /// Idle passes the capacity bar; syncing/paused pass a progress bar, so
  /// the bar is always in the same place instead of jumping into the
  /// header mid-sync.
  private func twoRowCard(
    subtitle: String,
    @ViewBuilder actions: () -> some View,
    @ViewBuilder bar: () -> some View
  ) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      HStack(spacing: 12) {
        DeviceIcon(drive: deviceState?.mountPath, size: 40)
        VStack(alignment: .leading, spacing: 2) {
          Text(deviceDisplayName)
            .font(.title3.bold())
          Text(subtitle)
            .foregroundStyle(.secondary)
        }
        Spacer()
        actions()
      }
      bar()
    }
    .padding(.vertical, 6)
  }

  private var deviceDisplayName: String {
    deviceState?.identity.name ?? deviceState?.identity.modelLabel ?? "iPod"
  }

  private var deviceIdentity: some View {
    HStack(spacing: 9) {
      DeviceIcon(drive: deviceState?.mountPath, size: 26)
      VStack(alignment: .leading, spacing: 1) {
        Text(deviceDisplayName).fontWeight(.semibold)
        if let s = DeviceSurfaceLogic.storageText(deviceState) {
          Text(s).font(.caption).foregroundStyle(.secondary)
        }
      }
    }
  }

  /// The connected device's live `preview_device` reply, when the Music
  /// page's edits have produced one — drives the projected (orange)
  /// overlay so the bar tracks checkbox changes before a sync runs.
  private var connectedPreview: DevicePreview? {
    deviceState?.preview
  }

  /// The merged capacity readout (was the Music page's own bottom strip —
  /// one floating bar now, not two stacked ones). Fill = actual disk
  /// usage; the orange layer, when a live preview supplies
  /// `projectedFreeBytes`, is the daemon's conservative projection of
  /// usage after the current selection syncs.
  private func capacityCard(storage: (free: Int64, total: Int64)) -> some View {
    let total = Double(storage.total)
    let usedBytes = UInt64(max(0, storage.total - storage.free))
    let usedFraction = min(1, Double(usedBytes) / total)
    let projectedFraction = connectedPreview?.projectedFreeBytes
      .map { min(1, max(0, (total - Double($0)) / total)) }
    return VStack(alignment: .leading, spacing: 6) {
      GeometryReader { proxy in
        ZStack(alignment: .leading) {
          Capsule().fill(.quaternary)
          if let projected = projectedFraction, projected > usedFraction {
            Capsule().fill(.orange.opacity(0.55))
              .frame(width: proxy.size.width * projected)
          }
          Capsule().fill(Color.accentColor)
            .frame(width: proxy.size.width * usedFraction)
        }
      }
      .frame(height: 6)
      HStack {
        Text("\(formatBytes(usedBytes)) used")
        Spacer()
        Text("\(formatBytes(UInt64(storage.total))) total")
      }
      .foregroundStyle(.secondary)
      // The skipped-for-space / missing-art rollups keep their home on
      // this card (trust surface — the user must see what a sync left
      // behind). The last-sync line is deliberately NOT here anymore:
      // it lives in the device page's titlebar subtitle.
      if let line = DeviceRowFormatting.skippedForSpaceLine(
        syncedSummary: syncedSummary, skipped: deviceState?.lastRun?.skippedForSpace)
      {
        Text(line).font(.caption).foregroundStyle(.secondary)
      }
      if let artLine = DeviceRowFormatting.artworkMissingLine(deviceState?.lastRun?.artwork) {
        Text(artLine).font(.caption).foregroundStyle(.orange)
      }
    }
  }

  private func statusText(_ big: String, _ subs: [String]) -> some View {
    VStack(alignment: .trailing, spacing: 1) {
      Text(big).fontWeight(.semibold).font(.callout)
      ForEach(subs, id: \.self) { sub in
        Text(sub).font(.caption).foregroundStyle(.secondary)
      }
    }
  }

  /// Subordinate lines under the idle status: last-sync timestamp, then
  /// (Task 17) the most recent run's skipped-for-space + missing-artwork
  /// rollups, when either applies. Kept secondary to the main "N synced"
  /// line — see `DeviceRowFormatting`.
  private var idleSubordinateLines: [String] {
    [
      deviceState?.latestSuccessfulSync.map { "Last sync \(shortDate($0.timestamp))" },
      DeviceRowFormatting.skippedForSpaceLine(
        syncedSummary: syncedSummary, skipped: deviceState?.lastRun?.skippedForSpace),
      DeviceRowFormatting.artworkMissingLine(deviceState?.lastRun?.artwork),
    ].compactMap { $0 }
  }

  private var syncedSummary: String {
    if let total = deviceState?.libraryCount {
      return "\(deviceState?.syncedCount ?? 0) of \(total)"
    }
    return "\(deviceState?.syncedCount ?? 0)"
  }

  private var lastSyncedLine: String {
    deviceState?.latestSuccessfulSync.map { "Last synced at \(shortDate($0.timestamp))" }
      ?? "Never synced"
  }

  private func withSerial(_ action: (DeviceSerial) -> Void) {
    guard let serial else { return }
    action(serial)
  }

  private func formatEta(_ secs: UInt64) -> String {
    let f = DateComponentsFormatter()
    f.allowedUnits = secs < 3600 ? [.minute, .second] : [.hour, .minute]
    f.unitsStyle = .abbreviated
    return f.string(from: TimeInterval(secs)) ?? "\(secs)s"
  }

  private func shortDate(_ iso: String) -> String {
    guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
    return d.formatted(date: .omitted, time: .shortened)
  }
}

extension View {
  /// The design's floating-bar surface. On macOS 26+ this is real Liquid
  /// Glass (`glassEffect`, which brings its own edge treatment — no extra
  /// shadow wanted). On macOS 15 the native equivalent is a material card:
  /// `regularMaterial` in the same rounded rect, a hairline separator
  /// stroke, and a soft shadow to lift it off the scroll content.
  @ViewBuilder
  fileprivate func floatingBarBackground() -> some View {
    if #available(macOS 26.0, *) {
      self.glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    } else {
      self.background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
          RoundedRectangle(cornerRadius: 16, style: .continuous)
            .strokeBorder(Color(nsColor: .separatorColor).opacity(0.6), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.12), radius: 10, y: 3)
    }
  }
}

/// Pure formatting for the two device-row rollup lines (Task 17). Kept free
/// of SwiftUI so the byte→GB conversion and visibility rules are unit
/// testable without instantiating a view.
enum DeviceRowFormatting {
  /// Bytes → "X.Y GB", always one decimal place — deliberately not
  /// `ByteCountFormatter` (used elsewhere for `formatBytes`), since that
  /// picks the unit (MB/GB/...) and decimal count automatically and the
  /// skipped-for-space line needs a fixed GB/one-decimal format.
  static func gbString(_ bytes: UInt64) -> String {
    String(format: "%.1f GB", Double(bytes) / 1_000_000_000)
  }

  /// "Synced N of M — X albums didn't fit (Y GB)", or `nil` when nothing
  /// was deferred this run (`skipped == nil` or `skipped.tracks == 0`).
  static func skippedForSpaceLine(syncedSummary: String, skipped: SkippedForSpace?) -> String? {
    guard let skipped, skipped.tracks > 0 else { return nil }
    return
      "Synced \(syncedSummary) — \(skipped.albums) album\(skipped.albums == 1 ? "" : "s") didn't fit (\(gbString(skipped.bytes)))"
  }

  /// Number of tracks to report as missing art, or `nil` when the run's
  /// artwork rollup shows nothing to flag (no failed sources and every
  /// eligible track got art embedded).
  static func artworkMissingCount(_ artwork: ArtworkSummary?) -> Int? {
    guard let artwork else { return nil }
    let shortfall = artwork.eligible - artwork.embedded
    guard artwork.failedSources > 0 || shortfall > 0 else { return nil }
    return shortfall > 0 ? shortfall : artwork.failedSources
  }

  /// "Art missing for X tracks", or `nil` when `artworkMissingCount` is `nil`.
  static func artworkMissingLine(_ artwork: ArtworkSummary?) -> String? {
    guard let count = artworkMissingCount(artwork) else { return nil }
    return "Art missing for \(count) track\(count == 1 ? "" : "s")"
  }
}

#if DEBUG
  @MainActor
  private func deviceRowPreview(_ model: AppModel) -> some View {
    DeviceRow(
      model: model, onSyncNow: { _ in }, onPause: { _ in }, onCancelSync: { _ in },
      onResume: { _ in }, onRetry: { _ in }, onSetUp: { _ in }
    )
    .frame(width: 820)
  }

  #Preview("Idle") {
    deviceRowPreview(PreviewFixtures.connectedSyncedModel())
  }

  #Preview("Syncing") {
    deviceRowPreview(PreviewFixtures.syncingModel())
  }

  #Preview("Paused") {
    deviceRowPreview(PreviewFixtures.pausedModel())
  }

  #Preview("Scanning") {
    deviceRowPreview(PreviewFixtures.scanningModel())
  }

  #Preview("No device") {
    deviceRowPreview(PreviewFixtures.noDeviceModel())
  }

  #Preview("Not configured") {
    deviceRowPreview(PreviewFixtures.notConfiguredModel())
  }

  #Preview("Error") {
    deviceRowPreview(PreviewFixtures.errorModel())
  }

  #Preview("Skipped for space + missing art") {
    deviceRowPreview(PreviewFixtures.connectedOverfullModel())
  }
#endif
