# Artwork-safe Sync and Cancellation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent DB writes or cancellation from dropping existing artwork, publish tracks/art/manifest coherently, and expose cancellation as a visible finalization phase.

**Architecture:** Albums are admitted and staged as bounded units under an atomic pending-session journal. `CheckpointCoordinator` opens a fresh DB, applies staged mutations, rehydrates every expected thumbnail, writes/reopens/verifies, publishes through Plan 2's `ManifestStore`, then removes obsolete files and the journal. Cancellation stops new admission, drains the current album, and finalizes through the same coordinator.

**Tech Stack:** Rust/libgpod FFI, serde journal, existing transcoder pipeline, subprocess/daemon IPC, Swift serial-keyed state from Plan 1.

## Global Constraints

- Depends on Plans 1–2. Literal ArtworkDB-before-track publication is impossible; stage artwork/audio first and publish linked records together.
- Old audio remains intact until the replacement DB is durable. Before libgpod removes/rebuilds artwork output, create and validate a complete rollback snapshot of `iTunesDB`, `ArtworkDB`, and every affected `.ithmb`; retain it through DB verification and device-manifest publication.
- Do not manually assign `ipod_path`; retain `itdb_cp_track_to_ipod` semantics.
- Cancellation is never completion. Emergency kill preserves the journal and reports interrupted finalization.

---

### Task 1: Pending journal and checkpoint coordinator

**Files:** Create `crates/classick/src/sync_transaction.rs`, `pending_session.rs`, `artwork_cache.rs`; modify `lib.rs`, `checkpoint.rs`, `ipod/db.rs`, `device_state.rs`, `fit.rs`, `manifest_store.rs`.

```rust
pub enum StopReason { Cancelled, Paused }
pub enum PendingPhase { Staging, ReadyToPublish, DatabaseVerified, DeviceManifestPublished, CleanupComplete }
pub struct StagedFile { pub source: PathBuf, pub pending_path: PathBuf, pub final_ipod_path: Option<PathBuf>, pub dbid: u64 }
pub struct ObsoleteFile { pub path: PathBuf, pub prior_dbid: u64 }
pub struct PendingSession { pub version: u32, pub session_id: SessionId, pub serial: String, pub phase: PendingPhase, pub albums: Vec<PendingAlbum>, pub staged_files: Vec<StagedFile>, pub obsolete_files: Vec<ObsoleteFile> }
pub struct CheckpointCoordinator<'a> { pub mount: &'a Path, pub serial: &'a str, pub manifest_store: &'a ManifestStore, pub artwork_cache: ArtworkCache }
pub fn publish(&self, journal: &mut PendingSession, manifest: &mut Manifest, progress: &Progress) -> Result<CheckpointResult>;
```

Use durable phases `Staging → ReadyToPublish → DatabaseVerified → DeviceManifestPublished → CleanupComplete`. Publication order: prepare all artwork; create/validate the full DB/artwork rollback snapshot; fresh DB; replay into a cloned candidate manifest; rehydrate all retained source-known tracks; remove stale artwork outputs; write; reopen; verify; persist `DatabaseVerified`; publish device manifest then host cache; assign the candidate manifest only after device publication; persist `DeviceManifestPublished`; obsolete/pending cleanup; journal removal. Restore the complete snapshot on write/verification/device-manifest failure. Journal every final `itdb_cp_track_to_ipod` path before DB write. Recovery inspects the live DB in ambiguous phases and never deletes a referenced new file. Defer deletion and set remove fit credit to zero. Journal every album; publish only at album boundaries/time/count/finalization.

- [ ] Add RED tests for atomic/corrupt journal, pre/post-publication recovery, foreign-file preservation, album order, boundary scheduling, failure injection at artwork/DB/manifest boundaries, old-audio retention, exact publication order, and peak-space budgeting.
- [ ] Implement `unlink_track_keep_file`, metadata+art setter, per-track verify, and referenced paths in `OwnedDb`; implement coordinator and recovery.
- [ ] Run focused module/integration tests then `cargo test -p classick` GREEN.
- [ ] Commit: `git commit -m "refactor(sync): add staged session journal and checkpoint coordinator"`.

### Task 2: Explicit finalized run outcomes

**Files:** Modify `apply_loop.rs`, `pipeline.rs`, `progress.rs`, `ipc.rs`, `main.rs`, `orchestrator.rs`, `docs/ipc-protocol.md`.

