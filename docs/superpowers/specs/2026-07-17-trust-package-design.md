# Trust Package (P0 trust & correctness) — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-17
**Scope:** The first of five specs derived from the 2026-07 PRD gap analysis:
invisible iTunesDB backup + automatic restore, free-space-aware sync
(album-atomic first-fit), per-device sync state with shared/custom selection,
an empty-source hard error, one explicit "Replace Library" destructive action,
and an artwork-robustness + Apple-compatibility workstream (diagnosis-first).
Core-first: all behavior lives in the Rust core/daemon; the macOS app gains
thin UI. The Windows app safely ignores the additive wire changes (its
catch-up is a later spec).

**Base branch:** `main` after the `macos-desktop-app` (0.4.0) branch merges —
the UI touchpoints land in the Device view that branch introduces.

## Problem & goal

The PRD's ship-blocking trust bar: users' single biggest fear (per forum
research and our own incident history) is a sync tool corrupting or wiping the
only copy of their iPod's library. Today: the session-start DB backup exists
but recovery is a manual file copy; a sync larger than the device fails
mid-write; the manifest silently diffs whichever iPod is plugged in against
another device's state; a degenerate source walk (empty dir) would plan a full
wipe; mass removal is only ever implicit; and album art still goes missing in
ways the sync never reports.

**Goal:** a user should never have to think about any of this. Backups and
recovery are invisible and automatic. Syncs fit the device gracefully. State
is per-device by construction. The only way to mass-remove music is a
clearly-labeled explicit action. Art either displays or the sync says why not.

## Decisions (brainstorm 2026-07-17)

| Question | Decision |
|---|---|
| Backup depth | **Single rolling slot**, session-start, on-device (`iTunesDB.classick-backup`) — unchanged mechanics. Rejected: two-slot last-known-good, N-deep snapshots (more machinery than the failure modes warrant). |
| Restore visibility | **Fully invisible.** Auto-restore on parse failure; no GUI surface at all; `--restore-db-backup` CLI as the only manual escape hatch. Rejected: Device-view restore button (invites misuse; user directive: "the user should not have to worry about this. Ever."). |
| Over-capacity sync | **Sync what fits, report the rest.** Album-atomic first-fit in deterministic diff order. Rejected: refuse-to-start (turns overnight auto-sync into a silent no-op); warn-and-fail-when-full (ugly partial albums). |
| Fit priority | No user-facing priority system. Predictability guarantees instead: never split an album; deterministic order; the report points at the Library view as the fix-path. |
| Device identity | **Per-device manifests** keyed by serial. Rejected: single manifest + mismatch guard (leaves multi-iPod unsupported). |
| Selection scope | **Per-device with shared "mirror" default.** Root `selection.json` = shared selection every device follows; a device may opt into a custom copy. |
| State layout | **`devices/<serial>/` directory** under the config dir. Rejected: serial-suffixed flat files (suffix parsing, noisy config dir). |
| Mass-removal guard | **None.** Degenerate inputs become hard errors (missing source, empty walk); everything else auto-applies. Sync is always automatic. Rejected: >50 %-removal confirmation gate (breaks unattended sync for a case better handled as an error). |
| Explicit destruction | One **Replace Library** action (UI + CLI) is the only mass-removal path. |
| Apple compatibility | **First-class verified requirement**, not a documented limitation. Prior "iTunes always rejects libgpod DBs" lore is treated as stale; verify fresh on-device. |
| Artwork | **Diagnosis-first workstream inside this spec** with a permanent audit tool and an invariant covering every sync-exit path. |

## 1. On-disk state layout & migration

```
<config dir>/classick/            (~/Library/Application Support | %APPDATA%)
├── config.toml                   unchanged (+ selection_mode under ipod_identity)
├── history.json                  unchanged
├── library_index.json            unchanged (source-scoped)
├── selection.json                the SHARED selection (default for every device)
└── devices/
    └── <serial>/
        ├── manifest.json         per-device sync state (moved from root)
        └── selection.json        present only if this device opted into custom
```

- **Serial** is the normalized `ipod_identity.serial` the daemon already uses.
  Manifest load/save always takes the connected device's serial; no code path
  reads a manifest for "whichever iPod is plugged in."
- **Effective selection:** `devices/<serial>/selection.json` when the device's
  `selection_mode = "custom"`, else root `selection.json`. Switching to custom
  copies the shared file as a seed; switching back leaves the custom file
  dormant (not deleted). Default is `shared` — today's behavior, zero change
  for existing users.
- **Migration** (one-time): if root `manifest.json` exists and `devices/`
  doesn't, move it to `devices/<configured-serial>/manifest.json`. If no
  device is configured yet, migrate on the next successful device connect.
- `Manifest.ipod_serial` is finally written (belt-and-braces; the directory is
  the primary key). On load, a serial mismatch logs a warning and treats the
  manifest as foreign (falls back to `--rebuild-manifest` guidance) rather
  than diffing against the wrong device.

