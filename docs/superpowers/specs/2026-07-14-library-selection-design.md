# Library Selection (choose what syncs) — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-14
**Scope:** Let the user choose which music syncs to the iPod — by Artist,
Album, or Genre — via a browser window in the macOS app, an iTunes-style
mode picker (entire library / only selected / all except selected), a new
library-index scan mode in the core, and additive daemon-protocol commands.
macOS-first; the core + protocol work is cross-platform so the Windows UI
inherits it later.

## Problem & goal

Today classick syncs everything under the configured source folder. That is
the right default but offers no granularity: users with libraries larger than
the iPod (or with music they don't want on it) have no way to scope the sync
short of restructuring folders or abusing `_excluded` directories.

**Goal:** a lightweight library browser in the macOS app where the user
checkboxes Artists, Albums, and Genres, with iTunes-style semantics, and the
sync engine honors that selection — including removing deselected music from
the device on the next sync.

## Decisions (brainstorm 2026-07-14)

| Question | Decision |
|---|---|
| Selection semantics | Three-way mode: `all` (default, today's behavior) / `include` (only checked syncs) / `exclude` (checked stays off). Artist/genre rules auto-cover future albums. |
| Granularity | Artist + Album + Genre. No per-track checkboxes in v1. |
| Deselect behavior | Deselected tracks become ordinary `Remove` actions on the next sync; surfaced via the existing review/summary counts and a live impact preview in the browser footer. `no_delete` suppresses them like any removal. No new safety machinery. |
| Design ceiling | SMB/NAS source + ~20k tracks: incremental tag-index cache, scan progress UI, aggregated (not per-track) wire payload. |
| Index architecture | **Scan subprocess** (`classick --ipc-mode --scan-library`), spawned by the daemon under the shared sync/backfill guard — consistent with every other heavy operation, crash-isolated, reuses forwarded-event progress plumbing. Rejected: in-daemon scanner thread (breaks thin-supervisor pattern, a tag-parse panic kills the daemon); Swift-side AVFoundation scan (duplicated tag logic, values would diverge from what lands in the iTunesDB, nothing reusable for Windows). |
| Browser layout | Outline list (iTunes-style): Artists/Genres tabs, artists expand to albums, mixed-state checkboxes. Rejected: Finder-style column browser (murky cross-column checkbox semantics), album-art grid (requires extracting/thumbnailing embedded art for the whole library). |

## 1. Selection model

### `selection.json`

Lives in the config dir beside `config.toml`. Atomic write (tmp + fsync +
rename), same as the manifest. Tool-written only — the browser UI is the
editor; nobody hand-edits it. JSON (not TOML) because it is machine state
whose `{mode, rules}` payload travels over the daemon socket verbatim — the
same serde types serialize disk and wire.

```json
{
  "version": 1,
  "mode": "all",
  "rules": [
    { "kind": "artist", "name": "Boards of Canada" },
    { "kind": "album",  "artist": "Aphex Twin", "album": "Drukqs" },
    { "kind": "genre",  "name": "Ambient" }
  ]
}
```

- `mode: "all"` — no filtering. Rules are kept but inert, so flipping modes
  preserves checkbox state. Absent, unreadable, or corrupt `selection.json`
  degrades to `all` with a logged warning — **never** to "sync nothing".
- `mode: "include"` — a track syncs iff it matches ≥ 1 rule.
- `mode: "exclude"` — a track syncs iff it matches 0 rules.
- Auto-inclusion of new content falls out naturally: an artist rule matches
  any track by that artist, including albums that appear later. Same for
  genre rules.

### Matching semantics

- Case-insensitive (Unicode casefold) comparison against the index's tag
  values.
- "Artist" means **album artist, falling back to track artist** — the same
  grouping the browser displays, so what the user checks is exactly what
  matches. Compilations group under "Various Artists" via album_artist.
- Album rules key on the `(artist, album)` pair (artist as defined above).
- Missing tags bucket into the empty string, rendered as "Unknown Artist" /
  "No Genre", and those buckets are checkable like anything else (a rule
  with `name: ""` matches them).
- Genre strings match verbatim in v1 — no delimiter-splitting of
  multi-genre tags ("Electronic; Ambient" is one genre value). Documented
  limitation.

## 2. Library index & scan mode

### `library-index.json`

Config dir, atomic write. Per-file cache of the tag data selection needs:

```json
{
  "version": 1,
  "source_root": "/Volumes/music",
  "scanned_at_unix_secs": 1784040000,
  "files": {
    "<abs path>": {
      "mtime": 1783000000, "size": 31457280,
      "artist": "…", "album_artist": "…", "album": "…",
      "genre": "…", "title": "…", "duration_ms": 214000
    }
  }
}
```

Keyed to `source_root`: pointing the app at a different folder invalidates
the whole cache and forces a full rescan.

### `--scan-library` mode

New CLI mode alongside `--apply` / `--backfill-rockbox`:

1. Walk the source with the existing `source::walk()` (same skip rules,
   same SMB retry behavior).
2. For each entry: `(mtime, size)` matches the cached record → reuse it
   (stat-only, no file open). Miss → probe tags via **lofty** and update the
   record. Drop records for vanished files.
3. Write the index atomically; emit progress over the existing IPC event
   vocabulary (`track_start` / `track_done` / `finish`) so the daemon's
   forwarding plumbing and the UI's progress rendering are reused as-is.

lofty on **all** platforms (it is already an ungated dependency): no
per-file ffprobe shell-outs on Windows later, and on macOS the browser shows
exactly the values the iTunesDB gets because it is the same probe seam
(`transcode/macos_probe.rs`). First scan over SMB is the only expensive one;
rescans are stat-only plus deltas — the manifest fast-path trick applied to
tags. A lofty parse failure on one file logs + skips (bucketed as unknown),
mirroring the walker's skip-don't-abort policy; a panic is contained by the
subprocess boundary.

## 3. Sync integration

In the orchestrator, immediately after `source::walk()` and before
`manifest::diff()`:

```
selection::filter(sources, &selection, &mut index) -> Vec<SourceEntry>
```

- Loads `selection.json` (default `all` on any failure) and
  `library-index.json`.
- `mode: all` → returns sources unchanged (zero-cost today's behavior).
- Otherwise evaluates rules per file using index tags. A file not yet in
  the index (added since last scan) is probed inline via the same lofty
  seam and folded into the index, which is saved back at the end — the
  sync self-heals the fresh-files gap between scans.
- Everything downstream is untouched: deselected-but-on-iPod tracks fall
  out of the filtered source list, so the diff emits ordinary `Remove`
  actions; the existing summary/review flow surfaces the counts;
  `no_delete` suppresses them like any other removal. No new `Action`
  variants, no apply-loop changes.

New Rust module: `crates/classick/src/selection.rs` (types, serde,
matching, filter). Scan mode entry point: `crates/classick/src/scan.rs` (or
a small module under `orchestrator`'s mode dispatch — implementer's call,
keeping files ≤ ~500 LOC).

## 4. Daemon protocol v1.4.0

Purely additive over v1.3.0. Daemon `hello` emits
`protocol_version = "1.4.0"`. `docs/ipc-protocol.md` gains a "Daemon
v1.4.0" section mirroring the tables below (that doc remains the source of
truth; this spec summarizes).

### New commands (UI → daemon)

| Type | Fields | Behavior |
|---|---|---|
| `get_library` | (none) | Replies `library_update` from the cached index. No index yet → `library_update` with `scanned_at_unix_secs: null` and empty collections. |
| `scan_library` | (none) | Spawns the scan subprocess under the shared sync/backfill state-machine guard. No-op (log + drop, `backfill_rockbox` style) if busy or unconfigured. On subprocess `finish`, daemon reloads the index and broadcasts a fresh `library_update` to all clients. |
| `get_selection` | (none) | Replies `selection_update`. |
| `save_selection` | `mode`, `rules` | Persists `selection.json` atomically; replies `selection_update`; broadcasts refreshed `status_update` (see `library_count` below). |
| `preview_selection` | `mode`, `rules` | Pure computation, no persistence: evaluates the hypothetical selection against index + manifest, replies `selection_preview`. UI debounces this while the user toggles checkboxes. |

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `library_update` | `source_root`, `scanned_at_unix_secs` (null = never scanned), `artists[]` — each `{name, albums[]: {name, genre?, tracks, bytes}}` — `genres[]: {name, tracks, bytes}`, `total_tracks`, `total_bytes`. Aggregated, never per-track; a 20k-track library is a few hundred KB on one line. An album's `genre?` is display-only (the most common genre among its tracks; omitted on tie/absence) — genre *rules* always match per-track against `genres[]` values, so mixed-genre albums partially match a genre rule. |
| `selection_update` | `mode`, `rules` — mirror of `selection.json`. |
| `selection_preview` | `selected_tracks`, `selected_bytes`, `adds`, `removes` — adds/removes from intersecting rule evaluation with the manifest, powering "next sync: +120 / −214". |

### Touched existing surfaces

1. `status_update.state` gains `"scanning"` alongside `idle`/`syncing`. The
   protocol doc gets an explicit clause: **unknown state values MUST be
   treated as `idle`** (matching existing unknown-message tolerance), so
   older clients don't break.
2. `status_update.library_count` becomes the **selected** count — the "Y"
   in "X of Y synced" now means "tracks the current selection wants on the
   iPod". Under `mode: all` the value is unchanged, so existing setups see
   no difference.

### Daemon internals

New module `daemon/library.rs`: loads + aggregates the index, evaluates
rules for previews, owns selection persistence. Scan spawn reuses
`sync_orchestrator`'s subprocess scaffolding with the `--scan-library` flag,
exactly as `backfill_rockbox` did. The C# `DaemonCommand`/`DaemonEvent`
mirrors gain the new types when the Windows UI adopts the feature; until
then unknown events are ignored per protocol forward-compat rules.

## 5. macOS UX

### Entry point

"Choose Music…" item in the menu-bar dropdown beside the sync controls.
Opens a single-instance Choose Music window (own `NSWindow` via the
`SetupWindow` controller pattern; re-invocation brings it to front).
Disabled with an explanatory tooltip until a source folder is configured.

### Window layout (approved mockup: outline list)

- **Top bar:** three-way mode picker (segmented: *Entire library / Only
  selected / All except selected*) + search field (filters both tabs).
- **Tabs:** Artists | Genres.
  - *Artists:* outline of artist rows (checkbox, disclosure, name, "N
    albums · N tracks · N GB") expanding to album rows (checkbox, name,
    "N tracks · N MB"). Mixed-state checkbox on partially-selected artists.
  - *Genres:* flat checklist with per-genre track/byte counts.
- **Footer:** live impact — "2,340 of 9,214 tracks · 14.2 GB", device
  capacity bar (red + warning when selection exceeds capacity; warn, don't
  block Save — sync already handles disk-full), "next sync: +120 / −214",
  scan freshness ("Library scanned 2 days ago · Rescan"), Cancel / Save.

### Scan states

On open the app sends `get_library`:

- **Never scanned:** empty state — "Classick needs to read your library's
  tags once" — with a **Scan Library** button (`scan_library`). Progress
  renders from forwarded `track_start`/`track_done` ("Scanning… 840 of
  9,214") in the same style as sync progress.
- **Index exists:** browser renders immediately from cache; Rescan is a
  one-click refresh (cheap: stat-only + deltas). Browsing never blocks on a
  scan.
- **Busy (scan or sync running):** browsing works from cache; Rescan and
  sync affordances reflect the busy state from `status_update`.

### Checkbox semantics

Checkboxes edit a **local draft** of `{mode, rules}`; nothing persists
until Save.

- Checking an artist row = one artist rule (covers future albums).
- Checking individual albums = album rules. Hand-checking **all** of an
  artist's albums collapses to the artist rule (iTunes intuition: future
  albums follow). Unchecking one album under a checked artist expands the
  artist rule into explicit album rules minus that album.
- Genres tab: one genre rule per row.
- In `include` mode a check means "sync this"; in `exclude` mode the same
  checkboxes mean "leave this off", with header copy making it explicit
  ("Checked items will NOT be synced"). Switching modes keeps the draft
  rules — they flip meaning. In *Entire library* mode the browser grays out
  but keeps state.

### Live footer preview

Debounced (~300 ms) `preview_selection` with the draft rules. Device
capacity comes from the storage info the daemon already reports for the
connected iPod.

### Save flow

- **Save:** `save_selection`, close window; if an iPod is connected and the
  preview showed pending changes, surface the standard "Sync now?"
  affordance. Removal counts re-surface through the existing review flow in
  Review mode.
- **Cancel:** discard draft. Closing with unsaved changes → standard
  "Save / Discard / Cancel" sheet.

### Menu-bar surface

The dropdown's "X of Y synced" line reads Y from the selected count (per
the `library_count` semantics change) and gains a "Selection: 2,340 tracks"
line whenever mode ≠ all — a filter being active is always visible.

## 6. Edge cases

- **Tag edits change rule matches.** Renaming an album/artist in the source
  makes old rules stop matching: track drops off on next sync (and shows in
  remove counts). Accepted v1 behavior; the browser reflects the new names
  after a rescan.
- **Stale rules.** Rules referencing artists/albums no longer in the index
  are kept (the music may come back) but rendered nowhere; they're inert.
  The browser only shows what the index contains.
- **Selection vs. `--rebuild-manifest` entries.** `source_known: false`
  entries are already preserved untouched by the diff; selection filtering
  happens on the source list, so it cannot touch them.
- **Corrupt/missing files degrade safely.** `selection.json` → `mode: all`;
  `library-index.json` → treated as never-scanned (sync inline-probes as
  needed, browser prompts for a scan).
- **Scan vs. sync concurrency.** Shared state-machine guard: never
  concurrent, no SMB contention, no index-file races. Sync's inline probe
  covers files added between scans.
- **Windows.** Core + protocol are cross-platform from day one; only the
  WinUI browser window is future work.

## 7. Testing

- **Rust unit:** `selection` rule matching (modes, casefold, album-artist
  fallback, empty-tag buckets, verbatim genre); filter → diff producing
  Adds/Removes on mode flips; corrupt-selection → `all` fallback.
- **Rust unit (scan):** incremental cache (hit = no probe, miss = probe,
  vanished = dropped, `source_root` change = full rescan); atomic write.
- **Daemon integration** (pattern: `daemon_runtime_integration.rs` sandbox):
  `scan_library` guard no-ops while syncing; `save_selection` round-trip;
  `preview_selection` counts against a fixture index + manifest;
  `library_update` broadcast after scan finish; `status_update` gains
  `scanning` + selected `library_count`.
- **Swift unit:** `WireCodec` coverage for the five new commands + three
  new events; AppModel reducer tests for draft-rule editing (mixed state,
  artist-rule collapse/expand), mode flips, and scan-state transitions.
- **End-to-end (manual, on-device):** include-mode subset sync; deselect →
  removal with review counts; new album auto-arrives under a checked
  artist; exclude-mode genre.

## 8. Compatibility & rollout

- No selection file → `mode: all` → byte-identical behavior to today.
- Protocol bump is additive minor (1.3.0 → 1.4.0); older UI builds ignore
  the new events, and the one enum extension (`scanning`) is covered by the
  new unknown-state-means-idle clause.
- Manifest format untouched. Config format untouched.
- Ship order: core (selection + scan + filter) → daemon (commands/events)
  → macOS UI. Each stage is independently landable behind the invisible
  default.
