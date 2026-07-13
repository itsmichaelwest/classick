# Resumable, Parallel, Resilient Sync — Design

**Date:** 2026-07-13
**Status:** Approved (brainstorm) → ready for implementation plan
**Scope:** One cohesive redesign of the apply loop plus the daemon/IPC/UI
surface needed to expose it. macOS is the build+verify target; Windows changes
are specified but land as `TODO(windows)` (no build environment).

## Motivation

On the reference device — an iPod Classic (MC293) on its **original spinning
4200 rpm HDD**, FAT32 via macOS 15's new **fskit** driver — a full sync of a
~1500-track FLAC library takes hours. afconvert transcode is the bottleneck
(4–7 s per 44.1/16 track, 9–23 s per hi-res 88.2 kHz/24-bit track; raw iPod
write is fine at ~34 MB/s). Because it takes so long, the user **deliberately
stops** long syncs, leaving the library partially synced. The reported "album
art is unreliable" was a symptom of this: art extraction/decode is verified
correct (see memory `macos-sync-reality`), but only the tracks that got synced
have art.

Today's apply loop (`crates/classick/src/apply_loop.rs::run`) is strictly
serial: `for action in plan { transcode; add_to_libgpod; checkpoint every
SYNC_CHECKPOINT_EVERY=25 }`. Resume already works at checkpoint granularity —
each checkpoint persists the manifest, and a re-run re-diffs source vs. manifest
and continues — but nothing parallelizes the transcode, there is no deliberate
pause, and a rare transient write failure aborts the whole run.

## Goals

1. **Faster full sync** — parallelize afconvert transcode while keeping the
   libgpod add/copy serialized (libgpod is not thread-safe).
2. **Deliberate pause/resume** — a first-class graceful **Pause** that drains
   in-flight work, checkpoints, and enters a resumable **Paused** state;
   **Resume** continues from the checkpoint. Plus **"X of Y synced"** visibility
   in the UI.
3. **Resilience** — bounded auto-retry with backoff on the rare transient iPod
   write failure, so a one-off fskit/HDD hiccup does not abort an otherwise
   hands-off sync.

## Non-goals

- **No resume-on-replug.** The user syncs in deliberate chunks; auto-continuing
  on plug-in would surprise them. (The existing auto-sync-on-plug `enabled`
  setting is unchanged and orthogonal.)
- **No album-art changes.** The art pipeline is correct (locked decisions in
  `classick-macos-decisions`: afconvert-only, no cover-art fallback).
- **No Windows build/verify.** Windows code paths get `TODO(windows)` markers.
- **No change to the source→ALAC transcode itself** (afconvert on macOS,
  ffmpeg/refalac elsewhere) beyond running instances concurrently.

## Architecture

Replace the serial loop with an **ordered, bounded-window parallel pipeline**:

```
plan (in source order)
   │  feeder assigns seq numbers; only Add/Modify need transcode
   ▼
[bounded job channel]  ──►  ┌─ transcode worker 1 (afconvert) ─┐
   (≤ window outstanding)   ├─ transcode worker 2 ─────────────┤ ─► reorder
                            └─ transcode worker N ─────────────┘    buffer
                                                                      │ (by seq)
                                                                      ▼
                                            single COMMITTER thread (owns OwnedDb)
                                            consumes in strict plan order:
                                              • Remove/Metadata/Unchanged: inline
                                              • Add/Modify: take ready transcode,
                                                add_track_with_file (+retry),
                                                update manifest
                                              • checkpoint (time-or-count)
                                              • pause: drain window, checkpoint, exit
```

**Concurrency invariant:** exactly one thread ever touches libgpod (the
committer). Transcode workers touch only the filesystem (source read, afconvert,
art extract to a temp `.m4a`/art bytes). This preserves the current safety of
the single-`OwnedDb` model.

**Ordering invariant:** the committer applies actions in the original plan
order. Transcodes may *finish* out of order but are *committed* in order via the
reorder buffer, so the manifest always reflects a contiguous prefix of the plan
→ resume remains a clean "first N done."

