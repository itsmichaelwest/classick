<div align="center">

<img src="ui/windows/Classick.UI/Assets/AppList.targetsize-512.png" width="160" alt="Classick app icon">

# Classick

Sync a FLAC library to an iPod Classic — from **Windows** or **macOS**.

[![Status](https://img.shields.io/badge/status-pre--1.0-yellow)](#status)
[![Windows](https://img.shields.io/badge/tray%20app-Windows%2011%2B-0078D4)]()
[![macOS](https://img.shields.io/badge/menu--bar%20app-macOS%2015%2B-000000)]()
[![CLI](https://img.shields.io/badge/CLI-Windows%20%C2%B7%20Linux%20%C2%B7%20macOS-success)]()
[![Rust](https://img.shields.io/badge/Rust-stable-orange)]()
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

</div>

---

Classick wraps libgpod, transcodes FLAC to ALAC on the way over, and runs quietly from the system tray (Windows) or menu bar (macOS). Point it at your library, plug in the iPod, and it keeps the two in sync.

## Status

Routine syncs work end-to-end on both platforms — device detection, plan review, transcode, write, manifest, and pause/resume. It's been driven hardest on macOS lately (an iPod Classic MC293), where sync + artwork are on-device verified. Not every iPod Classic variant has been tested, and the corners still occasionally surprise me. Still pre-1.0: **don't point it at music you can't replace.**

The macOS app ships as a notarized, self-updating build (Sparkle).

## Rockbox compatibility

Classick can make one shared ALAC library play on **both** the stock Apple firmware *and* [Rockbox](https://www.rockbox.org/) on a dual-boot iPod. Turn on **Rockbox compatibility** and transcoded tracks are written self-describing — embedded MP4 tags + cover art — so Apple firmware reads them via the iTunesDB and Rockbox reads them straight from the files. The **Update artwork & metadata** button (and any normal sync) refreshes artwork + tags for both firmwares in place, without re-copying audio — handy after retagging your library in something like Lidarr.

## What's in here

- `crates/classick/` — Rust core. One binary that runs as a CLI, an IPC subprocess, or a long-lived daemon. Cross-platform; wraps libgpod, spawns the transcoder, writes the iTunesDB.
- `ui/windows/` — WinUI 3 / .NET 10 **tray app**. Owns the daemon, surfaces device state + sync progress + settings + first-run wizard. See `ui/windows/README.md`.
- `ui/macos/` — SwiftUI **menu-bar app** (macOS 15+). The Mac counterpart to the tray app, same daemon + IPC. See `ui/macos/README.md`.
- `docs/` — IPC wire format, design specs, SCSI notes.

Each UI owns the `classick` daemon and talks to it over the same JSON IPC (`docs/ipc-protocol.md`) — a **named pipe** (`\\.\pipe\classick`) on Windows, a **Unix socket** on macOS. Nothing about the UIs is baked into the Rust core.

## Build

### macOS

Rust stable, Xcode 26 / Swift 6.3, and the vendored macOS libgpod. Transcoding uses the system `afconvert` (no ffmpeg or MSYS2 needed).

```bash
cargo build --release        # the daemon the app embeds + spawns
ui/macos/bundle.sh           # -> ui/macos/Classick.app (ad-hoc signed, for dev)
```

For a signed + notarized release: `scripts/release-macos.sh <version>`. Full detail in `ui/macos/README.md`.

### Windows

Rust stable on MSVC, MSYS2 at `C:\msys64` for the GLib headers libgpod's bindgen pass needs, and the .NET 10 SDK.

```powershell
cargo build --release
cd ui\windows
dotnet build Classick.UI.slnx -c Debug
```

The csproj copies `target\release\classick.exe` and the libgpod DLLs next to `Classick.UI.exe` at build time. Skip the cargo step and you'll get a warning, not a build failure.

## More

- `AGENTS.md` — orientation for anyone working in this repo, human or agent.
- `docs/SPEC.md` — original design, rejected alternatives, FFI rationale.
- `LEARNINGS.md` — incidents and gotchas. Read before touching iTunes-DB or artwork code.
