# Post-v1 Roadmap — ipod-sync

Forward-looking capture of enhancements identified during the Phase 2 build. Not yet committed-to or scheduled; the order below reflects dependency analysis and risk sequencing, not promises.

**v1 = Phase 2 complete** (tag `phase-2-complete`). Everything below assumes v1 ships first.

---

## Phase 3 — Format + encoder expansion

**Scope:** support pass-through for iPod-native formats (MP3, AAC, ALAC, optionally WAV/AIFF) so they're copied bit-perfect instead of re-encoded; add **refalac64** (Apple's reference ALAC encoder) as the preferred encoder for tracks that DO need transcoding.

**Effort:** ~3-5 days

**Why now:** small, foundational, no architectural shift. Extends `transcode.rs` and `source.rs` only. Retires two limitations (FLAC-only sources, ffmpeg-only encoding) that future phases would otherwise inherit. Detailed spec: `2026-05-18-phase-3-formats-and-encoders.md`.

**Open questions:** none blocking — design is locked.

---

## Phase 4 — Multiple iPods

**Scope:** allow the tool to manage more than one iPod from the same machine. Each iPod gets its own manifest (keyed by serial), its own per-device exclude list / sync settings, and is identified by a user-chosen nickname plus the FirewireGuid-derived serial.

**Effort:** ~1 week

**Why now:** the daemon (Phase 5) needs to know which iPod just got plugged in — that requires per-iPod identity already being a first-class concept. Doing #3 before #2 prevents a daemon-rewrite when multi-iPod support arrives.

**Key changes anticipated:**
- Manifest path becomes `%APPDATA%\ipod-sync\manifests\<serial>.json` (versus today's single `manifest.json`).
- `Manifest.ipod_serial` (already a field, unused) becomes mandatory and the primary key.
- `ipod::device::detect_ipod_mount` returns `Vec<MountInfo>` not `Result<String>`; orchestrator needs `--ipod <serial>` or `--ipod <drive>` disambiguation when multiple are mounted.
- A small `%APPDATA%\ipod-sync\ipods.json` registry mapping serial → nickname + last-known-good drive + per-iPod settings (source overrides, format preferences, etc.).
- Schema migration: existing single `manifest.json` is read once on first Phase 4 run and moved to `manifests/<serial>.json` based on the connected iPod's serial at migration time. Backwards-compatible: a user with only one iPod sees a transparent upgrade.

**Open questions:**
- How does the user provide a nickname on first sight of a new iPod? CLI prompt? Auto-default to "iPod" + serial-suffix? Pulled from the `Itdb_Device.user_name` libgpod field?
- What happens to a manifest if its iPod hasn't been plugged in for >N runs — auto-archive? Stay forever? (Probably stay forever; cleanup is the user's call.)

---

## Phase 5 — Daemon + tray + auto-sync on connect