**Backpressure / disk bound:** the job channel holds at most `window` outstanding
transcodes, so at most `window` temp `.m4a` files exist at once — independent of
library size. If a slow hi-res track is still encoding when the committer
reaches its seq, the committer blocks on just that item (head-of-line), which is
rare and cheap on this HDD.

### Component boundaries

- **`crates/classick/src/pipeline.rs`** (new) — the reusable ordered
  bounded-window parallel map. Inputs: an iterator of jobs, a `window` size, a
  worker count, and a `transcode_fn: Fn(&Job) -> Result<Transcoded>`. Output: an
  iterator/channel of `(seq, Result<Transcoded>)` delivered **in seq order**.
  Knows nothing about libgpod, the manifest, or the daemon — pure orchestration,
  unit-testable with an injected `transcode_fn` and a fake clock. A `Pause`/stop
  signal cancels the feeder and stops spawning new work; already-running workers
  finish (their results are drainable).
- **`crates/classick/src/apply_loop.rs`** (slimmed) — owns `OwnedDb`, drives the
  pipeline as the committer: pulls `(seq, Transcoded)` in order, runs the
  per-action match arms (Add/Modify/Remove/Metadata), applies the checkpoint
  policy, handles pause/cancel decisions, and updates the manifest. The existing
  per-action logic (`add_one`, `do_metadata_only`, retry/skip/abort prompts)
  moves under this committer.
- **Retry helper** — a small `retry_transient` in `try_with_prompt.rs` (or a new
  `retry.rs`): `retry_transient(attempts, backoff, op)` runs `op`, retrying on
  `Err` up to `attempts` with the given backoff schedule, returning the last
  error on exhaustion. Applied only to the iPod write ops.
- **Daemon** (`crates/classick/src/daemon/`) — `Pause`/`Resume` commands, a
  `Paused` sync outcome, and X/Y counts in the status broadcast.

## Data flow & control

### Transcode classification (unchanged, just parallelized)
Each Add/Modify job runs the current `add_one` front half on a worker:
`transcode::probe` (lofty on macOS) → `classify` → `transcode_to_alac`
(afconvert on macOS) → art extract if `has_embedded_art`. The result
`Transcoded { seq, temp_path, tags, art, encoder, source_format, fingerprints }`
carries everything the committer needs. **The committer never transcodes.**

### Committer per-action handling
- **Add** → `db.add_track_with_file(temp, tags, art)` wrapped in
  `retry_transient`; on success update `manifest.tracks`, delete temp.
- **Modify** → retryable `delete_track` then retryable add (same as today's
  delete-then-readd), both wrapped.
- **Remove** → retryable `delete_track`.
- **Metadata** → `do_metadata_only` (retryable write).
- **Unchanged** → skip.
- After each committed action, evaluate the **checkpoint policy** (below).

### Checkpoint policy (replaces fixed `SYNC_CHECKPOINT_EVERY = 25`)
Checkpoint (`db.write()` + `manifest::save`) when **either** ≥ **10 committed
tracks** *or* ≥ **60 s** since the last checkpoint, whichever comes first.
Rationale: on a spinning HDD a hi-res track can take 20 s, so a pure count
bound lets abrupt-unplug loss balloon; the time bound caps worst-case lost work
to ~a minute regardless of track size. `db.write()` itself is wrapped in
`retry_transient`. Constants live in `lib.rs`
(`CHECKPOINT_MAX_TRACKS = 10`, `CHECKPOINT_MAX_SECONDS = 60`), replacing
`SYNC_CHECKPOINT_EVERY`.

