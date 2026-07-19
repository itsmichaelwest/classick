# Library, Playlists & Device Pages — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-17
**Scope:** The UX/model foundation the user chose to solve before further
device-feature work: playlists as first-class objects (manual + smart,
created in Classick), a device-content model of `scope ∪ subscribed
playlists`, per-device everything (selection, auto-sync, Rockbox mode), and
a restructured macOS app around a canonical per-device configuration
surface. Supersedes the shared/"mirror" selection model shipped in the
trust package.

**Visual source of truth:** Figma file `5hOlDaKWg7UPFNnRY2fznV`, frames
`3:3773` (device Music page) and `4:6349` (device Settings page), built on
Apple's macOS 26/27 Liquid Glass component library. The frames override any
prose ambiguity about layout; this spec records the semantics they imply.

## Problem & goal

The trust package shipped shared-vs-custom selection, and the first design
review of multi-device UX exposed its flaw: without playlists, "shared"
carries almost no weight (different-sized iPods want different *amounts* of
music, which artist/album/genre vocabulary can't express), and a Library
view that edits per-device state means one view whose meaning silently
changes with the active device. The user's requirement: solve the
library↔device hierarchy before any further device-feature work — without
reimplementing an iTunes-style library manager.

**Goal:** one coherent model — Library = look at music; Playlists = name
sets of music; a device page = decide & see what syncs — with playlists as
the compositional layer that makes multiple devices coherent, while
Classick stays a sync tool: every byte of state either derives from the
source folder or is a small, portable, user-ownable file.

## Decisions (brainstorm + Figma review, 2026-07-17)

| Question | Decision |
|---|---|
| Source library writes | **Never.** The source folder is read-only, permanently. Rejected: writing .m3u files into the library (SMB, Lidarr, user trust). |
| Where playlists live | **App state dir, mirrored to the iPod** each sync so the device carries them across machines (adopt-if-empty on a new host). Rejected: source-folder files (read-only rule); iPod-only (device loss = curation loss). |
| Playlist kinds (v1) | **Manual + smart, created in Classick.** Manual = ordered track list, stored as `.m3u8` (relative-to-source paths, portable). Smart = declarative rules file, host-evaluated at sync time into a static device playlist. |
| Device content model | **`scope ∪ subscribed playlists`.** Scope = Entire library / Selected items / All except selected (three modes — exclude survives). A subscribed playlist's tracks ALWAYS sync, even outside scope, and the playlist appears in the iPod's Music menu. A device with an empty Include scope + subscriptions = playlists-only device, for free. |
| Shared/mirror selection | **Retired.** Selection is purely per-device. The trust package's `custom_selection` machinery becomes the only path; the shared `selection.json` seeds each configured device once, then is ignored. |
| Per-device settings | **Auto-sync-when-connected and Rockbox compatibility move from global daemon settings to per-device** (as frame 4:6349 shows). Global values migrate to the configured device; daemon plug-in gating reads the device's own flag. |
| Canonical surface | **The device page is the only place sync intent is displayed and edited.** Library view returns to pure browsing. Drag-drop / context-menu "add to iPod / add to playlist" are additive *gestures* that check the same boxes — explicitly deferred, v2+, no model impact. |
| Two-level navigation | **Sidebar disclosure, not nested tabs**: a connected device expands into **Music** and **Settings** children (frame 3:3773). Page switcher = sidebar; facet switcher = one segmented control. Parent row click selects Music (default child); the chevron alone toggles disclosure. |
| Disconnected devices | Children remain visible; pages render last-known state, editable ("changes apply on next sync"), Sync Now disabled. Sidebar row shows the device dimmed. |
| Mode↔content contract | The Music page's browser is always the same rich component; **checkboxes render only in the two selective modes**. Entire library = read-only browse of what will be (and is) on the device. One caption line under the segmented control states the contract. |
| Mode-switch safety | **Switching Entire library → Selected items seeds the checkboxes from the device's current contents** (zero-diff at the moment of switching). No mode change can silently plan a mass removal. |
| Capacity bar | Persistent, floating (frame 3:3773); re-projects live as checkboxes/subscriptions change (fit-engine estimate); deferral results ("N albums didn't fit") surface in its caption. |
| Design language | Liquid Glass components on macOS 26+; standard AppKit/SwiftUI controls are the same components pre-26, so macOS 15 floor holds with graceful fallback. No custom re-implementations of system controls. |

## 1. Core: playlist model & storage

New `crates/classick/src/playlist.rs` (+ `playlist_rules.rs` if the rule
evaluator warrants its own file):

- **Manual playlist:** ordered list of source-relative track paths. File:
  `<config>/classick/playlists/<slug>.m3u8` — `#EXTM3U` header, UTF-8,
  paths relative to the source root, `#PLAYLIST:<display name>` comment.
  Portable: any tool can read/edit it; Classick tolerates external edits
  (reload on change).
- **Smart playlist:** `<config>/classick/playlists/<slug>.rules.json` —
  `{ version, name, match: "all"|"any", rules: [{field: artist|album|
  genre|year, op: is|contains|gte|lte, value}], limit: {bytes?|tracks?},
  order: "recently_modified"|"random_stable"|"alpha" }`. Evaluated
  host-side against the library index at sync/preview time; the device
  receives a static playlist. `random_stable` uses a persisted seed so
  re-syncs don't churn the device.
- **Resolution:** a playlist resolves to `Vec<PathBuf>` against the current
  walk; missing/deselected-from-library files are skipped with a per-track
  log line and a count in the sync summary (never an error).
- **iPod mirror:** each successful sync writes all playlist files +
  `subscriptions.json` to `iPod_Control/classick/playlists/` on the device.
  On a host with an empty playlists dir and a connected device carrying a
  mirror: adopt (copy back) once, log it. If both exist and differ: local
  wins, log a warning (full conflict semantics are spec #4's problem —
  the file format is per-playlist + timestamped precisely so that merge is
  tractable later).

## 2. Core: per-device config v2

`devices/<serial>/` gains (superseding pieces of the trust package):

- `selection.json` — now the ONLY selection (per-device). Same
  `SelectionMode {All, Include, Exclude}` + rules. Migration: if the shared
  root `selection.json` exists and the device has none, copy it in (the
  existing seed helper); the shared file is then ignored (left in place).
  The `custom_selection` wire/config field is deprecated: tolerated on
  decode, no longer consulted.
- `subscriptions.json` — `{ version, playlists: [<slug>...] }`.
- `settings.json` — `{ version, auto_sync: bool, rockbox_compat: bool }`.
  Migration: seeded once from the global `DaemonSettings` values; the
  global fields remain for older clients but the daemon reads the device
  file when present. Plug-in auto-sync gating uses the device's flag.

## 3. Core: sync planner changes

In the apply-loop plan phase (module boundaries per the plan; apply_loop.rs
must not grow — this lands alongside the queued file-split follow-up):

- Effective track set = `scope_filter(walk) ∪ resolve(subscribed playlists)`.
  Union is computed on source paths before the manifest diff; everything
  downstream (fit engine, artwork, checkpoints) is unchanged.
- Fit-engine note: subscribed-playlist tracks are Adds like any other; the
  album-atomic rule applies. Deferral reporting distinguishes "outside
  scope, from playlist P" only in logs, not the wire.
- **iTunesDB playlists:** new FFI surface in `ipod/db.rs` —
  `itdb_playlist_new` / `itdb_playlist_add_track` / `itdb_playlist_remove` —
  wrapped safely: after the track loop, reconcile device playlists to match
  subscriptions (create/update/delete Classick-managed playlists by name;
  never touch foreign playlists). Rockbox reads the same files via its own
  scan, so playlist parity on Rockbox = the existing self-describing-file
  story plus (v2, deferred) `.m3u8` export to a visible folder.

## 4. Wire protocol (daemon 1.5.0 → 1.6.0, additive)

- Commands: `list_playlists`, `get_playlist {slug}`, `save_playlist
  {manual|smart payload}`, `delete_playlist {slug}`, `get_device_config
  {serial}` / `save_device_config {serial, selection?, subscriptions?,
  settings?}`, `preview_device {serial}` (selection+subscription-aware
  fit/capacity preview: selected bytes, playlist-outside-scope bytes,
  projected free).
- Events: `playlists_update`, `device_config_update`. Existing
  `get_selection`/`save_selection` remain, operating on the configured
  device (deprecated in docs, kept for compat).
- `docs/ipc-protocol.md` updated in the same commits as wire types, per
  repo rule. Windows client ignores all of it (major-check only).

## 5. macOS app restructure

Sidebar (frame 3:3773): `Library` · `Devices` section (device rows with
disclosure → **Music**, **Settings**; eject affordance; disconnected =
dimmed, collapsed by default) · `Playlists` section (+ button; playlist
rows) · `History`.

- **Library page:** browse-only (checkbox affordances removed). Facets:
  Artists / Albums / Genres via the segmented control; sizes shown per row.
- **Device › Music page** (frame 3:3773): toolbar = title + "Last synced"
  subtitle, `Sync [mode ▾]` pop-up (three modes), prominent **Sync Now**;
  segmented control Artists | Albums | Genres | Playlists; content = rich
  browser (artist column-headers, album rows with checkbox + track count +
  size) — checkboxes only in selective modes; Playlists segment = the
  subscriptions checklist with per-playlist track counts and an "always
  synced, shown on the iPod" caption; floating capacity bar with live
  projection + deferral caption. Empty states: no source configured / scan
  in progress / library empty / device empty — each one line + action.
- **Device › Settings page** (frame 4:6349): info form (Name, Capacity,
  Synced X of Y, Last synced); toggles form (Sync automatically when
  connected, Rockbox compatibility mode, Force update artwork and metadata
  → **Update Now**); destructive form (**Replace Library…** — existing
  typed-confirmation flow relocates here); Remove iPod form (existing
  ForgetIpod).
- **Playlist page** (sidebar click): manual — track list, reorder, remove,
  **Add Songs…** opens a library-browser picker sheet (this is the
  canonical editor, not a deferred gesture); smart — rule builder form +
  live-updating resolved preview (count + bytes). Delete playlist in a
  toolbar menu; deleting a subscribed playlist warns with the affected
  device names.
- The menu-bar extra keeps its current glance/quick-sync role; no changes
  beyond renamed actions if any.

## 6. Error handling

- Playlist file unreadable/corrupt → skipped with a visible per-playlist
  error in the Playlists section; sync proceeds without it (fail-open on
  scope, fail-visible on playlists — a silently-vanishing playlist on the
  iPod would read as data loss).
- Smart rules matching zero tracks → valid empty playlist (shown as such).
- iPod mirror write failure → warn, never fails the sync.
- Foreign (non-Classick) playlists on the device are never modified or
  deleted.

## 7. Testing

- Rust unit: M3U8 round-trip (incl. relative-path resolution, external-edit
  tolerance, BOM/CRLF); rules evaluation matrix (match any/all, limits with
  album-atomicity interplay, random_stable determinism); union planner
  (scope ∪ playlists, dedup, empty-Include + subscriptions); migrations
  (shared selection seed, global→device settings, adopt-from-mirror);
  reconcile-device-playlists (create/update/delete, foreign untouched).
- Rust integration (fake-mount harness, `Music/F00` pre-created):
  subscribed playlist syncs outside scope and appears in a reparsed DB
  playlist; playlist removal on unsubscribe; mirror write + adopt.
- Swift: reducer tests for device-config state, subscription toggles,
  mode-switch seeding, empty states; wire codec round-trips for 1.6.0.
- On-device gate: playlists appear in the iPod Music menu (stock firmware +
  Rockbox); playlists-only device; mode-switch seeding on real contents;
  cross-machine adopt via the device mirror (can be simulated with a
  second config dir).

## Non-goals

- Playback, ratings, play counts, sort orders — no library database, ever.
- Drag-drop and context-menu gestures (deferred, additive, v2).
- Rockbox tagcache/.tcd generation; on-device smart evaluation.
- Writing anything into the source library.
- Windows UI (separate catch-up spec; wire is additive).
- Multi-device *simultaneous* daemon operation (config is per-device now;
  the daemon still drives one connected device at a time).

## Open items for implementation planning

- Slug rules for playlist filenames (unicode names → filesystem-safe slugs,
  collision handling).
- `itdb_playlist_*` FFI audit (which symbols are in the bindgen allowlist
  already; smart-playlist enum gap noted in Phase 0 learnings).
- Whether `preview_device` reuses the existing selection-preview machinery
  or replaces it.
- Liquid Glass availability gating pattern (macOS 15 floor, 26+ effects).
- Trust-package interaction: this spec builds ON the unmerged
  `trust-package` branch (per-device state dirs, fit engine, Replace flow);
  its plan must sequence after that branch merges.