**Scope:** the "plug-and-go" experience that closes the last UX gap vs iTunes. Background service watches for iPod device-arrival events; on connect, runs sync (with each iPod's settings from Phase 4); shows a tray icon for status; sends desktop notification on completion or failure.

**Effort:** ~2 weeks

**Why now:** depends on Phase 4 (multi-iPod identity). Could plausibly come before Phase 6 (GUI) since the GUI would be a client of the daemon's IPC and benefits from the daemon already existing.

**Anticipated architecture:**

```
┌───────────────────────────┐         ┌──────────────────────┐
│  ipod-syncd (background)  │◄────────│  ipod-sync (CLI)     │
│   - USB event listener    │  IPC    │   - manual runs      │
│   - sync orchestrator     │  (local │   - status query     │
│   - per-iPod state        │   sock) │   - --no-daemon flag │
└─────────┬─────────────────┘         └──────────────────────┘
          │
          │ IPC
          │
┌─────────▼──────────────────┐
│  ipod-sync-tray (per-OS)   │
│   - Windows: tray icon     │
│   - macOS:  menubar item   │
│   - Linux:  AppIndicator   │
└────────────────────────────┘
```

The existing CLI binary stays unchanged for users who don't want the daemon. The daemon hosts the orchestrator logic (extracted from `main.rs`); both CLI and tray talk to it over a local socket (named pipe on Windows, Unix socket on Mac/Linux). If the daemon isn't running, the CLI falls back to in-process orchestration like today.

**Anticipated platform specifics:**

| OS | Background process | Device event source | Tray | Notification |
|---|---|---|---|---|
| Windows | Tray app (no true service — runs in user session, registered in HKCU\...\Run) | `RegisterDeviceNotification` + `WM_DEVICECHANGE` | `tray-icon` crate or `windows-rs` `Shell_NotifyIcon` | `tauri-plugin-notification` or `windows-rs` `ToastNotification` |
| macOS | launchd user agent (`~/Library/LaunchAgents/`) | `IOKit` `IOServiceAddMatchingNotification` for USB attach | NSStatusItem via Rust cocoa bindings (or a tiny Swift companion) | `NSUserNotification` |
| Linux | systemd user unit (`~/.config/systemd/user/`) | `udev` events via `libudev-sys` or polling `/proc/mounts` | `AppIndicator` via libappindicator | `notify-rust` crate |

**Open questions:**
- Should the daemon auto-sync immediately on connect, or wait N seconds (in case the user wants to manually run with custom flags first)? Configurable; default 5-10 sec grace period.
- What about USB power-only / charge-only connections where the iPod doesn't mount as disk? Don't trigger sync; wait for mass-storage event.
- Single-instance enforcement: prevent two daemons fighting. Lock file in `%APPDATA%` / `~/.local/share/ipod-sync/`.

---

## Phase 6 — Native GUI app

**Scope:** desktop GUI for configuring sources, iPod nicknames, exclude lists, viewing sync history, and triggering manual syncs. Replaces or augments the tray icon from Phase 5 with a full window.

**Effort:** ~weeks to months (depends heavily on the architecture decision below)

**Why last:** the GUI is a client of the daemon's IPC. Building it before Phase 5 means re-implementing the orchestrator inside the GUI process and then throwing that away when the daemon arrives. Wait until Phase 5 lands.

### The architectural decision

**There is no Rust library that provides truly native controls on Windows + macOS + Linux.** The options:

| Approach | What "looks like" | Effort | Notes |
|---|---|---|---|
| **Tauri** | Each platform's native WebView (WebView2 / WKWebView / WebKitGTK) rendering an HTML/CSS/JS frontend. "Modern cross-platform" — consistent across OSes, not literally native. | One codebase, ~1-2 weeks for initial app | Mature; used by Linear, Cloudflare, 1Password. Reasonable trade-off. |
| **Three native frontends sharing a Rust core** | Genuinely native on each platform: WinUI 3 on Windows (C# or Rust via windows-rs), SwiftUI on macOS, GTK 4 + libadwaita on Linux. | 3× the UI work, but each platform feels at home | The `ipod-sync` crate becomes a library; each platform's UI calls into it via FFI. The daemon abstraction (Phase 5's IPC) means UIs don't need to embed the FFI directly — they can just be IPC clients of the daemon. |
| **Slint / iced / egui** | Cross-platform but not native — own widget rendering. Looks consistent but foreign on every OS. | Smaller than Tauri | Best for embedded/kiosk. Skip for a consumer-facing desktop app. |

**My current lean:** start with **Tauri** for the first GUI iteration. Ship fast, single codebase, "good enough" native feel on all three. If users complain about a specific platform feeling out of place, add a native frontend for that platform later (the daemon-IPC architecture makes this clean — the GUI is just a client).

**Decision is deferred to Phase 6 spec writing.** The lean above is current thinking, not commitment.

---

## Cross-cutting investigations (any phase)

### Pipe-based transcode intermediates

Phase 1 chose temp files (per SPEC §12 #1) to sidestep MP4 `moov`-atom seekability with named pipes. Phase 3 adds another temp file (WAV intermediate for refalac). Both are correctness-first, performance-second. Investigate:

- **(a)** `ffmpeg → stdout WAV → refalac stdin → temp.m4a`, avoiding the WAV temp file in the refalac path. Windows piping is fragile; needs a real proof-of-concept before relying on it.
- **(b)** The original "named pipe + fragmented MP4 (`-movflags +empty_moov+frag_keyframe`)" path SPEC §4.4 sketched for the ALAC stage, eliminating the temp m4a as well.

At ~1,400 tracks of ~28 MB ALAC each, removing one temp-file IO pass per track saves ~40 GB of disk write/read churn per full sync. Meaningful at scale, irrelevant for one-shot 12-track tests.

Best done in Phase 4+ after the rest of the architecture has settled — premature optimization right now.

### Other parking-lot items

These didn't make it into a phase but are worth recording for future sessions:

- **Smart playlists, play counts, ratings two-way sync** — SPEC §7 lists these as out-of-scope for v1. Might rejoin scope after the basic tool is rock-solid. Risky (touches more of libgpod's surface, including the parts we patched out in Phase 0).
- **AC3 / WMA / other niche source formats** — currently rejected. Could add transcode support, low priority.
- **Smart-playlist-rule UNKNOWN warning fix** — Phase 1 hit `itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed` as benign noise. It's because libgpod walks a smart-playlist rule type it doesn't recognize on this iPod. Cosmetic; suppressed in user output via the GLib log handler.
- **iPod nano 5G / Touch support** — currently impossible because Phase 0 Task 3 patched libplist + iTunesCDB out of libgpod. Re-introducing requires un-patching, finding a Windows-compatible libplist build, and dealing with the SQLite-based DB format the nano 5G+ uses. Significant work; only worth it if there's actual demand.
- **Native distribution / installer** — currently `cargo build`-only. A real release should produce a signed MSI (Windows), .pkg/.dmg (macOS), .deb/.rpm/AppImage (Linux). With everything bundled (libgpod runtime DLLs + pixbuf loaders + refalac + ffmpeg + the .exe). Phase 5 or 6 territory, depending on whether the daemon needs an installer first.
- **Rust port of libgpod's iTunesDB writer** — SPEC §12.7 documents this as the v2/v3 migration that removes the MinGW runtime DLL dependency and gives us full control over the hashed-DB signing on Classic 7G. Big project (weeks). Only revisit if the libgpod vendor approach becomes untenable.