### Retry policy
`retry_transient` with **3 attempts** and backoff **250 ms → 1 s → 3 s** wraps
**only** the iPod write ops (`add_track_with_file`/`itdb_cp_track_to_ipod`,
`delete_track`, checkpoint `db.write()`). **Transcode failures are not retried**
— afconvert failing is deterministic (a bad/unsupported file), so a failed
transcode job surfaces as a per-track error that skips+logs (the track is
absent from the manifest and reappears in the next diff). On **retry
exhaustion** of a write op:
- **Interactive TUI:** the existing Retry/Skip/Abort prompt (now already showing
  the full `{e:#}` chain).
- **Daemon/auto mode:** **skip + log** the track and continue (it reappears in
  the next diff), rather than aborting the whole run — completing the sync is
  the priority.

### Pause / Resume
- **Pause** is a new `Decision` (delivered like today's Quit via `decision_rx`,
  and via a new daemon `Pause` command). When the committer observes Pause: stop
  the feeder (no new jobs), **drain the reorder buffer** — commit every transcode
  that is already done (do not waste completed afconvert work), discard/cleanup
  temps for jobs not yet transcoded, **checkpoint**, and return a **`Paused`**
  outcome.
- **Paused** is a distinct sync outcome from `Cancelled`/`Failed`/`Succeeded`.
  The daemon records it and the UI surfaces "Paused — X of Y — Resume".
- **Resume** = trigger a normal sync (`TriggerSync`). The manifest diff continues
  from the checkpoint. No suspended in-memory sync/DB is held (the iPod is not
  pinned open between chunks).
- **Cancel** (existing) remains as the abort/stop path; mechanically it also
  checkpoints, but its resulting state is the normal idle/last-sync state, not
  `Paused`. On macOS the in-sync menu promotes **Pause** as the primary action.

### Visibility: "X of Y synced"
- `Y` = source track count (from `source::walk`, already computed at sync start;
  the daemon can also compute it cheaply on demand for the idle display).
- `X` = `manifest.tracks.len()` (tracks currently on the iPod per the manifest).
- The daemon includes `synced_count` (X) and `library_count` (Y) in its
  `status_update`. The menu renders "119 of 1500 synced".

## IPC changes (protocol minor bump: daemon 1.1.0 → 1.2.0)

`docs/ipc-protocol.md` is the source of truth and is updated in lock-step.

- **New daemon command:** `Pause` (the only new command). **Resume is not a
  wire command** — the UI implements Resume by sending the existing
  `TriggerSync`, which diff-resumes from the checkpoint. This keeps the wire
  surface minimal and reuses proven machinery.
- **New sync outcome / event:** `Paused` (alongside the existing finish/`success`
  reporting).
- **New status fields:** `synced_count`, `library_count`.
- **Sync-event stream:** the per-track `summary`/progress already carries planned
  totals; no breaking change, only additive fields.
- Adding commands + additive fields is a **minor** bump; both sides must agree.
  Rust: `ipc.rs` / `ipc_daemon.rs`. macOS: `DaemonCommand.swift` /
  `DaemonEvent.swift` / `WireModels.swift`. Windows C#: `TODO(windows)`.

## UI changes (macOS)

- `AppModel`: add a `.paused` case to `Phase` (or a paused flag on the synced
  state); add `syncedCount`/`libraryCount`; derive "X of Y synced".
- `MenuContent`: during a sync show **Pause** (primary) [and Cancel]; when
  `.paused` show **Resume** + "Paused — X of Y"; when idle show "X of Y synced".
- `ClassickApp`/`AppDelegate`: wire `pause()`/`resume()` to the new daemon
  commands (mirrors existing `syncNow`/`cancelSync`).
- Windows WinUI: `TODO(windows)`.

## File structure