```rust
pub enum RunOutcome { Completed, Cancelled, Paused }
pub enum ProgressEvent { Finalizing { reason: StopReason, staged_albums: usize, staged_tracks: usize }, Cancelled, TrackDone(TrackResult), Phase(String), Finish(SyncSummary) }
```

Admit one album, use a bounded `OrderedTranscoder` for that album, consume every result, then checkpoint if due/stopping. Cancel during N drains N and admits none of N+1. A coherent cancellation emits exact JSON order `finalizing`, `cancelled`, then `finish` with `success:true`; the distinct cancelled event wins over process-success finish. Failed/interrupted finalization emits no `cancelled` and ends error/aborted. Remove post-commit whole-library repair, new dirty-marker creation, mid-loop raw DB writes, and cancelled-as-completed. A legacy dirty marker forces one coordinated publication then clears.

- [ ] Add RED tests for cancel before first album, mid-album drain/no-next-admit, pause, event ordering, review-stage quit, worker draining, failed finalization, and legacy marker migration.
- [ ] Implement and bump the subprocess protocol additively; run apply/pipeline/progress tests GREEN.
- [ ] Commit: `git commit -m "refactor(sync): make cancellation an explicit finalized outcome"`.

### Task 3: Daemon finalization drain

**Files:** Modify `daemon/sync_orchestrator.rs`, `session_admission.rs`, `runtime.rs`, `history.rs`, `ipc_daemon.rs`, daemon integration tests, protocol docs.

```rust
pub enum OrchestratorOutcome { Completed { summary: SyncSummary }, Cancelled { summary: Option<SyncSummary> }, Paused { summary: Option<SyncSummary> }, Aborted { reason: String, summary: Option<SyncSummary> } }
pub enum SessionPhase { Running, Finalizing { reason: StopReason } }
```

After cancel, write once and keep stdin/stdout open; forward progress until `cancelled` and EOF. Use a 120-second stall grace reset by progress, not a five-second total timer. Admission remains occupied during finalizing. Interrupted finalization is aborted/error and cannot replace latest successful sync.

- [ ] Add RED tests for one cancel write, `finalizing→cancelled→EOF`, ordinary completed finish, stall kill, heartbeat reset, cancelled history, shutdown drain, and B rejection while A finalizes.
- [ ] Implement and run daemon orchestrator/runtime integrations GREEN.
- [ ] Commit: `git commit -m "fix(daemon): drain cancellation through finalization"`.

### Task 4: macOS finalizing state

**Files:** Modify macOS wire, keyed device reducer, `DeviceRow.swift`, `ClassickApp.swift`, codec/reducer tests.

Add serial/session-scoped `.finalizing(reason, stagedAlbums, stagedTracks)` and `.cancelled`. Subtitle is exactly “Finishing sync…”; caption is exactly “Keep the iPod connected”; “Saving completed albums” may appear only as the meter label. Disable sync/pause/cancel while finalizing. Preserve latest successful sync; Plan 3 owns the authoritative terminal transition rather than raw finish.

- [ ] Add RED tests for decoding, immediate daemon finalizing, raw-finish non-transition, timestamp preservation, interrupted error, and A/B isolation.
- [ ] Implement and run Swift tests GREEN.
- [ ] Commit: `git commit -m "fix(ui): surface cancellation finalization"`.

### Task 5: Strong per-track artwork audit

**Files:** Modify `art_audit.rs`, `ipod/db.rs`, `build.rs`; add gobject import/link artifacts only if required; extend tests.

Validate DBID, `has_artwork`, nonzero `mhii_link`, non-null artwork/thumbnail, `itdb_track_has_thumbnails`, and decoded `itdb_track_get_thumbnail`. Always `g_object_unref` returned pixbuf. Classify failures (`MissingTrack`, `HasArtworkUnset`, `MissingMhiiLink`, `MissingArtworkRecord`, `MissingThumbnail`, `DecodeFailed`). Audit remains read-only.

- [ ] Add RED classification tests including the exact false positive: global ithmb exists but a track has zero link/null thumbnail; cover decode-null and legitimate no-source-art.
- [ ] Implement/link, run focused tests and full Rust suite GREEN.
- [ ] After all automated gates, run the audit on the mounted iPod and verify the six reported albums; never write the music share.
- [ ] Commit: `git commit -m "fix(artwork): verify every expected thumbnail link and decode"`.
