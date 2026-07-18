import SwiftUI

/// The persistent bottom device strip. iPod identity + capacity/progress +
/// status + the primary action, driven entirely by `model.phase`.
struct DeviceRow: View {
    var model: AppModel
    var onSyncNow: () -> Void
    var onPause: () -> Void
    var onCancelSync: () -> Void
    var onResume: () -> Void
    var onRetry: () -> Void

    var body: some View {
        HStack(spacing: 14) {
            content
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .frame(maxWidth: .infinity)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .idle:
            deviceIdentity
            capacityBar
            Spacer()
            statusText("\(syncedSummary) synced", idleSubordinateLines)
            Button("Sync Now", action: onSyncNow).buttonStyle(.borderedProminent)

        case let .syncing(current, total, label, etaSecs):
            deviceIdentity
            VStack(alignment: .leading, spacing: 4) {
                ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
                    .frame(maxWidth: 320)
                Text("\(current) of \(total)\(label.isEmpty ? "" : " · \(label)")")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            statusText("Adding \(total) tracks", [etaSecs.map { "~\(formatEta($0)) left" }].compactMap { $0 })
            Button("Pause", action: onPause)
            Button("Cancel", action: onCancelSync)

        case let .paused(synced, total):
            deviceIdentity
            Spacer()
            statusText("Paused", ["\(synced)\(total.map { " of \($0)" } ?? "") synced"])
            Button("Resume", action: onResume).buttonStyle(.borderedProminent)

        case .scanning:
            deviceIdentity
            ProgressView().controlSize(.small)
            Text("Updating library…").font(.caption).foregroundStyle(.secondary)
            Spacer()

        case .noDevice:
            Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
            VStack(alignment: .leading) {
                Text("No iPod connected").foregroundStyle(.secondary)
                Text("Plug in your iPod to sync").font(.caption).foregroundStyle(.tertiary)
            }
            Spacer()
            statusText("\(model.libraryCount ?? 0) tracks selected", [])
            Button("Sync Now", action: onSyncNow).disabled(true)

        case .notConfigured:
            Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
            Text("iPod not set up").foregroundStyle(.secondary)
            Spacer()

        case let .error(message):
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.title3)
            VStack(alignment: .leading) {
                Text("Sync failed").foregroundStyle(.red).fontWeight(.semibold)
                Text(message).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer()
            Button("Retry", action: onRetry).buttonStyle(.borderedProminent)
        }
    }

    private var deviceIdentity: some View {
        HStack(spacing: 9) {
            Image(systemName: "ipod").font(.title2).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.device?.name ?? model.device?.model ?? "iPod").fontWeight(.semibold)
                if let s = model.storageText { Text(s).font(.caption).foregroundStyle(.secondary) }
            }
        }
    }

    @ViewBuilder private var capacityBar: some View {
        if let storage = model.deviceStorage {
            let used = Double(storage.total - storage.free)
            ProgressView(value: used, total: Double(storage.total))
                .frame(maxWidth: 260)
                .tint(.accentColor)
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
            model.lastSync.map { "Last sync \(shortDate($0.timestamp))" },
            DeviceRowFormatting.skippedForSpaceLine(syncedSummary: syncedSummary, skipped: model.lastRunSkippedForSpace),
            DeviceRowFormatting.artworkMissingLine(model.lastRunArtwork),
        ].compactMap { $0 }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
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
        return "Synced \(syncedSummary) — \(skipped.albums) album\(skipped.albums == 1 ? "" : "s") didn't fit (\(gbString(skipped.bytes)))"
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
    DeviceRow(model: model, onSyncNow: {}, onPause: {}, onCancelSync: {}, onResume: {}, onRetry: {})
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