## 2. Invisible backup & automatic restore

**Backup (unchanged):** `backup_itunesdb()` writes the single rolling
`iPod_Control/iTunes/iTunesDB.classick-backup` at session start via the
existing `.tmp` + atomic-rename path. No UI, no setting. One line of
reassurance copy in wizard/Settings is the only visible trace.

**Auto-restore:** when `OwnedDb::open` fails to parse the live iTunesDB:

1. Validate the backup exists **and itself parses** (open with libgpod first —
   never replace a corrupt DB with an unvalidated file).
2. Move the corrupt DB aside to `iTunesDB.corrupt` (single forensic slot,
   overwritten each time).
3. Atomically copy the backup over `iTunesDB`, re-open, proceed with the sync.
4. Record it: `tracing` warn, a note on the sync-history entry, and the
   existing IPC log-line event. No prompt, no modal.

If the backup is missing or also unparseable: fail as today, with improved
copy pointing at the real remedies (`--rebuild-manifest`, `--restore-db-backup`).

**Post-restore consistency is free:** the restored DB is at most one session
stale; the existing `reconcile_with_disk` sweep removes orphaned files and the
manifest diff re-adds anything missing.

**Manual escape hatch:** `--restore-db-backup` validates the backup parses,
copies it over the live DB, prints what it did, exits. Covers failures only a
human can observe (e.g. firmware shows an empty library while the DB parses).

**Non-feature:** no heuristic restores (e.g. "track count fell to zero") —
restore triggers on parse failure only. The empty-source error and the
explicit Replace action close off the legitimate paths to a suddenly-empty DB.

## 3. Free-space-aware sync (fit engine)

Runs **entirely in the Rust core**, in the sync subprocess between plan
computation and the apply loop. CLI, macOS, and (later) Windows get identical
behavior; UIs only render the report.

