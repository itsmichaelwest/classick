# Classick — macOS app

Native macOS menu-bar app — the Mac counterpart to the WinUI 3 tray app
(`ui/windows/`). It owns the `classick` daemon and gives a daily-driver
iPod-sync experience: see the iPod's state, sync manually or automatically on
plug-in, and stay out of the way.

Talks to the daemon over the same **v1.1.0 IPC** as the Windows app
(`docs/ipc-protocol.md`) — **no Rust changes**. This is SP2 of the macOS port;
see `docs/superpowers/specs/2026-07-12-macos-swiftui-app-design.md`.

## Requirements

- macOS 15 (Sequoia) or later. Liquid Glass is adopted conditionally on macOS 26.
- Xcode 26 / Swift 6.3 (Swift 6 strict concurrency).
- A built `classick` daemon binary (the app spawns it). See the repo root
  `AGENTS.md` for the Rust build, and `crates/classick/vendor/libgpod/BUILD-NOTES.md`
  for the macOS libgpod build.
- **Not sandboxed** — the app spawns a daemon that needs raw device access and
  shares the `$TMPDIR` socket. Do not enable the App Sandbox; it breaks IPC.

## Build & run

```bash
# From the repo root: build the daemon the app embeds + spawns.
cargo build --release

# Build the Swift executable and assemble Classick.app (LSUIElement agent).
ui/macos/bundle.sh            # -> ui/macos/Classick.app  (ad-hoc signed)

open ui/macos/Classick.app    # menu-bar icon appears; no Dock icon
```

`bundle.sh` embeds `target/release/classick` into `Contents/Resources` so the
app can spawn `classick --daemon`. Real Developer ID signing + notarization +
`.dmg` is SP3.

## Test

```bash
cd ui/macos
swift test        # wire-codec + AppModel-reducer + DaemonClient (mock socket)
```

The socket client and SwiftUI scenes are verified by running the app against a
real daemon + iPod (drive-and-observe); the pure logic is unit-tested.

## Architecture

Three layers under `Sources/Classick/`, mirroring the WinUI split (UI owns
presentation; the daemon owns config, device detection, scheduling, sync):

- **`Ipc/`** — `WireModels.swift` (`Codable` command/event types, snake_case
  `type` discriminator) + `DaemonClient.swift` (`actor`: connects to
  `$TMPDIR/classick.sock`, validates the `hello` handshake, sends
  `DaemonCommand`s, yields `DaemonEvent`s as an `AsyncStream`, auto-reconnects).
- **`Model/`** — `AppModel.swift` (`@Observable @MainActor`; reduces
  `DaemonEvent`s into `phase`/`device`/`config`/`pendingPrompt`) +
  `Storage.swift` (iPod free/total via `URLResourceValues` — the daemon reports
  no storage on macOS).
- **`Daemon/DaemonProcess.swift`** — spawns + owns `classick --daemon` (attaches
  if a daemon already answers the socket); stops it on quit.
- **`Views/`** + `ClassickApp.swift` — `MenuBarExtra` (`.menu` style) driven by
  `AppModel`; `Settings` scene (General + About); first-run setup `Window`;
  daemon-relayed prompts via `NSAlert`. Startup/shutdown run from an
  `AppDelegate` (not a menu `.task`, which only materializes on click).

## Scope & idioms

- **Native menu** primary surface. The rich `.window` popover panel (storage
  meter + progress bar) is a documented **v1.1** option — the model/client
  layers are surface-agnostic, so it's a view-only swap.
- **Deferred to v1.1:** the History browser and the dry-run review flow
  (daemon-triggered syncs `--apply`).
- Auto-sync defaults on. First-run is a single window, not a wizard.
