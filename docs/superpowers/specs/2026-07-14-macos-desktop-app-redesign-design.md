# macOS Desktop App Redesign (iTunes-style main window) — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-14
**Scope:** Promote the macOS app from a menu-bar-only accessory to a proper
Dock app with a persistent, iTunes-style main window: a source sidebar
(Library / Devices / History), a content area, and a full-width device row
pinned at the bottom that shows sync progress, capacity, and the primary
action. Adds a daemon-side filesystem watcher (auto-refresh the library, no
manual Rescan) and a daemon-computed sync ETA. macOS-first; the watcher, ETA,
and one minor wire bump are cross-platform so the Windows UI inherits them.

## Problem & goal

The macOS app today is `LSUIElement` (accessory, no Dock icon) and surfaces
everything through a `MenuBarExtra(.menu)` dropdown, with the library browser,
settings, and setup as transient AppKit windows opened from the menu. That's a
fine companion but a poor *primary* experience: there's no home, no persistent
view of your library or device, and sync progress/storage are buried in a menu
you have to open.

**Goal:** a conventional Mac app with a Dock icon and a persistent main
window modeled on classic iTunes — browse your library and pick what syncs in
one place, see your device and history in a sidebar, and always see sync
progress + capacity + the next action in a status strip at the bottom. Keep
the menu-bar extra for glances and keep the background-sync companion role
(closing the window must not stop syncing).

## Decisions (brainstorm 2026-07-14)

| Question | Decision |
|---|---|
| App shape | **Hybrid.** Dock icon + main window become primary; the menu-bar extra stays (condensed) for glances and quick actions. Rejected: drop the menu bar (loses the always-available glance); regular-app-that-quits-on-close (loses background sync). |
| Close behavior | Closing the main window does **not** quit — app keeps running in Dock + menu bar, daemon keeps syncing. Quit is explicit (⌘Q). Dock click / menu "Open Classick" reopens the window. |
| Window layout | `NavigationSplitView`: sidebar (Library / Devices / History) + detail content + a full-width **device row** pinned at the bottom across all views. Rejected: single device-centric view (no library home); toolbar-tabs instead of sidebar (less iTunes-like). |
| Where selection lives | On the **Music Library** view itself — inline checkboxes as you browse. The Device view is status-only. Rejected: selection on the Device view (iTunes-classic, but splits browsing from choosing). |
| Selection persistence | Auto-save (debounced) instead of a modal Save button, since the browser is now a persistent view. Keep the existing "Sync now?" offer when a change affects a connected iPod. |
| Device view content | **Dashboard from existing state** — identity, capacity, X/Y synced, last/next sync, device-scoped controls. No track listing (would need new IPC + manifest reads). Rejected: full contents list; synced-by-artist summary. |
| Manual scan | Removed as a primary control. A **`notify`-crate filesystem watcher** in the daemon auto-refreshes the library (incremental) and broadcasts `library_update`. Manual "Rescan" survives only as a menu-bar-extra escape hatch. |
| Sync ETA | **Computed daemon-side** from rolling track throughput, emitted as an optional `eta_secs` on the `track_start` sync event. Both UIs render it. Rejected: UI-side estimate (would diverge between macOS and Windows). |
| Settings split | Device-scoped controls (auto-sync, Rockbox compat, Remove this iPod) move onto the Device view. Global prefs (source folder, notifications, updates) stay in the standard Settings (⌘,) scene. |

## Non-goals

- No audio playback. Classick is a sync tool, not a player.
- No on-iPod per-track contents listing (Device view is a dashboard).
- No per-track selection — granularity stays Artist / Album / Genre as today.
- No redesign of the first-run setup wizard or the selection *engine*; both
  are reused as-is.

---

## 1. App shell & lifecycle (SwiftUI)

**Activation policy.** Flip `LSUIElement` to `false` in `Info.plist` so the
app gets a Dock icon and a normal app menu. `MenuBarExtra` continues to work
alongside a regular window; no `NSApp.setActivationPolicy` juggling is needed
at steady state.

**Scenes.** Add a `WindowGroup` (id `"main"`) hosting `MainWindow`, alongside
the existing `MenuBarExtra` and `Settings` scenes. Single-instance: use
`.windowResizability(.contentSize)` off, a sensible default frame
(~980×620), and `handlesExternalEvents` / a scene-id guard so we don't spawn
duplicate windows.

**Hybrid close-≠-quit.** Because the app is no longer `LSUIElement`, the
default AppKit behavior is "quit when last window closes" only if
`applicationShouldTerminateAfterLastWindowClosed` returns `true`. Override it
to **`false`** on `AppDelegate` so closing the main window leaves the app
(and the daemon it owns) running in the Dock + menu bar.

**Reopen.** Implement `applicationShouldHandleReopen(_:hasVisibleWindows:)` to
re-open/focus the `MainWindow` when the Dock icon is clicked with no window
visible. The menu-bar extra's "Open Classick" does the same via an
`@Environment(\.openWindow)` action keyed to `"main"`.

