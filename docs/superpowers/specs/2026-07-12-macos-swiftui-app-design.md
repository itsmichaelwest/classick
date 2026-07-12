# macOS SwiftUI App (design)

**Goal:** a native macOS menu-bar app (`Classick.app`) — the Mac counterpart to
the WinUI 3 tray app — that owns the `classick` daemon and gives a usable
daily-driver experience: see the iPod's state, sync (manually or automatically
on plug-in), and get out of the way. Built with proper Mac idioms, not a WinUI
port.

**This is sub-project 2 of 3** (see `2026-07-12-macos-core-enablement-design.md`
for the program). It builds entirely on SP1's proven, hardware-verified daemon
backend and talks to it over the existing v1.1.0 IPC contract — **zero Rust
changes**. SP3 (packaging: signed/notarized `.dmg`) follows.

**Non-goals (this spec):**
- Any change to the Rust core/daemon — the wire contract in
  `docs/ipc-protocol.md` is fixed and already exercised (SP1's `daemon-probe`).
- The full **History browser** and the **dry-run review** flow — deferred to
  v1.1 (daemon-triggered syncs always `--apply`, so there's no review event to
  handle in v1).
- **Rich-panel** primary surface — documented below as the future option, not
  built in v1.
- Signing / notarization / `.dmg` — SP3. Dev builds are ad-hoc signed.

---

## Target & toolchain

- **Deployment floor: macOS 15 Sequoia.** Liquid Glass and other 26-only
  niceties are adopted conditionally (`if #available(macOS 26, *)`); the app
  degrades gracefully on 15. `MenuBarExtra`, `@Observable`, `Settings` scene,
  `UserNotifications`, and Swift 6 concurrency are all available on 15.
- **Swift 6.3 / Xcode 26.6** (installed and active). Swift 6 language mode
  (strict concurrency).
- **Build: SwiftPM package + a `bundle.sh`** that assembles `Classick.app`
  around the built executable (Info.plist with `LSUIElement`, Resources, the
  embedded `classick` binary). Fully CLI-drivable (no Xcode GUI), and the bundle
  layout is the foundation SP3 extends for signing + the dylib closure.
  *Alternative if bundling friction appears (esp. entitlements for
  UserNotifications): an `.xcodeproj` built via `xcodebuild`. See Risks.*

---

## Architecture

Three layers under `ui/macos/`, mirroring the WinUI split (UI owns presentation;
the daemon owns config, device polling, scheduling, sync orchestration).

### `DaemonClient` (actor)
Owns the Unix-socket connection to the daemon and is the only code that touches
the wire.
- Resolves the socket path the same way Rust does: `NSTemporaryDirectory()` +
  `classick.sock` (= `$TMPDIR/classick.sock`, the confstr path SP1 pinned).
- Connect → read the `hello` line → **validate `protocol_version`** (major-mismatch
  = fatal, surface an error state) → then stream.
- Sends `DaemonCommand`s and yields `DaemonEvent`s as an `AsyncStream`.
- Newline-delimited JSON via `Codable` models mirroring `docs/ipc-protocol.md`.
- **Auto-reconnect** with backoff on drop (the daemon may restart); re-subscribe
  on reconnect.

### `AppModel` (`@Observable`, `@MainActor`)
The single source of truth, updated by a reducer over `DaemonEvent`s. Holds:
device (serial/model/name/drive/storage), daemon state (idle/syncing), live sync
progress (`current`/`total`/`label`, derived from forwarded `sync_event`s),
config (source, auto-sync, schedule), and any pending prompt. **State lives at
app scope** so closing/reopening the menu never loses in-flight progress (the
WinUI lesson in `LEARNINGS.md`).

### Daemon lifecycle
The app **spawns and owns** `classick --daemon` on launch (locating the binary:
bundled in `Contents/Resources` for a real build, else `target/release/classick`
in dev), and terminates it on quit. Mirrors the WinUI `CoreLocator` ownership
model. If a daemon is already running (socket answers), attach instead of
spawning a second.

### Scenes (SwiftUI `App`)
- `MenuBarExtra` (`.menu` style) — the primary surface.
- `Settings` scene — the ⌘, window.
- A setup `Window` (first-run) opened on demand.
- A prompt alert presented when the daemon relays a decision request.

---

## Primary surface — native menu (`MenuBarExtra .menu`)

The status-item **icon** encodes state without opening the menu:
no-device (outline) · idle (filled) · **syncing (animated)** · error (badge).
Liquid Glass tint on 26.

The dropdown is native rows, reactive to `AppModel`:

```
Michael’s iPod — up to date
108 / 160 GB · 1,276 tracks
Last sync: 2h ago
──────────────────
Sync Now            ⌘S
Settings…           ⌘,
──────────────────
Quit Classick       ⌘Q
```

**States** (top section swaps; actions persist):
- *No device:* "No iPod connected".
- *Not set up:* "Set Up Classick…" → opens the setup window; Sync Now hidden.
- *Idle:* device + storage + last-sync (above), **Sync Now** enabled.
- *Syncing:* "Syncing… 34 of 120" + "Karma Police" (updates live while open);
  **Sync Now** becomes **Cancel Sync**. Icon animates.
- *Error / rejected:* a short reason line ("Music.app is open — quit it to
  sync") + a retry affordance.

### Future option (documented, not built): rich panel (`.window` style)
Switching `MenuBarExtra` to `.window` yields a popover panel with room for a
real **storage meter** and a **live progress bar** + activity feed (the WinUI
popover shape). The `AppModel`/`DaemonClient` layers are surface-agnostic, so
this is a view-only swap later. Reference layout:

```
┌─────────────────────────────┐
│  Michael’s iPod             │
│  iPod Classic · 160 GB       │
│  ▓▓▓▓▓▓▓░░░░  108 / 160 GB   │
│ ───────────────────────────  │
│  Syncing…  34 / 120          │
│  ▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░  Karma…│
│ ───────────────────────────  │
│  [ Cancel ]          ⚙︎   ⋯  │
└─────────────────────────────┘
```

---

## First-run — single setup window (not a wizard)

When no source is configured, the menu shows **"Set Up Classick…"**. It opens one
window (Mac apps do lightweight setup, not multi-step wizards):
1. **Music folder** — `.fileImporter` (security-scoped) to pick the FLAC root.
2. **iPod** — shows the auto-detected device ("iPod Classic · Michael's iPod")
   from the daemon's `device_connected`, or "Plug in your iPod" if absent.
3. **"Sync automatically when plugged in"** toggle (default **on**).
4. **Done** → sends `save_config { source, ipod:{serial,model_label}, daemon:{…} }`.

A one-line note reminds the user to quit Music.app before syncing (the
Restore-trap warning), matching the Windows copy.

---

## Settings window (⌘,)

Standard SwiftUI `Settings` scene, minimal for v1:
- **General:** music folder (re-pick), **Sync automatically on plug-in** toggle,
  schedule (Off / every N hours), **Launch at login** (`SMAppService`), and
  **Remove this iPod** (`forget_ipod`).
- **About:** version, license/LGPL note, GitHub link.

Every edit persists via `save_config` (debounced), and the daemon is the store
of record — the app never writes the TOML itself. **History pane deferred to
v1.1.**

---

## Auto-sync & notifications

- **Auto-sync default ON:** when the daemon reports `device_connected` for the
  configured iPod and auto-sync is enabled, the daemon already handles the
  plug-in trigger; the app just reflects it. Manual mode = the user uses **Sync
  Now**.
- **Notifications** (`UserNotifications`): "Sync complete — 12 added" on
  `finish{success:true}`, and a failure notification on `finish{success:false}`
  or `error`. Gated by a settings toggle. Requests authorization on first run.

---

## In-sync prompts (modal alert)

A native menu can't host an interactive prompt, so when a `sync_event` carries a
forwarded `prompt` (source-change safeguard, per-track Retry/Skip/Abort) or
`form`, the app presents a **modal alert window** with the message + options and
replies via `decide_prompt { id, choice }`. Without this a prompt would stall
the sync indefinitely, so it is in-scope for v1.

---

## IPC surface the app uses

Wire contract is fixed (`docs/ipc-protocol.md`); Swift `Codable` models mirror it.

**Commands sent** (`DaemonCommand`, `type` tag, snake_case):
`subscribe_device_events`, `get_status`, `get_config`, `save_config`,
`forget_ipod`, `trigger_sync {source:"manual"}`, `cancel_sync`,
`decide_prompt {id, choice}`. *(Deferred: `get_history`.)*

**Events received** (`DaemonEvent`):
`hello {protocol_version, core_version}`,
`status_update {state, configured, ipod_connected, last_sync?, next_scheduled?, storage?}`,
`config_update {source, daemon, ipod}`,
`device_connected {serial, model_label, drive, name?}`,
`device_disconnected {serial}`,
`sync_rejected {reason}`,
`sync_event {line}` — wraps a v1.0.0 event; the app parses `header`, `summary`,
`track_start {current,total,label}`, `track_done`, `log {message}`,
`prompt {id,message,options}`, `form {id,...}`, `error {message,recovery_hints?}`,
`finish {success}`. *(No `review` in v1 — daemon syncs `--apply`.)*

---

## Testing & verification

- **Unit (no daemon):** `DaemonClient` JSON codec (round-trip every command +
  event, incl. a real `sync_event` line); `AppModel` reducer (feed a scripted
  event sequence → assert derived state: idle→syncing→progress→idle, device
  connect/disconnect, prompt surfaced). Pure, fast.
- **Integration (mock socket):** a test `UnixSocket` server emits a canned
  hello + event script; assert the client streams and reconnects.
- **Manual/hardware (the gate):** launch `Classick.app` against the real daemon +
  iPod — menu reflects live device state; plug/unplug updates it; **Sync Now**
  drives a real sync with live progress; a completion notification fires; the
  setup window configures a fresh install.

---

## Risks & mitigations

1. **UserNotifications needs a signed, bundle-identified app.** Notifications may
   not register from a hand-rolled, unsigned bundle. *Mitigation:* ad-hoc sign
   the dev bundle with a stable bundle id in `bundle.sh`; if still flaky, fall
   back to an `.xcodeproj`/`xcodebuild` build (SP3 needs real signing anyway).
2. **`MenuBarExtra .menu` reactive updates while open.** Live "Syncing… N of M"
   depends on the menu re-rendering from `@Observable` state; verify the row
   updates while the menu is open (fallback: rely on the animated icon +
   notification, refresh the count on menu-open).
3. **Socket lifecycle races.** App spawns the daemon, then must connect before
   the socket is bound. *Mitigation:* retry-connect with backoff; treat
   "already running" (socket answers) as attach-not-spawn.
4. **Security-scoped folder access.** The picked music folder needs a
   security-scoped bookmark so the daemon (a separate process) can read it.
   *Mitigation:* the daemon runs unsandboxed (SP1 decision) and reads the path
   directly; the app just stores the path string. Confirm no sandbox is enabled
   (consistent with the `$TMPDIR` socket decision — the app is not
   App-Store-sandboxed).
5. **Not sandboxed** — required (raw device access + spawns a daemon +
   shared `$TMPDIR` socket). State it so nobody enables the sandbox and breaks
   IPC.

---

## Definition of done

- `Classick.app` builds from the CLI (`swift build` + `bundle.sh`) and launches
  as a menu-bar agent (no Dock icon).
- Menu reflects live device state from the daemon; plug/unplug updates it.
- First-run setup configures source + iPod; config persists (daemon TOML).
- **Sync Now** drives a real sync to the iPod with live progress; a completion
  notification fires.
- A relayed prompt surfaces as an alert and is answered via `decide_prompt`.
- `DaemonClient` codec + `AppModel` reducer unit tests pass.
- `ui/macos/README.md` documents build/run, mirroring `ui/windows/README.md`.

---

## Follow-on (not this spec)

- **v1.1:** History browser, dry-run review flow, rich-panel `.window` surface.
- **SP3 — Packaging:** self-contained `Classick.app` bundling `classick` + the
  libgpod dylib closure + ffmpeg; Developer ID signing + hardened runtime +
  notarize + staple + `.dmg` → GitHub release.
