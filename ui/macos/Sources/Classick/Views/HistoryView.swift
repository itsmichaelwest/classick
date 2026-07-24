import SwiftUI

/// Read-only table of past syncs, newest first by default, from
/// AppModel.history. Columns sort — the daemon's order is chronological, but
/// "show me every failure" and "which iPod was this" are the questions this
/// table exists to answer.
struct HistoryView: View {
  var model: AppModel

  @State private var sortOrder = [
    KeyPathComparator(\Row.timestamp, order: .reverse)
  ]

  private var rows: [Row] {
    model.authoritativeHistory.enumerated()
      .map { offset, entry in
        Row(
          id: offset,
          timestamp: entry.timestamp,
          when: Self.when(entry.timestamp),
          deviceName: deviceName(for: entry.serial),
          trigger: Self.trigger(entry.trigger),
          outcome: SyncOutcomeDisplay.make(entry.outcome),
          durationSecs: entry.durationSecs,
          duration: Self.duration(entry.durationSecs))
      }
      .sorted(using: sortOrder)
  }

  private struct Row: Identifiable {
    let id: Int
    let timestamp: String
    let when: String
    let deviceName: String
    let trigger: String
    let outcome: SyncOutcomeDisplay
    let durationSecs: UInt64
    let duration: String
  }

  var body: some View {
    Group {
      if model.authoritativeHistory.isEmpty {
        ContentUnavailableView(
          "No syncs yet", systemImage: "clock.arrow.circlepath",
          description: Text("Your sync history will appear here."))
      } else {
        Table(rows, sortOrder: $sortOrder) {
          TableColumn("When", value: \.timestamp) { Text($0.when) }
            .width(min: 140, ideal: 170)
          TableColumn("iPod", value: \.deviceName) { Text($0.deviceName) }
          TableColumn("Trigger", value: \.trigger) { Text($0.trigger) }
            .width(min: 80, ideal: 100)
          // Sorts by the raw wire value so every failure groups together
          // regardless of how it's spelled on screen.
          TableColumn("Outcome", value: \.outcome.raw) { row in
            Label {
              Text(row.outcome.label)
            } icon: {
              Image(systemName: row.outcome.systemImage)
                .foregroundStyle(row.outcome.tint)
            }
          }
          .width(min: 110, ideal: 130)
          TableColumn("Duration", value: \.durationSecs) { row in
            Text(row.duration).monospacedDigit()
          }
          .width(min: 80, ideal: 90)
        }
      }
    }
    .navigationTitle("Sync History")
  }

  private static func when(_ iso: String) -> String {
    guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
    return d.formatted(date: .abbreviated, time: .shortened)
  }
  private static func trigger(_ t: String) -> String {
    switch t {
    case "plug_in": return "Plug-in"
    default: return t.capitalized
    }
  }
  private func deviceName(for rawSerial: String) -> String {
    guard let serial = try? DeviceID(rawSerial), let state = model.devices[serial] else {
      return rawSerial
    }
    return state.identity.name ?? state.identity.modelLabel
  }
  private static func duration(_ secs: UInt64) -> String {
    let f = DateComponentsFormatter()
    f.allowedUnits = [.minute, .second]
    f.unitsStyle = .abbreviated
    return f.string(from: TimeInterval(secs)) ?? "\(secs)s"
  }
}

/// How a wire `outcome` reads in the history table. The raw values are the
/// daemon's `SyncOutcome` (`crates/classick/src/wire/history.rs`): ok, error,
/// aborted, cancelled. The column used to render `outcome.capitalized`, so a
/// failed sync and a successful one differed by one gray word — no symbol, no
/// color, nothing scannable down a column of thirty rows.
struct SyncOutcomeDisplay: Equatable, Sendable {
  var raw: String
  var label: String
  var systemImage: String

  static func make(_ raw: String) -> Self {
    switch raw {
    case "ok":
      Self(raw: raw, label: "Synced", systemImage: "checkmark.circle.fill")
    case "error":
      Self(raw: raw, label: "Failed", systemImage: "exclamationmark.triangle.fill")
    case "cancelled", "canceled":
      Self(raw: raw, label: "Cancelled", systemImage: "xmark.circle.fill")
    case "aborted":
      Self(raw: raw, label: "Interrupted", systemImage: "exclamationmark.circle.fill")
    // An outcome this build doesn't know (newer daemon) still renders as
    // itself rather than vanishing or asserting.
    default:
      Self(raw: raw, label: raw.capitalized, systemImage: "questionmark.circle")
    }
  }
}

extension SyncOutcomeDisplay {
  var tint: Color {
    switch raw {
    case "ok": .green
    case "error", "aborted": .orange
    default: .secondary
    }
  }
}

#if DEBUG
  #Preview("Populated") {
    HistoryView(model: PreviewFixtures.connectedSyncedModel())
      .frame(width: 720, height: 360)
  }

  #Preview("Empty") {
    HistoryView(model: PreviewFixtures.firstRunModel())
      .frame(width: 720, height: 360)
  }
#endif