**Ownership unchanged.** The daemon process, `DaemonClient`, and `AppModel`
stay owned by `AppDelegate` (per the current rationale in `ClassickApp.swift`)
— the new window observes `AppModel`, it does not own lifetime.

## 2. Main window layout (SwiftUI)

`MainWindow` is a `NavigationSplitView`:

- **Sidebar** — a `List` with three sections and a `@State` selection enum
  `SidebarItem { case library, device(serial), history }`:
  - **Library** → `Music Library` (track count badge).
  - **Devices** → the connected iPod row (name/model + a connected dot);
    absent when no device. Selecting it shows the Device view.
  - **History** → `Sync History`.
- **Detail** — switches on the sidebar selection: `LibraryView`,
  `DeviceView`, or `HistoryView`.
- **Bottom device row** — attached with
  `.safeAreaInset(edge: .bottom) { DeviceRow(model:) }` on the split view so
  it spans sidebar+detail and persists across every selection.

Default sidebar selection on launch: `Library` when configured; the empty/setup
state otherwise (see §7).

## 3. Library view (SwiftUI)

The current `ChooseMusicWindow` browser becomes the persistent `LibraryView`,
reusing `SelectionDraft` and the existing daemon commands. Changes:

- **No modal footer / Cancel / Save.** The mode picker (Entire library / Only
  selected / All except selected), Artists/Genres segmented control, search,
  and the outline of mixed-state checkboxes remain.
- **Auto-save.** Debounce selection edits (~500 ms after the last change) and
  send `save_selection` automatically. Preserve the existing behavior of
  seeding the draft once from `selection_update` and never clobbering
  in-flight edits (the `seededFromModel` latch moves over intact).
- **Sync-on-change offer.** When an auto-saved change yields
  `selectionPreview.adds + removes > 0` and a device is connected, keep the
  existing "Sync now?" prompt (moved out of the old Save button path).
- **Capacity/impact** that used to live in the browser footer moves to the
  device row (§6); the library view keeps only the per-row track/byte counts.
- **Empty/scanning state** (library never scanned) is retained but rephrased:
  since the watcher scans on startup, the empty state is a transient
  "Reading your library…" with progress, not a "Scan Library" button.

## 4. Device view (SwiftUI, dashboard)

A read-mostly dashboard built entirely from state `AppModel` already holds:

- **Header:** device name (editable label is out of scope), model, serial,
  capacity total.
- **Capacity breakdown:** used / to-add / free (same figures as the device
  row, larger).
- **Sync status:** X of Y synced, last sync (from `lastSync`), next scheduled
  (from `nextScheduledUnixSecs`).
- **Device-scoped controls** (write via existing commands):
  - Auto-sync toggle → `save_config` with updated `DaemonSettings.enabled`.
  - Rockbox-compat toggle → `save_config` (`rockbox_compat`); "Update existing
    library for Rockbox" → `backfill_rockbox` (both already wired in Settings;
    the actions move/duplicate here).
  - **Remove this iPod** → `forget_ipod`.

No new IPC. This view is empty/hidden when no device is connected (the sidebar
Devices row is absent).

## 5. Sync History view (SwiftUI)

A read-only `Table` over the `history_update` entries the daemon already
sends (`AppModel` will retain the latest `[HistoryEntry]`; today it only keeps
`lastSync`). Columns: date/time (localized), trigger (manual / scheduled /
plug-in), outcome, duration. The add/modify/remove summary is decoded
leniently if present. Requesting history uses the existing `get_status`/event
stream; no new command.

## 6. Device row (SwiftUI) + ETA (Rust)

A single `DeviceRow` view driven by `model.phase` + device/storage/selection
state, pinned bottom across all views. States:

| State | Left | Middle | Right |
|---|---|---|---|
| Idle / up to date | iPod icon + name/model | capacity bar (used / to-add / free) | "X synced · Last sync …" + **Sync Now** |
| Syncing | icon + "Syncing…" | live progress bar + current track ("114 of 336 · …") | "Adding N tracks · ~M min left" + **Pause** + **Cancel** |
| No device | dimmed icon + "No iPod connected" | — | "N tracks selected" + disabled Sync Now |
| Error | red icon + "Sync failed" | inline failure reason | **Details** + **Retry** |

**ETA (Rust).** The daemon computes remaining time from rolling throughput
over the last N completed tracks (N≈8) and emits it as a new **optional**
`eta_secs: u64` field on the `track_start` event:

- Source of truth: `crates/classick/src/progress.rs::run_ipc` (or wherever
  `track_start` is emitted) tracks per-track completion timestamps in a small
  ring buffer, computes `remaining_tracks * avg_secs_per_recent_track`, and
  omits the field until it has ≥2 samples (so the UI shows "114 of 336" with
  no ETA early on).
- Wire: `docs/ipc-protocol.md` inner `sync_event` protocol bumps
  **v1.0.0 → v1.1.0** (additive optional field, backward-compatible). The
  Rust `ipc.rs` event and both UI decoders add the optional field; Swift uses
  `decodeIfPresent`, C# tolerates its absence.
