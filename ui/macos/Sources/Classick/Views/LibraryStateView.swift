import SwiftUI

/// Every non-browser state a library-backed page can show, in one place.
/// `LibraryView` and `DeviceMusicPage` render the same situations and used
/// to hand-build near-identical `VStack` + `.headline` + `Button` copies of
/// each, which drifted apart in spacing, font, and whether an action was
/// offered at all.
///
/// `ContentUnavailableView` is the platform's own empty-state control — it
/// owns the symbol/title/description/action rhythm, so these now read like
/// History's and the playlist editor's (which already used it) instead of
/// like four bespoke stacks. The constrained copy ("No audio files found in
/// <path>", "Nothing synced yet — press Sync Now.") is preserved verbatim as
/// the description; only the chrome around it changed.
@MainActor
enum LibraryStateView {
  static func needsScan(onScan: @escaping () -> Void) -> some View {
    ContentUnavailableView {
      Label("Scan Your Library", systemImage: "music.note.square.stack")
    } description: {
      Text("Classick needs to read your library's tags once")
    } actions: {
      Button("Scan Library", action: onScan)
        .keyboardShortcut(.defaultAction)
    }
  }

  static func libraryEmpty(path: String, onScan: @escaping () -> Void) -> some View {
    ContentUnavailableView {
      Label("No Music Found", systemImage: "music.note.square.stack")
    } description: {
      Text("No audio files found in \(path)")
    } actions: {
      Button("Rescan Library", action: onScan)
    }
  }

  /// No action button: "press Sync Now" refers to the app-wide device bar
  /// at the bottom of the window, and a second copy here would be two
  /// controls for one intent.
  static var deviceEmpty: some View {
    ContentUnavailableView(
      "Nothing Synced Yet", systemImage: "ipod",
      description: Text("Nothing synced yet — press Sync Now."))
  }

  /// Determinate scan progress. Not a `ContentUnavailableView`: this state
  /// is work in flight, not an absence, and the platform control has no
  /// progress affordance.
  static func scanning(current: Int, total: Int) -> some View {
    let value = total > 0 ? Double(current) / Double(total) : 0
    return VStack(spacing: 12) {
      ProgressView(value: value)
        .frame(maxWidth: 260)
        .motion(Motion.meter, value: value)
      Text("Scanning… \(current) of \(total)")
        .font(.caption)
        .foregroundStyle(.secondary)
        .contentTransition(.numericText())
        .motion(Motion.meter, value: current)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }

  /// The one loading treatment for pages waiting on a daemon reply — was
  /// three (a bare spinner with "Loading…" beneath, `ProgressView("Loading…")`,
  /// and an inline "Loading settings…" row).
  static func loading(_ label: String = "Loading…") -> some View {
    VStack(spacing: 8) {
      ProgressView().controlSize(.small)
      Text(label).foregroundStyle(.secondary)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}

#if DEBUG
  #Preview("Needs scan") {
    LibraryStateView.needsScan(onScan: {}).frame(width: 620, height: 400)
  }

  #Preview("Library empty") {
    LibraryStateView.libraryEmpty(path: "~/Music/FLAC", onScan: {})
      .frame(width: 620, height: 400)
  }

  #Preview("Device empty") {
    LibraryStateView.deviceEmpty.frame(width: 620, height: 400)
  }

  #Preview("Scanning") {
    LibraryStateView.scanning(current: 812, total: 3_140).frame(width: 620, height: 400)
  }

  #Preview("Loading") {
    LibraryStateView.loading().frame(width: 620, height: 400)
  }
#endif
