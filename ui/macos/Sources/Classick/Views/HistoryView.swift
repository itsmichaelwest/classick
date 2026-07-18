import SwiftUI

/// Read-only table of past syncs, newest first, from AppModel.history.
struct HistoryView: View {
    var model: AppModel

    private var rows: [Row] {
        model.history.reversed().enumerated().map { Row(id: $0.offset, entry: $0.element) }
    }
    private struct Row: Identifiable { let id: Int; let entry: HistoryEntry }

    var body: some View {
        Group {
            if rows.isEmpty {
                ContentUnavailableView("No syncs yet", systemImage: "clock.arrow.circlepath",
                    description: Text("Your sync history will appear here."))
            } else {
                Table(rows) {
                    TableColumn("When") { r in Text(when(r.entry.timestamp)) }
                    TableColumn("Trigger") { r in Text(trigger(r.entry.trigger)) }
                    TableColumn("Outcome") { r in Text(r.entry.outcome.capitalized) }
                    TableColumn("Duration") { r in Text(duration(r.entry.durationSecs)) }
                }
            }
        }
        .navigationTitle("Sync History")
    }

    private func when(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
    private func trigger(_ t: String) -> String {
        switch t { case "plug_in": return "Plug-in"; default: return t.capitalized }
    }
    private func duration(_ secs: UInt64) -> String {
        let f = DateComponentsFormatter(); f.allowedUnits = [.minute, .second]; f.unitsStyle = .abbreviated
        return f.string(from: TimeInterval(secs)) ?? "\(secs)s"
    }
}

#if DEBUG
#Preview("Populated") {
    HistoryView(model: PreviewFixtures.connectedSyncedModel())
        .frame(width: 640, height: 360)
}

#Preview("Empty") {
    HistoryView(model: PreviewFixtures.firstRunModel())
        .frame(width: 640, height: 360)
}
#endif