**Inputs**
- Device free bytes at plan time. Windows: existing `GetDiskFreeSpaceExW`
  helper moves from daemon into core. macOS: `statfs` equivalent (the Swift
  app's display-side storage math is untouched).
- Estimates: passthrough = exact source bytes; transcode = source bytes
  (documented over-estimate — ALAC of the same PCM ≈ FLAC size, and hi-res
  sources only shrink at 16/44.1).
- **Reserve:** `max(512 MB, 2 % of volume)`, a named constant with a doc
  comment (artwork/DB growth + estimate error; never fill FAT32 to 100 %).

**Fit pass (plan time):** budget = `free + Σ(remove bytes) − reserve`.
Removals, modifies, and metadata-only actions always apply (modifies count
their net size delta). Adds are grouped into **albums** (scanned album tag,
parent-directory fallback for untagged) and attempted in the existing
deterministic diff order, **first-fit**: an album fits entirely or is deferred
entirely. No album is ever split. A single track larger than the whole budget
is just a non-fitting album.

**Reality correction (apply time):** the running tally uses actual
post-transcode sizes. Since estimates only over-shoot, a second cheap fit pass
over deferred albums runs at the end of the loop; albums that now fit sync in
the same session.

**Reporting:** run summary + sync-history entry gain
`skipped_for_space { albums, tracks, bytes }`. macOS Device row: "Synced N of
M — 14 albums didn't fit (9.2 GB)", with the Library view as the fix-path.

**Degradation:** if free space can't be queried, log a warning and sync
without a budget (exactly today's behavior). Mid-sync disk-full from estimate
error still lands in the existing per-track error/retry path as the backstop.

## 4. Empty-source error & Replace Library

**Empty-source hard error:** after the source walk, zero audio files ⇒ abort
before any plan is computed: *"Source library at `<path>` contains no audio
files — not syncing. If you meant to empty this iPod, use Replace Library."*
Surfaces through the existing error event → daemon `error` state → UI.
Missing/unreadable source remains its own preflight error. Consequence: **no
degenerate walk can ever plan removals**; removals only come from a real
library genuinely lacking those files, or from deselection. All such syncs
auto-apply with no prompts or thresholds.

**Replace Library — the one explicit destructive action:**
- **Core:** `--replace-library` deletes every track from the iTunesDB
  (managed *and* foreign — the take-over-an-iTunes-iPod case), writes the DB,
  resets the device manifest, then runs a normal sync of the effective
  selection. Productizes the `examples/wipe-tracks.rs` machinery. The
  session-start backup runs first — even Replace is undoable for one session
  via `--restore-db-backup`.
- **Daemon:** new `replace_library` command; rejected while a sync is running;
  otherwise spawns the subprocess with the flag. Emits normal sync events.
- **macOS UI:** "Replace Library…" in the Device view's device-scoped
  controls, destructive styling, confirmation sheet stating exactly what
  happens, **armed by typing the device name**.
- **CLI:** interactive confirmation; `--replace-library --apply` skips it for
  scripts.

## 5. Artwork robustness & Apple-compatibility invariants

**Diagnosis first.** Reported symptoms (2026-07-17): art missing after
incremental syncs, and specific albums that never get art. Prime suspect for
the former: the apply loop checkpoints `db.write()` every 10 tracks/60 s, and
`itdb_write` on a parsed DB deletes F1069 cover thumbnails (0.2.2 finding);
the end-of-run `rebuild_apple_artwork` repairs this — but **Pause and Cancel
exit after a checkpoint without the rebuild**, and a subsequent no-change
resume never triggers it. Likely suspect for the latter: source images the
normalize step can't decode. Both are hypotheses to verify, not conclusions.

1. **`--verify-artwork` audit mode** (productized from `examples/art-audit.rs`):
   for every art-bearing selected track, check source-has-art →
   device-track-has-artwork → expected ithmb entries present; report per-track
   failures with reasons. Diagnostic tool now, regression harness forever.
2. **Invariant:** after *any* sync outcome — fresh, incremental,
   paused-then-resumed, cancelled-then-rerun — every art-bearing synced track
   has device-displayable art. Any path that checkpoints the DB must either
   preserve artwork or guarantee the rebuild runs before the session ends
   (e.g. pause's drain performs the artwork rebuild as part of its final
   checkpoint, or resume detects and repairs).
3. **Fix what diagnosis finds** within this spec's implementation. The
   user's known-failing albums become test fixtures (to be collected at
   implementation start).
4. **Apple-compat invariants:** every written `.m4a` is standard ALAC,
   readable by iTunes/Music.app off-device; **Music.app must still mount and
   read the device after every on-device gate sync** (standing acceptance
   check). iTunes-on-Windows acceptance is verified fresh when the Windows
   catch-up spec lands — prior "always rejects" lore is stale and carries no
   weight here; if rejection reproduces, it gets diagnosed then.
5. **Never silent again:** the run summary gains an artwork line
   ("art embedded for 214 of 216 tracks — 2 sources had no readable image").

## 6. Wire protocol changes (all additive)

| Surface | Bump | Additions |
|---|---|---|
| Subprocess stdio | 1.2.0 → 1.3.0 | `skipped_for_space` + artwork summary on `done`; log-line note for auto-restore |
| Daemon pipe | 1.4.0 → 1.5.0 | `replace_library` command; `skipped_for_space` + artwork summary on history/status; `selection_mode` on settings get/save |

`docs/ipc-protocol.md` updates in the same change. The Windows app (major-
version check only) safely ignores all of it until its catch-up spec.

## 7. Error handling summary

- Auto-restore never installs an unvalidated file; corrupt originals are kept
  aside as `iTunesDB.corrupt`.
- Fit engine degrades to today's behavior when free space is unqueryable.
- Empty walk is an error, never a plan; missing source stays a preflight error.
- Replace Library is the only mass-removal path, and even it is backed up.
- New failure copy always names the actual remedy (`--rebuild-manifest`,
  Library view, `--restore-db-backup`).

## 8. Testing

**Rust unit:** fit pass (album grouping, first-fit order, reserve math,
oversized single track, end-of-run deferred retry); estimate accounting for
modifies/removes; per-device path resolution + migration (root → `devices/`,
unconfigured case); effective-selection resolution (shared/custom/seed/
switch-back); empty-walk error; serial-mismatch-as-foreign.

**Rust integration:** daemon `replace_library` (rejected-while-syncing, happy
path) in the `daemon_runtime_integration` sandbox pattern; auto-restore
round-trip against a corrupted fixture DB + valid backup fixture;
`--verify-artwork` against fixture libraries.

**Swift:** reducer tests for `skipped_for_space` + artwork-summary rendering,
selection-mode toggle, Replace confirmation (armed only by exact name match).

**On-device gate (user):** corrupt-DB auto-restore on real hardware; a
deliberately over-full sync (album atomicity + deferred retry observed);
Replace Library on a device carrying foreign tracks; artwork invariant across
fresh / incremental / paused-resumed / cancelled-rerun syncs, checked with
`--verify-artwork`; Music.app reads the device after each gate sync.

## Non-goals

- Multi-device *simultaneous* support (daemon still drives one configured
  iPod; per-device state just makes switching safe).
- N-deep backup history, restore UI, or restore heuristics.
- User-facing sync priorities (the selection UI is the priority mechanism).
- Device capability table (LBA caps, ALAC tiers) — spec #2.
- Windows UI parity — spec #3. Cross-platform shared sync state — spec #4.
- Rockbox database generation, playlists, gapless — later specs.

## Open items for implementation

- Collect the user's known-failing albums as artwork fixtures.
- Confirm normalized-serial format used for directory names (must be
  filesystem-safe on both platforms).
- Verify Music.app read-compat check is scriptable enough for the gate, or
  keep it a manual checklist item.
