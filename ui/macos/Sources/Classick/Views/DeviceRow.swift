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
            statusText("\(syncedSummary) synced", model.lastSync.map { "Last sync \(shortDate($0.timestamp))" })
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
            statusText("Adding \(total) tracks", etaSecs.map { "~\(formatEta($0)) left" })
            Button("Pause", action: onPause)
            Button("Cancel", action: onCancelSync)

        case let .paused(synced, total):
            deviceIdentity
            Spacer()
            statusText("Paused", "\(synced)\(total.map { " of \($0)" } ?? "") synced")
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
            statusText("\(model.libraryCount ?? 0) tracks selected", nil)
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

    private func statusText(_ big: String, _ sub: String?) -> some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(big).fontWeight(.semibold).font(.callout)
            if let sub { Text(sub).font(.caption).foregroundStyle(.secondary) }
        }
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