| File | Change |
|---|---|
| `crates/classick/src/pipeline.rs` | **New** — ordered bounded-window parallel map (feeder + workers + reorder buffer); injectable `transcode_fn`; pause/stop signal. Pure, unit-tested. |
| `crates/classick/src/apply_loop.rs` | Slimmed — becomes the committer driving `pipeline`; keeps per-action arms, checkpoint policy, pause/cancel handling, manifest updates. |
| `crates/classick/src/try_with_prompt.rs` | Add `retry_transient(attempts, backoff, op)`. |
| `crates/classick/src/lib.rs` | Replace `SYNC_CHECKPOINT_EVERY` with `CHECKPOINT_MAX_TRACKS`/`CHECKPOINT_MAX_SECONDS`; add worker/window defaults (`TRANSCODE_WORKERS`, `PIPELINE_WINDOW`). |
| `crates/classick/src/daemon/runtime.rs` | `Pause`/`Resume` handling, `Paused` outcome, X/Y in status. |
| `crates/classick/src/ipc.rs`, `ipc_daemon.rs` | New commands/fields; version bump. |
| `crates/classick/src/progress.rs` | Plumb Pause decision + Paused/counts through the progress backends. |
| `docs/ipc-protocol.md` | Document 1.2.0 additions. |
| `ui/macos/.../{AppModel,MenuContent,ClassickApp,WireModels,DaemonCommand,DaemonEvent}.swift` | Pause/Resume + X/Y. |
| `ui/windows/...` | `TODO(windows)` comments only. |

## Tunable defaults (approved)

- Transcode workers: `min(available_parallelism − 1, 4)` (`TRANSCODE_WORKERS`).
- Pipeline look-ahead window: `8` (`PIPELINE_WINDOW`).
- Checkpoint: `10` tracks **or** `60` s, whichever first.
- Retry: `3` attempts, backoff `250 ms → 1 s → 3 s`, iPod-write ops only.

## Error handling summary

- **Transient iPod write error** → `retry_transient` (3×); on exhaustion,
  TUI prompt or daemon skip+log.
- **Transcode (afconvert) error** → not retried; per-track skip+log; track
  reappears in next diff.
- **Checkpoint `db.write()` error** → `retry_transient`; on exhaustion the sync
  fails (as today) with the full `{e:#}` chain and recovery block, having
  preserved the previous checkpoint.
- **Pause** → graceful drain + checkpoint + `Paused`; never data loss.
- **Abrupt unplug/crash** → lose ≤ the checkpoint window (~10 tracks / 60 s of
  work); orphans cleaned by the next `reconcile_with_disk`.

## Testing

**Unit (`pipeline.rs`):**
- Results delivered strictly in seq order despite out-of-order completion
  (injected `transcode_fn` with per-seq delays / a fake clock).
- Window bound respected (never more than `window` in flight).
- Pause/stop: feeder stops issuing, in-flight results drainable, no deadlock.

**Unit (retry + checkpoint):**
- `retry_transient` retries on Err, honors backoff schedule, returns last error
  on exhaustion, returns Ok immediately on first success.
- Checkpoint policy fires on the count bound and independently on the time bound
  (fake clock).

**Integration (`tests/daemon_runtime_integration.rs` patterns):**
- Pause mid-sync → daemon reports `Paused`, manifest reflects a contiguous
  prefix, a subsequent `TriggerSync` continues (no re-adding committed tracks).
- Injected transient add failure (fake DB seam) → retried, then succeeds, sync
  completes without a prompt.
- Ordering: a plan with mixed action types commits in plan order.

**Manual (device):** a real macOS sync completes faster (parallel transcode);
Pause → Resume continues cleanly; "X of Y" reflects real progress.

## Rationale for key decisions

- **In-order committing** (vs. out-of-order) — resume clarity beats marginal
  throughput; the HDD commit is fast so head-of-line stalls are rare.
- **Interleaved pipeline** (vs. two-phase pre-transcode-to-cache) — bounds temp
  disk to the window, lands tracks on the iPod steadily, and makes pause cheap.
- **Resume = new sync via diff** (vs. suspended in-memory state) — reuses proven
  machinery, avoids pinning the iPod open between chunks or holding stale state.
- **Retry writes but not transcodes** — transient I/O is retryable; afconvert
  failures are deterministic and would just burn time.
- **Skip+log on exhaustion in daemon mode** — completing the sync matters more
  than a single track; the diff picks it up next run.
```
