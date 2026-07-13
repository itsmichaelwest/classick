# Resumable, Parallel, Resilient Sync — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Classick's serial apply loop into a parallel-transcode, in-order-committing, pause-able, auto-retrying sync so large libraries sync faster and can be paused/resumed without losing work.

**Architecture:** N afconvert transcode workers run ahead of a single libgpod committer thread over a bounded window; the committer applies actions in strict plan order, checkpoints on a time-or-count policy, retries transient iPod-write failures, and stops gracefully on Pause (drain window + checkpoint → Paused outcome). The daemon forwards a new `pause` command to the sync subprocess and reports "X of Y synced".

**Tech Stack:** Rust (std threads + `std::sync::mpsc::sync_channel` + `Mutex`/`Condvar`; `anyhow`; `tracing`), the existing IPC stdio + daemon-pipe wire, Swift/SwiftUI (macOS menu-bar app).

## Global Constraints

- **libgpod is single-threaded:** exactly ONE thread (the committer) may touch `OwnedDb`/libgpod. Transcode workers touch only the filesystem.
- **macOS is the build+verify target.** Verify with `cargo test` (workspace root), `cd ui/macos && swift test`, and `cd ui/macos && xcodegen generate && xcodebuild -project Classick.xcodeproj -scheme Classick -configuration Debug -destination 'platform=macOS' build`.
- **Windows changes are `TODO(windows)` comments only** — no Windows build here.
- **Transcode = afconvert on macOS; never bundle ffmpeg.** No album-art fallback. (Locked; don't touch the transcode/art internals beyond running instances concurrently.)
- **Tunable defaults (verbatim):** transcode workers `min(available_parallelism − 1, 4)`; look-ahead window `8`; checkpoint `10` tracks **or** `60` s (whichever first); retry backoff `[250 ms, 1 s, 3 s]` (up to 3 retries → 4 total attempts), **iPod-write ops only**.
- **No `println!` outside `examples/`** — `tracing` only (stdout is the IPC wire).
- **Wire is additive → minor bumps:** subprocess protocol `1.0.0 → 1.1.0` (new `pause` command + `paused` event); daemon protocol `1.1.0 → 1.2.0` (new `Pause` command + `synced_count`/`library_count` status fields). Both sides move together; `docs/ipc-protocol.md` is the source of truth.
- **Commits:** Conventional Commits; scopes `apply-loop`, `pipeline`, `daemon`, `ipc`, `ui`, `docs`. Never `git add -A`; stage named files. Never amend; never `--no-verify`.
- **Files ≤ ~500 LOC** — the pipeline machinery goes in a new `pipeline.rs`, not `apply_loop.rs`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/classick/src/try_with_prompt.rs` | + `retry_transient` (bounded retry with backoff). |
| `crates/classick/src/lib.rs` | Replace `SYNC_CHECKPOINT_EVERY` with `CHECKPOINT_MAX_TRACKS`/`CHECKPOINT_MAX_SECONDS`; add `TRANSCODE_WORKERS`/`PIPELINE_WINDOW`/`RETRY_BACKOFF`. |
| `crates/classick/src/checkpoint.rs` | **New** — `CheckpointClock` (time-or-count trigger), pure + tested. |
| `crates/classick/src/pipeline.rs` | **New** — `OrderedTranscoder` bounded-window ordered parallel map. Pure, injected transcode fn, tested. |
| `crates/classick/src/apply_loop.rs` | Split `add_one` → `transcode_one` (worker) + `commit_transcoded` (committer); drive the pipeline; checkpoint policy; pause drain; retries. |
| `crates/classick/src/progress.rs` | `Decision::Pause`; `ProgressEvent::Paused`; `run_plain`/`run_ipc` handling. |
| `crates/classick/src/ipc.rs` | `IpcCommand::Pause` (+ `to_decision`), `IpcEvent::Paused`, bump `PROTOCOL_VERSION`. |
| `crates/classick/src/ipc_daemon.rs` | `DaemonCommand::Pause`; `StatusUpdate.synced_count`/`library_count`; bump `DAEMON_PROTOCOL_VERSION`. |
| `crates/classick/src/daemon/sync_orchestrator.rs` | Forward Pause to subprocess stdin; `OrchestratorOutcome::Paused`. |
| `crates/classick/src/daemon/runtime.rs` | `Pause` command → orchestrator; library/synced counts in `StatusUpdate`. |
| `crates/classick/src/main.rs` | Emit `Paused` terminal event from `run`'s outcome. |
| `docs/ipc-protocol.md` | Document both protocol bumps. |
| `ui/macos/.../WireModels.swift` | `DaemonCommand.pause`; `StatusInfo` counts; `SyncEvent.paused`. |
| `ui/macos/.../AppModel.swift` | `Phase.paused`; counts; reducer handling. |
| `ui/macos/.../MenuContent.swift` | Pause in `.syncing`; `.paused` arm with Resume; "X of Y synced" in `.idle`. |
| `ui/macos/.../ClassickApp.swift` | `pause()`/`resume()` on AppDelegate; wire to menu. |
| `ui/windows/...` | `TODO(windows)` comments only. |

---

## Task 1: Bounded retry helper

**Files:**
- Modify: `crates/classick/src/try_with_prompt.rs`
- Modify: `crates/classick/src/lib.rs`
- Test: same file (`#[cfg(test)]` in `try_with_prompt.rs`)

**Interfaces:**
- Produces: `pub fn retry_transient<T>(backoff: &[Duration], op: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T>`; `pub const RETRY_BACKOFF: [Duration; 3]` in `lib.rs`.

- [ ] **Step 1: Write the failing test** — append to `crates/classick/src/try_with_prompt.rs`:

```rust
#[cfg(test)]
mod retry_tests {
    use super::retry_transient;
    use anyhow::anyhow;
    use std::cell::Cell;
    use std::time::Duration;

    const NODELAY: [Duration; 3] = [Duration::ZERO, Duration::ZERO, Duration::ZERO];

    #[test]
    fn succeeds_first_try_without_retrying() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || { calls.set(calls.get() + 1); Ok(7) });
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retries_then_succeeds() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 { Err(anyhow!("transient")) } else { Ok(42) }
        });
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn returns_last_error_after_exhaustion() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || {
            calls.set(calls.get() + 1);
            Err(anyhow!("fail {}", calls.get()))
        });
        assert_eq!(calls.get(), 4); // 1 initial + 3 retries
        assert_eq!(r.unwrap_err().to_string(), "fail 4");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p classick retry_tests`
Expected: FAIL — `retry_transient` not found.

- [ ] **Step 3: Implement.** Add to the top of `crates/classick/src/try_with_prompt.rs` (near the other `use`s):

```rust
use std::time::Duration;

/// Run `op`, retrying on `Err` after each delay in `backoff` (so a 3-element
/// backoff means up to 3 retries = 4 total attempts). Returns the first `Ok`,
/// or the LAST error after the schedule is exhausted. Use ONLY for transient
/// I/O (iPod writes); deterministic failures (e.g. a bad transcode) must not
/// be retried.
pub fn retry_transient<T>(
    backoff: &[Duration],
    mut op: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut attempt = 0usize;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= backoff.len() {
                    return Err(e);
                }
                let delay = backoff[attempt];
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                attempt += 1;
            }
        }
    }
}
```

Add to `crates/classick/src/lib.rs` (near `SYNC_CHECKPOINT_EVERY`):

```rust
use std::time::Duration;

/// Backoff schedule for transient iPod-write retries (add/copy, delete,
/// checkpoint write). 3 delays = up to 3 retries. See `retry_transient`.
pub const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
];
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p classick retry_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/try_with_prompt.rs crates/classick/src/lib.rs
git commit -m "feat(apply-loop): add bounded retry_transient helper"
```

---

## Task 2: Checkpoint clock (time-or-count)

**Files:**
- Create: `crates/classick/src/checkpoint.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod checkpoint;`, constants; remove `SYNC_CHECKPOINT_EVERY` in Task 5)
- Test: in `checkpoint.rs`

