import SwiftUI

@main
struct ClassickApp: App {
    var body: some Scene {
        MenuBarExtra("Classick", systemImage: "ipod") {
            Text("Classick")
            Button("Quit Classick") { NSApplication.shared.terminate(nil) }
                .keyboardShortcut("q")
        }
        .menuBarExtraStyle(.menu)
    }
}