- `AppModel` threads `eta_secs` into the `.syncing` phase; `DeviceRow`
  renders `~M min left` (formatted with `Duration`/`RelativeDateTimeFormatter`)
  only when present.

Rendering aside, `DeviceRow` is a pure function of `AppModel` — the reducer
stays the single source of truth and stays unit-testable.

## 7. First-run / not-configured

With a Dock app, first launch opens the main window. When
`model.needsFirstRunSetup` is true, `MainWindow` shows a centered setup
call-to-action (reusing the existing setup flow via
`AppDelegate.presentSetup`), and the sidebar's Library/Device content is
suppressed until configured. The existing auto-present-once latch
(`didAutoPresentSetup`) is preserved. The setup wizard itself is unchanged.

## 8. Menu-bar extra (condensed)

`MenuContent` slims down now that it's secondary:

- Glanceable status line (reuses the phase text it already renders).
- **Open Classick** (focuses/opens the main window) — new, at top.
- Sync Now / Pause / Resume / Cancel per phase (unchanged).
- **Rescan** — the manual escape hatch (`scan_library`), kept only here.
- Settings… / Check for Updates… / Quit (unchanged).

"Choose Music…" is removed (its browser is now the always-present Library
view); the button opens the main window on the Library item instead.

## 9. File watcher (Rust, daemon)

A new daemon component `daemon/library_watcher.rs`, a sibling to
`device_watcher.rs`:

- Uses the **`notify`** crate (cross-platform: FSEvents on macOS,
  ReadDirectoryChangesW on Windows, inotify on Linux). Quick health check:
  `notify` is the de-facto standard, actively maintained, widely adopted.
- Watches the configured source root (from `config.toml`); re-arms when the
  source changes via `save_config`; disarmed when no source is configured.
- **Debounces** bursts (~1 s quiet period) — large file operations emit many
  events — then runs the **existing incremental refresh**
  (`library_index::stale_entries` + `update_index`; only changed/new files are
  re-probed) and broadcasts the resulting `library_update` to connected UIs.
- Runs on startup too: a single incremental refresh so the library is current
  the moment the app opens, without a user action.
- Lifecycle: owned by the daemon runtime; started with the runtime, stopped on
  shutdown (no orphaned watch threads). Errors are logged and degrade to
  "manual Rescan still works", never crash the daemon.
- Scanning still runs under the daemon's existing shared operation guard so a
  watcher-triggered refresh can't race a sync/backfill (consistent with the
  scan-subprocess decision from the library-selection design).

## 10. Wire protocol delta

Exactly one change: inner `sync_event` `track_start` gains optional
`eta_secs`. Everything else (`library_update`, `save_selection`,
`forget_ipod`, `backfill_rockbox`, history) is already on the wire and reused.
`docs/ipc-protocol.md` is updated; the inner sync-event version bumps to
v1.1.0 (minor, additive). The daemon-pipe protocol version is unchanged.

## 11. Testing

- **Swift (reducer-centric):** `AppModel` remains the single source of truth
  and keeps its unit tests. Add coverage for: `eta_secs` threaded into
  `.syncing`; retained `[HistoryEntry]`; derived per-view state. The
  `DeviceRow`/views are thin and driven by the reducer, so logic is tested at
  the reducer, not via UI snapshotting.
- **Swift (lifecycle):** a small test/asserted behavior that
  `applicationShouldTerminateAfterLastWindowClosed` is `false` and reopen is
  wired.
- **Rust (watcher):** unit/integration test in the daemon suite (following the
  `daemon_runtime_integration.rs` sandbox pattern): touching a file in a
  temp source root triggers, after debounce, exactly one incremental refresh
  and a `library_update` broadcast; no source configured → no watch.
- **Rust (ETA):** unit test of the throughput/ETA calc (ring buffer → omitted
  before 2 samples → monotone-ish estimate after).
- **Wire:** `WireCodecTests` gains a case decoding a `track_start` with and
  without `eta_secs`.

## 12. Rollout / build

No packaging changes beyond `Info.plist` (`LSUIElement`). Existing
`bundle.sh` / `scripts/release-macos.sh` / Sparkle appcast flow is unchanged.
`notify` is a new Rust dependency (workspace `Cargo.toml`); it builds on all
three platforms. Ship as a normal minor app release (e.g. 0.4.0) once
on-device validated.

## Open risks

- **FSEvents coalescing / missed events** on network or unusual volumes — the
  manual Rescan escape hatch and the startup incremental refresh are the
  mitigations; the watcher is best-effort, not a correctness dependency.
- **Auto-save churn** — debounce + the existing "seed once" latch prevent
  save storms and edit clobbering; verify the debounce window feels right on
  device.
- **Window/menu-bar dual presence** confusing users — mitigated by "Open
  Classick" and close-≠-quit being the conventional Mac behavior for
  menu-bar-plus-window apps (e.g. many sync clients).
