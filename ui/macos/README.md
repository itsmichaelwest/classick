# Classick — macOS app

Native macOS menu-bar app — the Mac counterpart to the WinUI 3 tray app
(`ui/windows/`). It owns the `classick` daemon and gives a daily-driver
iPod-sync experience: see the iPod's state, sync manually or automatically on
plug-in, and stay out of the way.

Talks to the daemon over the same **v2.0.0 IPC** as the Windows app; see
`../../docs/ipc-protocol.md`.

## Requirements

- macOS 15 (Sequoia) or later. Liquid Glass: audited during the 2026-07 app
  restructure — every surface uses standard `Form`/`List`/`NavigationSplitView`
  controls, which adopt Liquid Glass automatically under macOS 26, so the app
  ships **zero** `#available(macOS 26)` gates by design. Add one only if a
  custom-drawn surface visibly diverges from the system look.
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
app can spawn `classick --daemon`. It ad-hoc signs for local dev; the
Developer ID–signed, notarized `.dmg` is produced by `scripts/release-macos.sh`
(see **Release & distribution** below).

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
- **`Updates/Updater.swift`** — Sparkle 2 auto-updates. Guarded on
  `#if canImport(Sparkle)`: Sparkle is an **app-target-only** dependency (in
  `project.yml`, not `Package.swift`), so this file compiles to nothing under
  `swift test` and is fully live under `xcodebuild`. Feed URL + EdDSA public key
  live in `Info.plist` (`SUFeedURL` / `SUPublicEDKey`).

## Scope & idioms

- **Native menu** primary surface. The rich `.window` popover panel (storage
  meter + progress bar) is a documented **v1.1** option — the model/client
  layers are surface-agnostic, so it's a view-only swap.
- **Deferred to v1.1:** the History browser and the dry-run review flow
  (daemon-triggered syncs `--apply`).
- Auto-sync defaults on. First-run is a single window, not a wizard.

## Release & distribution

`scripts/release-macos.sh <version>` is a one-command local release. It builds
the daemon + app (Release config), makes the app self-contained, signs it with
your **Developer ID**, notarizes + staples it, wraps it in a `.dmg`, and
generates the Sparkle appcast. It is arm64-only and runs locally (not CI).

```bash
scripts/release-macos.sh 0.1.0                # -> dist/Classick-0.1.0.dmg + dist/appcast.xml
RELEASE_GH=1 scripts/release-macos.sh 0.1.0   # also `gh release create v0.1.0`
```

**Secrets never touch the repo.** All three credentials live only in the
Keychain and are looked up by name at run time:

1. **Developer ID Application** signing identity — derived via
   `security find-identity`; used to sign every nested Mach-O (app, daemon,
   bundled dylibs, and Sparkle's pre-signed `Updater.app`/`Autoupdate`/XPC,
   which must be re-signed with our cert + a secure timestamp or notarization
   rejects them).
2. **`classick-notary`** — a `notarytool` credential profile holding an
   **App Store Connect API key** (not an app-specific password). Created once:
   `xcrun notarytool store-credentials classick-notary --key … --key-id … --issuer …`.
3. **Sparkle EdDSA private key** — in the Keychain (created by Sparkle's
   `generate_keys`). `generate_appcast` reads it to sign each update; the
   matching public key is pinned in `Info.plist` as `SUPublicEDKey`.

### Self-containment

`scripts/bundle-macos-libs.sh` walks the daemon's non-system dylib closure
(libgpod + glib/gobject/gmodule/gdk-pixbuf/gettext/libxml2 + transitive deps),
copies each into `Contents/Frameworks`, and rewrites install-names to `@rpath`
so nothing points at `/opt/homebrew` or the build tree. **Known gap:**
gdk-pixbuf loader modules aren't bundled yet, so embedded cover-art thumbnails
may not render on a clean machine — art failure is non-fatal (the track still
syncs); see `LEARNINGS.md`.

### Publishing + auto-update wiring

Sparkle's `SUFeedURL` points at `https://itsmichaelwest.github.io/classick/appcast.xml`
(GitHub Pages). To ship an update: run the release script for the new version,
publish the `.dmg` as a `gh release` asset, and upload the regenerated
`appcast.xml` to Pages. Installed copies check that feed on launch
(`SUEnableAutomaticChecks`) and offer the update; "Check for Updates…" in the
menu triggers it on demand.
