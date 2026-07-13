import SwiftUI

/// First-run setup: pick the music library folder, confirm the detected
/// iPod, opt into auto-sync, and persist it all in one `save_config`.
/// Reached from the `.notConfigured` menu row ("Set Up Classick…") and
/// auto-presented on first run — see `SetupWindowController`, which hosts
/// this view in an AppKit `NSWindow` (rather than a lazy SwiftUI `Window`
/// scene) so it can be shown deterministically from the app delegate.
/// `onClose` is injected by that controller because a hosted view has no
/// `@Environment(\.dismiss)` window to dismiss.
struct SetupWindow: View {
    var model: AppModel
    var onDone: (_ source: String, _ autoSync: Bool) -> Void
    var onClose: () -> Void

    @State private var pickedPath: String?
    @State private var autoSync = true
    @State private var isPickingFolder = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Set Up Classick")
                .font(.title2.bold())

            VStack(alignment: .leading, spacing: 6) {
                Text("Music Library")
                    .font(.headline)
                HStack {
                    Text(pickedPath ?? "No folder selected")
                        .foregroundStyle(pickedPath == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Button("Choose…") { isPickingFolder = true }
                }
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("iPod")
                    .font(.headline)
                Text(model.device.map { $0.name ?? $0.model } ?? "Plug in your iPod")
                    .foregroundStyle(model.device == nil ? .secondary : .primary)
            }

            Toggle("Sync automatically when plugged in", isOn: $autoSync)

            Text("Quit Music.app before syncing — iTunes will reject a Classick-managed iPod while it's running.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Spacer(minLength: 0)

            HStack {
                Spacer()
                Button("Done") {
                    guard let pickedPath else { return }
                    onDone(pickedPath, autoSync)
                    onClose()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(pickedPath == nil)
            }
        }
        .padding(20)
        .frame(width: 420, height: 340)
        .fileImporter(isPresented: $isPickingFolder, allowedContentTypes: [.folder]) { result in
            if case let .success(url) = result {
                pickedPath = url.path
            }
        }
    }
}