**Interfaces:**
- Produces: `checkpoint::CheckpointClock` with `new(max_tracks: usize, max_interval: Duration, now: Instant) -> Self` and `record(&mut self, now: Instant) -> bool` (returns true = checkpoint now, and resets); `CHECKPOINT_MAX_TRACKS: usize`, `CHECKPOINT_MAX_SECONDS: u64` in `lib.rs`.

- [ ] **Step 1: Write the failing test** — create `crates/classick/src/checkpoint.rs`:

```rust
//! Time-or-count checkpoint trigger for the apply loop. Checkpointing
//! (itdb_write + manifest save) is expensive/seeky on a spinning-HDD iPod, so
//! we bound BOTH the number of tracks and the wall-clock since the last flush:
//! whichever comes first fires a checkpoint. The time bound caps abrupt-unplug
//! loss to ~`max_interval` regardless of how slow individual (hi-res) tracks are.

use std::time::{Duration, Instant};

pub struct CheckpointClock {
    tracks_since: usize,
    last: Instant,
    max_tracks: usize,
    max_interval: Duration,
}

impl CheckpointClock {
    pub fn new(max_tracks: usize, max_interval: Duration, now: Instant) -> Self {
        Self { tracks_since: 0, last: now, max_tracks, max_interval }
    }

    /// Record one committed track. Returns `true` if a checkpoint is due now
    /// (and resets the counters). `now` is injected for testability.
    pub fn record(&mut self, now: Instant) -> bool {
        self.tracks_since += 1;
        let due = self.tracks_since >= self.max_tracks
            || now.duration_since(self.last) >= self.max_interval;
        if due {
            self.tracks_since = 0;
            self.last = now;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::CheckpointClock;
    use std::time::{Duration, Instant};

    #[test]
    fn fires_on_count_bound() {
        let t0 = Instant::now();
        // Large interval so only the count bound can fire.
        let mut c = CheckpointClock::new(3, Duration::from_secs(3600), t0);
        assert!(!c.record(t0));
        assert!(!c.record(t0));
        assert!(c.record(t0)); // 3rd track
        assert!(!c.record(t0)); // reset → counting again
    }

    #[test]
    fn fires_on_time_bound_independent_of_count() {
        let t0 = Instant::now();
        // max_tracks huge so only the time bound can fire; zero interval => the
        // first record already satisfies `elapsed >= 0`.
        let mut c = CheckpointClock::new(10_000, Duration::ZERO, t0);
        assert!(c.record(t0));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p classick checkpoint`
Expected: FAIL — module `checkpoint` not declared.

- [ ] **Step 3: Implement.** Add to `crates/classick/src/lib.rs`:

```rust
pub mod checkpoint;

/// Checkpoint when EITHER this many tracks have committed OR
/// `CHECKPOINT_MAX_SECONDS` have elapsed since the last checkpoint.
pub const CHECKPOINT_MAX_TRACKS: usize = 10;
pub const CHECKPOINT_MAX_SECONDS: u64 = 60;
```

(The `checkpoint.rs` body from Step 1 is the implementation.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p classick checkpoint`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/checkpoint.rs crates/classick/src/lib.rs
git commit -m "feat(apply-loop): time-or-count CheckpointClock"
```

---

## Task 3: Split `add_one` into transcode + commit halves (still serial)

De-risk the pipeline by extracting the worker-safe transcode half first and
proving the serial loop still works, before adding threads.

**Files:**
- Modify: `crates/classick/src/apply_loop.rs`
- Test: `crates/classick/src/apply_loop.rs` (`#[cfg(test)]`) — a pure test on `Transcoded` field wiring is not feasible without a device; rely on the existing integration suite + a smoke unit test that `transcode_one` compiles and `commit_transcoded` maps fields. Add a field-mapping unit test using a hand-built `Transcoded`.

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct Transcoded {
      pub temp: std::path::PathBuf,
      pub tags: crate::tags::Tags,
      pub art: Option<Vec<u8>>,
      pub encoder: String,
      pub encoder_version: String,
      pub source_format: String,
      pub fingerprint: String,
      pub audio_fingerprint: String,
  }
  pub(crate) fn transcode_one(src: &SourceEntry, config: &Config, refalac_version: &Option<String>) -> anyhow::Result<Transcoded>
  fn commit_transcoded(db: &OwnedDb, manifest: &mut Manifest, src: &SourceEntry, t: Transcoded) -> anyhow::Result<()>
  ```
- Consumes: existing `entry_from(...)`, `add_track_with_file`, `retry_transient`, `RETRY_BACKOFF`.

- [ ] **Step 1: Write the failing test** — append to `apply_loop.rs`:

```rust
#[cfg(test)]
mod split_tests {
    // Ensures Transcoded carries every field the manifest entry needs, so the
    // committer half can build an entry without re-reading the source.
    #[test]
    fn transcoded_has_manifest_fields() {
        let t = super::Transcoded {
            temp: std::path::PathBuf::from("/tmp/x.m4a"),
            tags: crate::tags::Tags::default(),
            art: None,
            encoder: "ffmpeg".into(),
            encoder_version: "v".into(),
            source_format: "flac".into(),
            fingerprint: "fp".into(),
            audio_fingerprint: "afp".into(),
        };
        assert_eq!(t.encoder, "ffmpeg");
        assert_eq!(t.source_format, "flac");
    }
}
```

(This forces `Transcoded` to exist with these fields. `crate::tags::Tags` must derive `Default` — it does; if not, construct it explicitly.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p classick split_tests`
Expected: FAIL — `Transcoded` not found.

- [ ] **Step 3: Implement.** In `apply_loop.rs`, add the `Transcoded` struct (near `AddOneOutcome`), then define `transcode_one` as `add_one`'s body up to (but not including) `db.add_track_with_file`, moving the fingerprint computation into it:

```rust
pub(crate) struct Transcoded {
    pub temp: std::path::PathBuf,
    pub tags: crate::tags::Tags,
    pub art: Option<Vec<u8>>,
    pub encoder: String,
    pub encoder_version: String,
    pub source_format: String,
    pub fingerprint: String,
    pub audio_fingerprint: String,
}

/// Worker-safe half of the old `add_one`: probe + classify + transcode + art
/// extract + fingerprints. Touches ONLY the filesystem — never libgpod — so it
/// runs on pipeline worker threads. Mirrors `add_one` lines 801–892 minus the
/// `db.add_track_with_file` call.
pub(crate) fn transcode_one(
    src: &SourceEntry,
    config: &Config,
    refalac_version: &Option<String>,
) -> anyhow::Result<Transcoded> {
    let probe = transcode::probe(&src.path, &config.ffmpeg)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);
    let source_format = source_format_from_probe(&probe);

    let classify_cfg = transcode::ClassifyConfig { passthrough_wav: config.passthrough_wav };
    let action = transcode::classify(&probe, &classify_cfg)
        .with_context(|| format!("classify {}", src.path.display()))?;

    // (encoder, encoder_version, temp) — verbatim from add_one lines 814–868.
    let (encoder, encoder_version, temp): (String, String, std::path::PathBuf) = match action {
        SourceAction::Passthrough => {
            let dst = transcode::temp_passthrough_path(&src.path);
            transcode::passthrough(&src.path, &dst)
                .with_context(|| format!("passthrough copy for {}", src.path.display()))?;
            ("passthrough".to_string(), String::new(), dst)
        }
        SourceAction::Transcode => match config.encoder {
            EncoderChoice::Ffmpeg => {
                let dst = transcode::temp_alac_path();
                if let Some(parent) = dst.parent() { std::fs::create_dir_all(parent).ok(); }
                transcode::transcode_to_alac(&src.path, &dst, &config.ffmpeg)
                    .with_context(|| format!("transcode {}", src.path.display()))?;
                let ver = ffmpeg_version(&config.ffmpeg)
                    .unwrap_or_else(|_| "ffmpeg (version unknown)".to_string());
                ("ffmpeg".to_string(), ver, dst)
            }
            EncoderChoice::Refalac => {
                let dst = transcode::temp_alac_path();
                if let Some(parent) = dst.parent() { std::fs::create_dir_all(parent).ok(); }
                let art_path_opt = if has_embedded_art(&probe) {
                    let art_path = transcode::temp_art_path();
                    transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)
                        .with_context(|| format!("extract art for refalac --artwork: {}", src.path.display()))?;
                    Some(art_path)
                } else { None };
                let ffmpeg_path = config.ffmpeg.as_path();
                let result = transcode::transcode_via_refalac(
                    &src.path, &dst, &config.refalac_path, ffmpeg_path, art_path_opt.as_deref(),
                ).with_context(|| format!("refalac transcode {}", src.path.display()));
                if let Some(p) = &art_path_opt { let _ = std::fs::remove_file(p); }
                result?;
                let ver = refalac_version.clone()
                    .unwrap_or_else(|| "refalac (version unknown)".to_string());
                ("refalac".to_string(), ver, dst)
            }
        },
    };

    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else { None };

    let fingerprint = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;
    let audio_fingerprint = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;

    Ok(Transcoded { temp, tags, art, encoder, encoder_version, source_format, fingerprint, audio_fingerprint })
}

/// Committer half: add the transcoded file to libgpod (with transient-retry)
/// and push the manifest entry. MUST run on the single committer thread.
fn commit_transcoded(
    db: &OwnedDb,
    manifest: &mut Manifest,
    src: &SourceEntry,
    t: Transcoded,
) -> anyhow::Result<()> {
    let handle = crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
        db.add_track_with_file(&t.temp, &t.tags, t.art.as_deref())
    })
    .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;
    let _ = std::fs::remove_file(&t.temp);
    manifest.tracks.push(entry_from(
        src, &handle, &t.fingerprint, &t.audio_fingerprint,
        &t.encoder, &t.encoder_version, &t.source_format,
    ));
    Ok(())
}
```

Then **rewrite `add_one`** to compose the two halves (keeps existing callers working during this task):

```rust
pub(crate) fn add_one(
    db: &OwnedDb,
    src: &SourceEntry,
    config: &Config,
    refalac_version: &Option<String>,
) -> anyhow::Result<()> {
    let t = transcode_one(src, config, refalac_version)?;
    commit_transcoded(db, /* manifest */ ??, src, t) // see note
}
```

**Note:** the old `add_one` returned `AddOneOutcome` and the caller pushed the
manifest entry. Since `commit_transcoded` now owns the manifest push, change the
Add/Modify arms to call `transcode_one` + `commit_transcoded` directly and drop
the old `AddOneOutcome` return path. Concretely, replace the Add arm body
(lines 525–559) with:

```rust
let committed = loop {
    match transcode_one(&src, config, &refalac_version) {
        Ok(t) => match commit_transcoded(&db, &mut manifest, &src, t) {
            Ok(()) => break true,
            Err(e) => match prompt_retry_skip_abort(progress, decision_rx, &src, &e)? {
                PromptOutcome::Retry => continue,
                PromptOutcome::Skip => { progress.log(format!("Skipped Add for {}", src.path.display())); break false; }
                _ => return Err(e),
            },
        },
        Err(e) => match prompt_retry_skip_abort(progress, decision_rx, &src, &e)? {
            PromptOutcome::Retry => continue,
            PromptOutcome::Skip => { progress.log(format!("Skipped Add for {}", src.path.display())); break false; }
            _ => return Err(e),
        },
    }
};
let _ = committed;
```

Extract the shared prompt into a helper (removes the duplicated await_prompt blocks in Add + Modify):

```rust
fn prompt_retry_skip_abort(
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    src: &SourceEntry,
    e: &anyhow::Error,
) -> anyhow::Result<PromptOutcome> {
    let msg = format!("Failed to add {}:\n  {e:#}\n\nChoose:", src.path.display());
    await_prompt(
        progress, decision_rx, msg,
        &["Retry", "Skip this track", "Abort"],
        &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
    )
}
```

Delete the now-unused `AddOneOutcome` struct. Update the Modify arm's re-add
half (lines 471–514) the same way (transcode_one + commit_transcoded + the
helper). Keep the delete half wrapped in `retry_transient`:

```rust
let deleted = loop {
    match crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
        db.delete_track(old.ipod_dbid).with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))
    }) {
        Ok(()) => break true,
        Err(e) => { /* existing Retry/Skip/Abort prompt, {e:#} */ }
    }
};
```

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p classick`
Expected: PASS — `split_tests` passes; existing apply-loop/daemon tests still pass (behavior unchanged: serial, just refactored).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/apply_loop.rs
git commit -m "refactor(apply-loop): split add_one into transcode_one + commit_transcoded"
```

---

## Task 4: `pipeline.rs` — ordered bounded-window parallel map

**Files:**
- Create: `crates/classick/src/pipeline.rs`
- Modify: `crates/classick/src/lib.rs` (`pub mod pipeline;`, worker/window constants)
- Test: in `pipeline.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct OrderedTranscoder<T: Send + 'static> { /* opaque */ }
  impl<T: Send + 'static> OrderedTranscoder<T> {
      pub fn start<J, F>(jobs: Vec<(usize, J)>, workers: usize, window: usize, transcode: F) -> Self
        where J: Send + 'static, F: Fn(&J) -> anyhow::Result<T> + Send + Sync + 'static;
      pub fn take(&self, seq: usize) -> anyhow::Result<T>;  // blocks until seq ready; frees a window slot
      pub fn stop(&self);                                   // signal no more takes; workers wind down
  }
  ```
  `jobs` are `(action_index, job)` pairs for ONLY the transcode actions (Add/Modify); `take(seq)` is called by the committer for those indices in increasing order.
- Consumes: `TRANSCODE_WORKERS`, `PIPELINE_WINDOW` from `lib.rs`.

**Design:** a `window`-permit semaphore bounds in-flight (dispatched-but-not-taken) jobs, so at most `window` temp files exist at once. A feeder thread acquires a permit then hands `(seq, job)` to a bounded `sync_channel(window)`; `workers` threads transcode and store `Result<T>` in a `Mutex<HashMap<usize, Result<T>>>` guarded by a `Condvar`. `take(seq)` waits on the condvar for `seq`, removes it, and releases a permit (unblocking the feeder).

- [ ] **Step 1: Write the failing test** — create `crates/classick/src/pipeline.rs`:

```rust
//! Ordered, bounded-window parallel map. Transcode workers run ahead of a
//! single consumer (the apply-loop committer); results are delivered strictly
//! in `seq` order via `take(seq)`. At most `window` jobs are ever in flight, so
//! temp-file/disk use is bounded independent of library size. libgpod is never
//! touched here — `transcode` is a pure filesystem operation.

use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct Results<T> {
    ready: Mutex<HashMap<usize, anyhow::Result<T>>>,
    cv: Condvar,
}

struct Permits {
    count: Mutex<usize>,
    cv: Condvar,
}

pub struct OrderedTranscoder<T: Send + 'static> {
    results: Arc<Results<T>>,
    permits: Arc<Permits>,
    _feeder: JoinHandle<()>,
    _workers: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> OrderedTranscoder<T> {
    pub fn start<J, F>(jobs: Vec<(usize, J)>, workers: usize, window: usize, transcode: F) -> Self
    where
        J: Send + 'static,
        F: Fn(&J) -> anyhow::Result<T> + Send + Sync + 'static,
    {
        let window = window.max(1);
        let workers = workers.max(1);
        let results = Arc::new(Results { ready: Mutex::new(HashMap::new()), cv: Condvar::new() });
        let permits = Arc::new(Permits { count: Mutex::new(window), cv: Condvar::new() });
        let transcode = Arc::new(transcode);

        let (job_tx, job_rx): (SyncSender<(usize, J)>, Receiver<(usize, J)>) = sync_channel(window);
        let job_rx = Arc::new(Mutex::new(job_rx));

        // Feeder: acquire a permit per job (bounds in-flight to `window`), then
        // enqueue. Dropping job_tx at the end signals workers to exit.
        let permits_f = permits.clone();
        let feeder = std::thread::spawn(move || {
            for (seq, job) in jobs {
                // acquire permit
                {
                    let mut n = permits_f.count.lock().unwrap();
                    while *n == 0 {
                        n = permits_f.cv.wait(n).unwrap();
                    }
                    *n -= 1;
                }
                if job_tx.send((seq, job)).is_err() {
                    break; // all workers gone
                }
            }
            // job_tx dropped here → workers' recv() returns Err → they exit.
        });

        let mut worker_handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let job_rx = job_rx.clone();
            let results = results.clone();
            let transcode = transcode.clone();
            worker_handles.push(std::thread::spawn(move || loop {
                let next = {
                    let rx = job_rx.lock().unwrap();
                    rx.recv()
                };
                let (seq, job) = match next {
                    Ok(pair) => pair,
                    Err(_) => break, // feeder dropped job_tx
                };
                let out = transcode(&job);
                let mut ready = results.ready.lock().unwrap();
                ready.insert(seq, out);
                results.cv.notify_all();
            }));
        }

        Self { results, permits, _feeder: feeder, _workers: worker_handles }
    }

    /// Block until job `seq` has been transcoded, return its result, and free a
    /// window permit (letting the feeder dispatch one more).
    pub fn take(&self, seq: usize) -> anyhow::Result<T> {
        let mut ready = self.results.ready.lock().unwrap();
        loop {
            if let Some(r) = ready.remove(&seq) {
                drop(ready);
                // release permit
                let mut n = self.permits.count.lock().unwrap();
                *n += 1;
                self.permits.cv.notify_one();
                return r;
            }
            ready = self.results.cv.wait(ready).unwrap();
        }
    }

    /// Idempotent best-effort: wake the feeder so it can observe a dropped
    /// consumer. (Workers exit when job_tx drops; the struct's Drop joins.)
    pub fn stop(&self) {
        self.permits.cv.notify_all();
        self.results.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::OrderedTranscoder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn delivers_in_seq_order_despite_out_of_order_completion() {
        // seq 0 sleeps longest, seq 4 shortest → they finish reversed, but
        // take(0..5) must still return 0,1,2,3,4.
        let jobs: Vec<(usize, usize)> = (0..5).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 4, 8, |&i: &usize| {
            std::thread::sleep(Duration::from_millis(((5 - i) * 20) as u64));
            Ok::<usize, anyhow::Error>(i * 10)
        });
        for seq in 0..5 {
            assert_eq!(ot.take(seq).unwrap(), seq * 10);
        }
    }

    #[test]
    fn never_exceeds_window_in_flight() {
        let max_seen = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(AtomicUsize::new(0));
        let (m, c) = (max_seen.clone(), cur.clone());
        let jobs: Vec<(usize, usize)> = (0..40).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 4, 8, move |&i: &usize| {
            let now = c.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            c.fetch_sub(1, Ordering::SeqCst);
            Ok::<usize, anyhow::Error>(i)
        });
        for seq in 0..40 {
            let _ = ot.take(seq).unwrap();
        }
        // in-flight = concurrently-running transcodes; bounded by min(workers,window).
        assert!(max_seen.load(Ordering::SeqCst) <= 8, "in-flight exceeded window");
    }

    #[test]
    fn propagates_errors_in_order() {
        let jobs: Vec<(usize, usize)> = (0..3).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 2, 4, |&i: &usize| {
            if i == 1 { Err(anyhow::anyhow!("boom {i}")) } else { Ok::<usize, anyhow::Error>(i) }
        });
        assert_eq!(ot.take(0).unwrap(), 0);
        assert!(ot.take(1).is_err());
        assert_eq!(ot.take(2).unwrap(), 2);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p classick pipeline`
Expected: FAIL — module `pipeline` not declared.

- [ ] **Step 3: Implement.** Add to `crates/classick/src/lib.rs`:

```rust
pub mod pipeline;

/// Concurrent afconvert transcode workers (afconvert is CPU-bound; oversubscribing
/// hurts). Resolved at runtime via available_parallelism, capped.
pub fn transcode_workers() -> usize {
    std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(1).clamp(1, 4)
}
/// Max jobs transcoded ahead of the committer (bounds temp-file disk use).
pub const PIPELINE_WINDOW: usize = 8;
```

(The `pipeline.rs` body from Step 1 is the implementation.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p classick pipeline`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/pipeline.rs crates/classick/src/lib.rs
git commit -m "feat(pipeline): ordered bounded-window parallel transcoder"
```

---

## Task 5: Drive the pipeline from the committer (parallel transcode + checkpoint policy)

Wire Tasks 2–4 into the apply loop: transcode Add/Modify actions in parallel via
`OrderedTranscoder`, commit in order, checkpoint via `CheckpointClock`.

**Files:**
- Modify: `crates/classick/src/apply_loop.rs`
- Modify: `crates/classick/src/lib.rs` (remove `SYNC_CHECKPOINT_EVERY`)
- Test: existing `tests/daemon_runtime_integration.rs` covers end-to-end; add a unit test asserting a mixed plan commits in order using a fake committer is out of scope (committer needs libgpod). Rely on the integration suite + manual device run.

**Interfaces:**
- Consumes: `transcode_one`, `commit_transcoded`, `OrderedTranscoder`, `CheckpointClock`, `transcode_workers()`, `PIPELINE_WINDOW`, `CHECKPOINT_MAX_TRACKS`, `CHECKPOINT_MAX_SECONDS`.

- [ ] **Step 1: Restructure the action loop.** Replace the serial `for action in actions` loop (lines 330–643) so that, before the loop, the transcode jobs are dispatched:

```rust
use crate::pipeline::OrderedTranscoder;
use crate::checkpoint::CheckpointClock;
use std::time::{Duration, Instant};

// Collect (action_index, SourceEntry) for the actions that transcode.
// SourceEntry is Clone; clone into the pipeline so workers own their inputs.
let transcode_jobs: Vec<(usize, SourceEntry)> = actions.iter().enumerate()
    .filter_map(|(idx, a)| match a {
        Action::Add(src) => Some((idx, src.clone())),
        Action::Modify(src, _old) => Some((idx, src.clone())),
        _ => None,
    })
    .collect();

let config_for_workers = config.clone();          // Config: derive Clone (see Step 2)
let refalac_for_workers = refalac_version.clone();
let transcoder = OrderedTranscoder::start(
    transcode_jobs,
    crate::transcode_workers(),
    crate::PIPELINE_WINDOW,
    move |src: &SourceEntry| transcode_one(src, &config_for_workers, &refalac_for_workers),
);

let mut ckpt = CheckpointClock::new(
    crate::CHECKPOINT_MAX_TRACKS,
    Duration::from_secs(crate::CHECKPOINT_MAX_SECONDS),
    Instant::now(),
);
let mut i = 0usize;
let mut cancelled = false;
let mut paused = false;

for (idx, action) in actions.into_iter().enumerate() {
    // Decision poll (Task 6 adds Pause; today only Quit).
    match decision_rx.try_recv() {
        Ok(Decision::Review(ReviewDecision::Quit)) => {
            progress.log("Cancel requested — finalising completed tracks before stopping...");
            cancelled = true; break;
        }
        Ok(Decision::Pause) => {   // Task 6 introduces this variant
            progress.log("Pause requested — finalising in-flight tracks…");
            paused = true; break;
        }
        _ => {}
    }
    match action {
        Action::Unchanged(_) => continue,
        Action::Remove(entry) => { /* unchanged, but wrap delete_track in retry_transient */ }
        Action::Metadata(..) => { /* unchanged do_metadata_only, wrap write in retry_transient */ }
        Action::Add(src) => {
            i += 1;
            progress.track_start(i, total_planned, format!("ADD {}", display_path(&src.path)));
            commit_pipelined(&transcoder, idx, &db, &mut manifest, &src, progress, decision_rx)?;
            progress.track_done();
        }
        Action::Modify(src, old) => {
            i += 1;
            progress.track_start(i, total_planned, format!("MODIFY {}", display_path(&src.path)));
            if effective_no_delete { /* existing skip-under-no-delete */ progress.track_done(); continue; }
            // delete old (retry), then commit the pre-transcoded new file:
            let deleted = /* retry_transient(delete_track) with existing prompt */;
            if deleted { manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid); commit_pipelined(&transcoder, idx, &db, &mut manifest, &src, progress, decision_rx)?; }
            progress.track_done();
        }
    }
    if ckpt.record(Instant::now()) {
        progress.log(format!("Checkpoint: persisting state after {i} tracks…"));
        crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || db.write().context("checkpoint: db.write"))?;
        manifest.last_source_root = Some(config.source.clone());
        manifest::save_atomic(&config.manifest_path, &manifest).context("checkpoint: manifest save")?;
    }
}
transcoder.stop();
```

Where `commit_pipelined` bridges the pre-transcoded result to the committer, handling transcode errors (skip+log — deterministic) and commit errors (retry via `commit_transcoded`, then prompt):

```rust
fn commit_pipelined(
    transcoder: &OrderedTranscoder<Transcoded>,
    idx: usize,
    db: &OwnedDb,
    manifest: &mut Manifest,
    src: &SourceEntry,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> anyhow::Result<()> {
    let t = match transcoder.take(idx) {
        Ok(t) => t,
        Err(e) => {
            // Deterministic transcode failure: do NOT retry, skip+log.
            progress.error(format!("Transcode failed for {}: {e:#}", src.path.display()));
            progress.log(format!("Skipped {} (transcode failed)", src.path.display()));
            return Ok(());
        }
    };
    loop {
        match commit_transcoded(db, manifest, src, /* need owned t */) {
            Ok(()) => return Ok(()),
            Err(e) => match prompt_retry_skip_abort(progress, decision_rx, src, &e)? {
                PromptOutcome::Retry => { /* re-take not possible; re-commit same t */ continue; }
                PromptOutcome::Skip => { progress.log(format!("Skipped Add for {}", src.path.display())); return Ok(()); }
                _ => return Err(e),
            },
        }
    }
}
```

**Ownership note:** `commit_transcoded` consumes `Transcoded` (it deletes the
temp and moves fields into the manifest entry). For the Retry loop, restructure
`commit_transcoded` to borrow (`&Transcoded`) and only remove the temp + push
the entry on success, so a retry re-uses the same `t`. Adjust its signature to
`fn commit_transcoded(db, manifest, src, t: &Transcoded) -> Result<()>` and move
the `std::fs::remove_file(&t.temp)` to after a successful add.

- [ ] **Step 2: Make `Config` cloneable for workers.** In `crates/classick/src/config.rs`, add `#[derive(Clone)]` to `Config` (verify all fields are `Clone`; `PathBuf`/`String`/enums are). This lets each worker own a snapshot. Run `cargo build` to confirm.

- [ ] **Step 3: Remove `SYNC_CHECKPOINT_EVERY`.** Delete the const from `lib.rs`; `grep -rn SYNC_CHECKPOINT_EVERY crates/` must return nothing.

- [ ] **Step 4: Run the suite**

Run: `cargo test -p classick` and `cargo test -p classick --test daemon_runtime_integration -- --test-threads=1`
Expected: PASS. (The integration suite drives a fake device/DB; ordering + checkpoint behavior exercised there.)

- [ ] **Step 5: Manual device smoke (optional here; required before merge).** With an iPod mounted: `cargo build --release && ./target/release/classick --apply --source <dir> --ipod <mount> --no-delete` — observe multiple tracks transcoding concurrently (faster wall-clock), checkpoints every ~10 tracks/60 s, and correct on-device order.

- [ ] **Step 6: Commit**

```bash
git add crates/classick/src/apply_loop.rs crates/classick/src/config.rs crates/classick/src/lib.rs
git commit -m "feat(apply-loop): parallel transcode pipeline + time-or-count checkpoint"
```

---

## Task 6: Graceful Pause (core side)

**Files:**
- Modify: `crates/classick/src/progress.rs` (`Decision::Pause`, `ProgressEvent::Paused`, `run_plain`, `run_ipc`)
- Modify: `crates/classick/src/ipc.rs` (`IpcCommand::Pause` + `to_decision`, `IpcEvent::Paused`, bump `PROTOCOL_VERSION`)
- Modify: `crates/classick/src/apply_loop.rs` (`run` returns a `RunOutcome`; emit Paused)
- Modify: `crates/classick/src/main.rs` (map outcome → terminal event)
- Test: `ipc.rs` (`to_decision` maps Pause), `progress.rs` (Pause decision plumbs)

**Interfaces:**
- Produces: `pub enum Decision { …, Pause }`; `ProgressEvent::Paused`; `IpcCommand::Pause`; `IpcEvent::Paused`; `pub enum RunOutcome { Completed, Paused }` returned by `apply_loop::run`.

- [ ] **Step 1: Write the failing test** — in `crates/classick/src/ipc.rs` `#[cfg(test)]`:

```rust
#[test]
fn pause_command_maps_to_pause_decision() {
    let cmd: IpcCommand = serde_json::from_str(r#"{"type":"pause"}"#).unwrap();
    assert!(matches!(cmd.to_decision(), Some(crate::progress::Decision::Pause)));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p classick pause_command_maps`
Expected: FAIL — no `Pause` variant.

- [ ] **Step 3: Implement.**
  - `progress.rs`: add `Pause` to `Decision`; add `Paused` to `ProgressEvent`; in `run_plain`, handle `ProgressEvent::Paused => { println!("Paused. Completed tracks were saved."); break }`; in `run_ipc`, `IpcEvent::from_progress` maps `ProgressEvent::Paused` → `IpcEvent::Paused`, and the event loop treats `Paused` as terminal (same as `Finish`).
  - `ipc.rs`: bump `PROTOCOL_VERSION` to `"1.1.0"`; add `IpcCommand::Pause`; extend `to_decision`:
    ```rust
    IpcCommand::Cancel => Some(Decision::Review(ReviewDecision::Quit)),
    IpcCommand::Pause  => Some(Decision::Pause),
    ```
    add `IpcEvent::Paused` (terminal, no fields) and map it in `from_progress`.
  - `apply_loop.rs`: change `run` to return `anyhow::Result<RunOutcome>` where `pub enum RunOutcome { Completed, Paused }`; at the final commit, if `paused` return `Ok(RunOutcome::Paused)` else `Ok(RunOutcome::Completed)`. The Paused path still runs the final `db.write()` + manifest save (drain + checkpoint) before returning.
  - `main.rs`: after `orchestrate` returns the outcome, emit `progress`'s terminal event: `Completed`/error → `progress.finish(success)`; `Paused` → send `ProgressEvent::Paused` then `finish(true)`. (Keep `finish` for exit-code semantics; `Paused` is emitted first so the wire carries it.)

- [ ] **Step 4: Wire pause drain in the committer.** The Task-5 loop already breaks with `paused = true` on `Decision::Pause`. On break, the `transcoder.stop()` + final commit path drains: any Add/Modify actions ALREADY iterated are committed; the not-yet-reached ones are abandoned (their in-flight transcodes' temp files are cleaned by the OS temp dir / next reconcile). The final `db.write()` + manifest save is the checkpoint. This gives "lose nothing already committed."

- [ ] **Step 5: Run tests**

Run: `cargo test -p classick`
Expected: PASS incl. `pause_command_maps_to_pause_decision`.

- [ ] **Step 6: Commit**

```bash
git add crates/classick/src/progress.rs crates/classick/src/ipc.rs crates/classick/src/apply_loop.rs crates/classick/src/main.rs
git commit -m "feat(apply-loop,ipc): graceful Pause outcome + pause command (proto 1.1.0)"
```

---

## Task 7: Daemon — forward Pause + report X/Y

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs` (`DaemonCommand::Pause`; `StatusUpdate.synced_count`/`library_count`; bump `DAEMON_PROTOCOL_VERSION` to `"1.2.0"`)
- Modify: `crates/classick/src/daemon/sync_orchestrator.rs` (`OrchestratorOutcome::Paused`; forward `{"type":"pause"}`; recognize `paused` line)
- Modify: `crates/classick/src/daemon/runtime.rs` (`Pause` arm; compute counts for `StatusUpdate`)
- Test: `ipc_daemon.rs` (decode `{"type":"pause"}`); existing daemon integration suite

**Interfaces:**
- Consumes: subprocess emits `{"type":"paused"}` (Task 6).
- Produces: `DaemonCommand::Pause`; `StatusUpdate { …, synced_count: usize, library_count: Option<usize> }`.

- [ ] **Step 1: Write the failing test** — in `ipc_daemon.rs` `#[cfg(test)]`:

```rust
#[test]
fn decodes_pause_command() {
    let cmd: DaemonCommand = serde_json::from_str(r#"{"type":"pause"}"#).unwrap();
    assert!(matches!(cmd, DaemonCommand::Pause));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p classick decodes_pause_command`
Expected: FAIL — no `Pause` variant.

- [ ] **Step 3: Implement.**
  - `ipc_daemon.rs`: bump `DAEMON_PROTOCOL_VERSION` to `"1.2.0"`; add `Pause` to `DaemonCommand` (doc: "Gracefully pause the running sync — drains in-flight, checkpoints, → Paused. No-op if idle."); add to `StatusUpdate`:
    ```rust
    /// Tracks currently on the iPod per the manifest (X in "X of Y synced").
    synced_count: usize,
    /// Source-library track count (Y). None until known (first walk).
    #[serde(skip_serializing_if = "Option::is_none")]
    library_count: Option<usize>,
    ```
  - `sync_orchestrator.rs`: add `OrchestratorOutcome::Paused { summary: Option<...> }`; in the event-relay match, add `"paused" => { finish_success = Some(true); /* record paused */ }` and return `Paused` when the subprocess emits it (it exits right after, so the `None` branch of `next_line` then completes — track a `paused` bool and return `Paused` instead of the normal completed outcome). Add a `Pause` forward path mirroring the cancel arm but writing `{"type":"pause"}\n` and NOT force-killing (pause is graceful — let the subprocess finish its drain and exit on its own; keep the 5 s kill only as a backstop). Wire a `pause_rx` oneshot alongside `cancel_rx`.
  - `runtime.rs`: add a `pause_tx_holder: &mut Option<oneshot::Sender<()>>` threaded like `cancel_tx_holder`; `DaemonCommand::Pause => { if let Some(tx) = pause_tx_holder.take() { let _ = tx.send(()); } }`. In the two `StatusUpdate` builders (the `GetStatus` arm ~line 700 and `broadcast_status` ~line 659 and the post-sync `handle_internal_event` ~line 417), populate `synced_count` = manifest length and `library_count` = cached source count:
    ```rust
    let (synced_count, library_count) = status_counts(config_path, connected);
    ```
    where `status_counts` reads the manifest length (via `manifest::load_or_default(manifest_path).map(|m| m.tracks.len())`) for X and walks the configured source (cached; recompute lazily) for Y. Add a small cache in daemon state keyed on the source path; invalidate on `SaveConfig`. If the walk is too costly to do on every status, compute `library_count` once per config-load and store it; `synced_count` is cheap and always fresh.

- [ ] **Step 4: Run tests**

Run: `cargo test -p classick` and the daemon integration suite (`-- --test-threads=1`).
Expected: PASS incl. `decodes_pause_command`.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/ipc_daemon.rs crates/classick/src/daemon/sync_orchestrator.rs crates/classick/src/daemon/runtime.rs
git commit -m "feat(daemon): forward Pause to sync subprocess + report X/Y counts (proto 1.2.0)"
```

---

## Task 8: macOS UI — Pause/Resume + "X of Y synced"

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift`
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Modify: `ui/macos/Sources/Classick/Views/MenuContent.swift`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift`
- Test: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift` (+ `WireCodecTests.swift`)

**Interfaces:**
- Produces: `DaemonCommand.pause`; `StatusInfo.syncedCount`/`libraryCount`; `SyncEvent.paused`; `Phase.paused(synced:total:)`; `AppDelegate.pause()`/`resume()`.

- [ ] **Step 1: Write the failing test** — in `AppModelReducerTests.swift`:

```swift
func testStatusUpdateCarriesSyncedCounts() {
    let m = AppModel()
    m.apply(.deviceConnected(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", drive: "/Volumes/IPOD", name: "iPod"))
    m.apply(.configUpdate(source: "/music", daemon: nil,
                          ipod: IpodIdentity(serial: "0xA", modelLabel: "iPod Classic (3rd gen)", name: nil)))
    m.apply(.statusUpdate(.init(state: .idle, configured: true, ipodConnected: true,
                                lastSync: nil, nextScheduledUnixSecs: nil, storage: nil,
                                syncedCount: 119, libraryCount: 1500)))
    XCTAssertEqual(m.syncedCount, 119)
    XCTAssertEqual(m.libraryCount, 1500)
}

func testPausedSyncEventEntersPausedPhase() {
    let m = AppModel()
    m.apply(.statusUpdate(.init(state: .syncing, configured: true, ipodConnected: true,
                                lastSync: nil, nextScheduledUnixSecs: nil, storage: nil,
                                syncedCount: 50, libraryCount: 1500)))
    m.apply(.syncEvent(line: #"{"type":"paused"}"#))
    guard case .paused = m.phase else { return XCTFail("expected .paused") }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd ui/macos && swift test --filter AppModelReducerTests`
Expected: FAIL — `StatusInfo` has no `syncedCount`; no `.paused`.

- [ ] **Step 3: Implement.**
  - `WireModels.swift`:
    - `DaemonCommand`: add `case pause` → encodes `{"type":"pause"}` (add to the `encode` switch: `case .pause: try container.encode("pause", forKey: .type)`).
    - `StatusInfo`: add `var syncedCount: Int` and `var libraryCount: Int?`; add `syncedCount = "synced_count"`, `libraryCount = "library_count"` to both `CodingKeys` (the struct's and `DaemonEvent`'s). Decode in the `status_update` arm: `let syncedCount = try container.decodeIfPresent(Int.self, forKey: .syncedCount) ?? 0`, `let libraryCount = try container.decodeIfPresent(Int.self, forKey: .libraryCount)`.
    - `SyncEvent`: add `case paused`; decode `case "paused": self = .paused`.
  - `AppModel.swift`:
    - `Phase`: add `case paused(synced: Int, total: Int?)`.
    - Fields: `private(set) var syncedCount: Int = 0`, `private(set) var libraryCount: Int?`.
    - In `apply` `.statusUpdate`: set `syncedCount = info.syncedCount; libraryCount = info.libraryCount`.
    - In `applySyncEvent`: add `case .paused: phase = .paused(synced: syncedCount, total: libraryCount)`.
    - `computePhase`: unchanged (Paused is set directly from the sync-event stream, like `.syncing`).
  - `MenuContent.swift`:
    - `.idle` arm: after the device line, add `if model.libraryCount != nil || model.syncedCount > 0 { Text(syncedSummary(model)) }` where `syncedSummary` renders `"\(synced) of \(total) synced"` or `"\(synced) synced"`.
    - `.syncing` arm: add `Button("Pause", action: onPause)` alongside `Button("Cancel Sync", action: onCancelSync)`.
    - add `.paused` arm: `Text("Paused — \(pausedSummary)")` + `Button("Resume", action: onResume)`.
    - Add `var onPause: () -> Void` and `var onResume: () -> Void` closures to `MenuContent`.
  - `ClassickApp.swift`:
    - `AppDelegate.pause() { Task { await daemonClient.send(.pause) } }` and `resume() { Task { await daemonClient.send(.triggerSync(source: .manual)) } }`.
    - Pass `onPause: appDelegate.pause, onResume: appDelegate.resume` into `MenuContent(...)`.

- [ ] **Step 4: Run tests + build**

Run: `cd ui/macos && swift test`
Expected: PASS (new tests + existing 21).
Run: `cd ui/macos && xcodegen generate && xcodebuild -project Classick.xcodeproj -scheme Classick -configuration Debug -destination 'platform=macOS' build`
Expected: `** BUILD SUCCEEDED **`.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Classick.xcodeproj/project.pbxproj
git commit -m "feat(ui): Pause/Resume + X of Y synced (macOS)"
```

---

## Task 9: Protocol docs + Windows TODO + final verification

**Files:**
- Modify: `docs/ipc-protocol.md`
- Modify: `ui/windows/Classick.UI.Core/Ipc/IpcCommand.cs`, `DaemonCommand.cs`, `DaemonEvent.cs` (comments only)
- Modify: `LEARNINGS.md`

- [ ] **Step 1: Update `docs/ipc-protocol.md`.** Document: subprocess protocol `1.1.0` (new `pause` command → graceful drain+checkpoint; new terminal `paused` event); daemon protocol `1.2.0` (new `Pause` command; `status_update` gains `synced_count`, optional `library_count`). Note the semver handshake still requires major `1`.

- [ ] **Step 2: Windows TODO markers.** Add to the relevant C# wire files:

```csharp
// TODO(windows): pause/resume + X-of-Y not yet wired on Windows.
// Add IpcCommand "pause" + terminal "paused" event (subprocess proto 1.1.0),
// DaemonCommand "Pause" + status_update synced_count/library_count (daemon
// proto 1.2.0), and a Pause/Resume UI. Mirror the Rust + macOS implementations.
// Can't build/verify Windows here — see docs/ipc-protocol.md.
```

- [ ] **Step 3: LEARNINGS entry.** One bullet: the pipeline is an ordered bounded-window parallel map with a single libgpod committer; Pause = graceful drain+checkpoint→Paused; resume is a normal `TriggerSync` (diff-based).

- [ ] **Step 4: Full verification.**

Run: `cargo test` (workspace) → all pass.
Run: `cd ui/macos && swift test` → all pass.
Run: `cd ui/macos && xcodebuild ... build` → BUILD SUCCEEDED.
Run (device, before declaring done): a real `--apply` sync completes faster (concurrent transcode), Pause → menu shows "Paused — X of Y" → Resume continues, checkpoints ~10 tracks/60 s.

- [ ] **Step 5: Commit**

```bash
git add docs/ipc-protocol.md ui/windows/Classick.UI.Core/Ipc/IpcCommand.cs ui/windows/Classick.UI.Core/Ipc/DaemonCommand.cs ui/windows/Classick.UI.Core/Ipc/DaemonEvent.cs LEARNINGS.md
git commit -m "docs(ipc): document proto 1.1.0/1.2.0 pause + X-of-Y; Windows TODO"
```

---

## Self-review notes

- **Spec coverage:** parallel transcode (Tasks 3–5), in-order commit (Task 4 `take(seq)` + committer loop), checkpoint time-or-count (Task 2/5), retry (Task 1/3/5), graceful Pause + Paused (Task 6/7), X of Y (Task 7/8), no-resume-on-replug (Resume = `TriggerSync`, Task 8), proto bumps (Task 6/7/9), Windows TODO (Task 9). All covered.
- **Type consistency:** `Transcoded` (Task 3) is produced by `transcode_one` and consumed by `commit_transcoded`/`commit_pipelined` (Tasks 3/5) and delivered by `OrderedTranscoder<Transcoded>` (Task 4/5). `Decision::Pause` (Task 6) is emitted by `IpcCommand::Pause.to_decision` (Task 6) and consumed by the committer loop (Task 5 references it; land Task 6 before enabling that arm, or stub the arm in Task 5 and fill in Task 6 — the plan orders Task 5 before Task 6, so in Task 5 the `Ok(Decision::Pause)` arm is added together with the variant OR gated behind Task 6; implementer note: if executing strictly in order, add the `Decision::Pause` variant in Task 5's decision-poll edit and the rest of Pause in Task 6).
- **Ordering caveat:** Task 5's loop references `Decision::Pause`. To keep tasks independently compilable, add the bare `Decision::Pause` enum variant (unused) in Task 5, and complete its wiring (IPC command, event, outcome) in Task 6.
