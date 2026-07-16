# macOS Desktop App Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the macOS menu-bar accessory into a Dock app with a persistent,
iTunes-style main window (sidebar + content + a pinned bottom device row),
auto-refresh the library via a daemon file watcher, and show a daemon-computed
sync ETA.

**Architecture:** Two codebases. **Rust core/daemon** gains a `notify`-based
library watcher (triggering the existing crash-isolated scan subprocess) and a
sync ETA emitted on the `track_start` IPC event. **SwiftUI app** flips from
`LSUIElement` to a Dock app with a `WindowGroup` main window
(`NavigationSplitView`: Library / Device / History + a bottom `DeviceRow`),
keeping the menu-bar extra as a condensed secondary surface. The `AppModel`
reducer stays the single source of truth and single test seam.

**Tech Stack:** Rust (tokio, `notify`, serde), Swift 6 / SwiftUI (macOS 15+),
newline-delimited JSON IPC.

## Global Constraints

- **Conventional Commits**, scopes in use: `daemon`, `ui`, `ipc`,
  `transcode`, `progress`, `docs`, `build`. New macOS UI work uses `ui`.
- **No `println!` outside `examples/`** — in IPC mode stdout IS the wire;
  use `tracing::{info,warn,error,debug}`.
- **IPC is the contract.** `docs/ipc-protocol.md` is source of truth; a wire
  change updates the doc + all decoders together. Inner `sync_event` protocol
  is currently **`1.1.0`**; this plan bumps it to **`1.2.0`** (additive).
- **macOS transcoding is `afconvert`** — never bundle ffmpeg on macOS. (Not
  touched here, but don't regress it.)
- **Keep files ≤ ~500 LOC**; split aggressively. New SwiftUI views are their
  own files.
- **Bug fixes / new behavior get tests** — reducer logic in `AppModelReducerTests`,
  Rust logic in the crate's unit/integration suites.
- **Swift 6 strict concurrency**: `@MainActor` on UI types, `Sendable` wire types.
- **Do not** use `git add -A`/`.` (stage named files); never `--no-verify`;
  never amend — new commit each time.
- **Daemon owns lifetime** of the process, `DaemonClient`, and `AppModel`
  (in `AppDelegate`). Views observe; they do not own.

---

## File Structure

**Rust (crates/classick):**
- Create: `src/daemon/library_watcher.rs` — `notify` watcher → coalesced change signal.
- Modify: `src/progress.rs` — add `EtaEstimator`, feed `eta_secs` in `run_ipc`.
- Modify: `src/ipc.rs` — add optional `eta_secs` to `IpcEvent::TrackStart`.
- Modify: `src/daemon/runtime.rs` — startup scan, watcher select arm + debounce, rewatch on SaveConfig.
- Modify: `src/daemon/mod.rs` — module decl + a `LIBRARY_DEBOUNCE_WINDOW` const.
- Modify: `Cargo.toml` (workspace + crate) — `notify` dependency.
- Modify: `docs/ipc-protocol.md` — `eta_secs` on `track_start`, version → 1.2.0.
- Test: `src/progress.rs` (unit), `src/daemon/library_watcher.rs` (unit/integration),
  `tests/daemon_runtime_integration.rs` (watcher→scan).

**Swift (ui/macos):**
- Modify: `Info.plist` — `LSUIElement` false.
- Modify: `Sources/Classick/ClassickApp.swift` — Dock lifecycle, `WindowGroup`, window-open plumbing.
- Modify: `Sources/Classick/Model/AppModel.swift` — retain `[HistoryEntry]`, ETA in `.syncing`.
- Create: `Sources/Classick/Views/MainWindow.swift` — split view + sidebar.
- Create: `Sources/Classick/Views/LibraryView.swift` — persistent browser (from `ChooseMusicWindow`).
- Create: `Sources/Classick/Views/DeviceRow.swift` — bottom strip, all states.
- Create: `Sources/Classick/Views/DeviceView.swift` — dashboard + device controls.
- Create: `Sources/Classick/Views/HistoryView.swift` — history table.
- Modify: `Sources/Classick/Views/MenuContent.swift` — condensed menu.
- Modify: `Sources/Classick/Ipc/WireModels.swift` — `SyncEvent.trackStart` gains `etaSecs`.
- Delete (end): `ChooseMusicWindow.swift` + `ChooseMusicWindowController.swift` once `LibraryView` replaces them.
- Test: `Tests/ClassickTests/AppModelReducerTests.swift`, `WireCodecTests.swift`.

---

## PHASE A — Rust core (cross-platform; no UI dependency)

### Task 1: Sync ETA on `track_start`

**Files:**
- Modify: `crates/classick/src/ipc.rs` (TrackStart struct + `from_progress`)
- Modify: `crates/classick/src/progress.rs` (add `EtaEstimator`, feed in `run_ipc`)
- Modify: `docs/ipc-protocol.md`
- Test: `crates/classick/src/progress.rs` (unit tests module)

**Interfaces:**
- Produces: `IpcEvent::TrackStart { current, total, label, eta_secs: Option<u64> }`
  on the wire (field omitted when `None`); `EtaEstimator` with
  `fn record_track_done(&mut self)`, `fn eta_secs(&self, current: usize, total: usize) -> Option<u64>`.

- [ ] **Step 1: Write the failing ETA unit test**

Add to the `#[cfg(test)] mod tests` in `crates/classick/src/progress.rs`:

```rust
#[test]
fn eta_estimator_none_until_a_track_completes() {
    let mut e = EtaEstimator::new_at(std::time::Instant::now());
    // No completed tracks yet → no estimate.
    assert_eq!(e.eta_secs(1, 10), None);
}

#[test]
fn eta_estimator_projects_from_average_after_completions() {
    let start = std::time::Instant::now() - std::time::Duration::from_secs(20);
    let mut e = EtaEstimator::new_at(start);
    // 4 tracks done over ~20s → ~5s/track. 6 remaining → ~30s.
    for _ in 0..4 { e.record_track_done(); }
    let eta = e.eta_secs(5, 10).expect("estimate after completions");
    assert!((25..=35).contains(&eta), "eta {eta} not ~30s");
}

#[test]
fn eta_estimator_none_when_nothing_remains() {
    let start = std::time::Instant::now() - std::time::Duration::from_secs(10);
    let mut e = EtaEstimator::new_at(start);
    for _ in 0..10 { e.record_track_done(); }
    assert_eq!(e.eta_secs(10, 10), None, "no remaining tracks → no eta");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p classick eta_estimator`
Expected: FAIL — `cannot find type EtaEstimator`.

- [ ] **Step 3: Implement `EtaEstimator`**

Add near the top of `crates/classick/src/progress.rs` (after imports; it reuses
the whole-run-average logic already in `TuiState::eta`):

```rust
/// Whole-run-average sync ETA. Shared by the TUI and IPC progress backends so
/// both surface an identical estimate. Deliberately simple: elapsed time since
/// the first track divided by completed-track count, projected over the
/// remaining tracks. A rolling window is a possible later refinement.
pub struct EtaEstimator {
    started_at: Instant,
    done: usize,
}

impl EtaEstimator {
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// Test seam: inject the start instant so elapsed time is deterministic.
    pub fn new_at(started_at: Instant) -> Self {
        Self { started_at, done: 0 }
    }

    /// Call once per completed track (on `TrackDone`).
    pub fn record_track_done(&mut self) {
        self.done += 1;
    }

    /// Estimated seconds remaining given the 1-based `current` track and
    /// `total`. `None` until at least one track has completed, or when nothing
    /// remains — so the UI shows a plain "X of Y" early instead of a wild guess.
    pub fn eta_secs(&self, current: usize, total: usize) -> Option<u64> {
        let _ = current; // remaining is derived from done/total, not current
        if self.done == 0 || total == 0 {
            return None;
        }
        let remaining = total.saturating_sub(self.done);
        if remaining == 0 {
            return None;
        }
        let per_track = self.started_at.elapsed().as_secs_f64() / self.done as f64;
        Some((per_track * remaining as f64).round() as u64)
    }
}
```

- [ ] **Step 4: Run the ETA test to verify it passes**

Run: `cargo test -p classick eta_estimator`
Expected: PASS (3 tests).

- [ ] **Step 5: Add `eta_secs` to the wire type**

In `crates/classick/src/ipc.rs`, extend `IpcEvent::TrackStart`:

```rust
    /// Per-track start. See §4.7.
    TrackStart {
        current: usize,
        total: usize,
        label: String,
        /// Estimated seconds remaining (whole-run average). Omitted before the
        /// first track completes. Added in protocol 1.2.0.
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_secs: Option<u64>,
    },
```

In the same file's `from_progress`, set the field to `None` (the stateless
mapping can't estimate; `run_ipc` fills it in — next step):

```rust
            PE::TrackStart {
                current,
                total,
                label,
            } => IpcEvent::TrackStart {
                current: *current,
                total: *total,
                label: label.clone(),
                eta_secs: None,
            },
```

- [ ] **Step 6: Feed the ETA in `run_ipc`**

In `crates/classick/src/progress.rs`, inside `run_ipc`'s event loop, keep an
`EtaEstimator` and override `TrackStart`'s `eta_secs`. Replace the loop body
(the `for event in rx { ... }` block) with:

```rust
    let mut eta = EtaEstimator::new();
    for event in rx {
        let is_terminal = matches!(
            event,
            ProgressEvent::Finish { .. } | ProgressEvent::Paused
        );
        if matches!(event, ProgressEvent::TrackDone) {
            eta.record_track_done();
        }
        if let Some(mut ipc_event) = IpcEvent::from_progress(&event) {
            if let crate::ipc::IpcEvent::TrackStart { current, total, eta_secs, .. } = &mut ipc_event {
                *eta_secs = eta.eta_secs(*current, *total);
            }
            tracing::info!("ipc: emitting event: {ipc_event:?}");
            write_ipc_event(&ipc_event).context("ipc: event write failed")?;
        }
        if is_terminal {
            tracing::info!("ipc: received terminal event, exiting event loop");
            break;
        }
    }
```

- [ ] **Step 7: Update the protocol doc**

In `docs/ipc-protocol.md`: in the `track_start` row/section add the optional
`eta_secs` field (u64, seconds remaining, omitted before the first track
completes, "since 1.2.0"), and bump the stated current version from `1.1.0` to
`1.2.0` (the "current version is `1.1.0`" line near the top and any version
table). This is a minor (additive optional field) bump.

Also bump `PROTOCOL_VERSION` in `crates/classick/src/ipc.rs`:

```rust
pub const PROTOCOL_VERSION: &str = "1.2.0";
```

- [ ] **Step 8: Run the full crate test suite**

Run: `cargo test -p classick`
Expected: PASS. (Existing IPC tests still pass — the new field is
`skip_serializing_if`, so serialized output for a `None` ETA is unchanged.)

- [ ] **Step 9: Commit**

```bash
git add crates/classick/src/ipc.rs crates/classick/src/progress.rs docs/ipc-protocol.md
git commit -m "feat(ipc): daemon-computed sync ETA on track_start (protocol 1.2.0)"
```

---

### Task 2: `library_watcher` module

**Files:**
- Modify: `Cargo.toml` (workspace root — add `notify` to `[workspace.dependencies]`)
- Modify: `crates/classick/Cargo.toml` (depend on `notify`)
- Modify: `crates/classick/src/daemon/mod.rs` (module decl + debounce const)
- Create: `crates/classick/src/daemon/library_watcher.rs`
- Test: `crates/classick/src/daemon/library_watcher.rs` (inline)

**Interfaces:**
- Produces: `LibraryWatcher::spawn(source: Option<PathBuf>) -> (LibraryWatcher, tokio::sync::mpsc::UnboundedReceiver<()>)`
  — each `()` is a coalesced "source changed, consider rescanning" tick.
  `LibraryWatcher::rewatch(&mut self, source: Option<PathBuf>)` re-points the
  watch (used by SaveConfig). Dropping the `LibraryWatcher` stops watching.

- [ ] **Step 1: Add the `notify` dependency**

`notify` is the de-facto-standard cross-platform FS-notification crate for
Rust (FSEvents / ReadDirectoryChangesW / inotify), actively maintained and
widely adopted. In the workspace root `Cargo.toml` under
`[workspace.dependencies]` add:

```toml
notify = "6"
```

In `crates/classick/Cargo.toml` under `[dependencies]` add:

```toml
notify = { workspace = true }
```

- [ ] **Step 2: Declare the module + debounce constant**

In `crates/classick/src/daemon/mod.rs`, add the module declaration alongside
the other `pub mod` lines:

```rust
pub mod library_watcher;
```

and add a debounce window constant next to `DEVICE_DEBOUNCE_WINDOW`:

```rust
/// Quiet period after the last filesystem event before a watcher-triggered
/// library scan fires. Bulk file operations (a Lidarr import, a big copy) emit
/// many events; this coalesces them into one scan.
pub const LIBRARY_DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);
```

- [ ] **Step 3: Write the failing watcher test**

Create `crates/classick/src/daemon/library_watcher.rs` with only the test
module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn touching_a_file_delivers_a_change_tick() {
        let dir = std::env::temp_dir().join(format!("classick-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_watcher, mut rx) = LibraryWatcher::spawn(Some(dir.clone()));

        // Give the OS watch a beat to arm, then create a file.
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(dir.join("new.flac"), b"x").unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(got, Ok(Some(()))), "expected a change tick, got {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_source_yields_no_ticks() {
        let (_watcher, mut rx) = LibraryWatcher::spawn(None);
        let got = tokio::time::timeout(Duration::from_millis(400), rx.recv()).await;
        assert!(got.is_err(), "no watched path → no ticks (timeout expected)");
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p classick library_watcher`
Expected: FAIL — `cannot find type LibraryWatcher`.

- [ ] **Step 5: Implement `LibraryWatcher`**

Prepend to `crates/classick/src/daemon/library_watcher.rs` (above the test
module):

```rust
//! Filesystem watcher for the configured source library. On any change under
//! the source root it emits a coalesced tick; the runtime debounces those and
//! triggers the existing (crash-isolated, incremental) scan subprocess. A
//! sibling to `device_watcher` — same "background source → mpsc → runtime
//! select arm" shape.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Owns the `notify` watcher. Dropping it stops the OS watch. `rewatch`
/// re-points it when the configured source changes.
pub struct LibraryWatcher {
    watcher: Option<RecommendedWatcher>,
    current: Option<PathBuf>,
    tx: UnboundedSender<()>,
}

impl LibraryWatcher {
    /// Start watching `source` (if any). Returns the watcher handle plus the
    /// receiver of coalesced change ticks. The `notify` callback runs on the
    /// crate's own thread; it forwards a unit tick per event batch onto the
    /// tokio channel (the runtime does the time-based debounce).
    pub fn spawn(source: Option<PathBuf>) -> (Self, UnboundedReceiver<()>) {
        let (tx, rx) = mpsc::unbounded_channel::<()>();
        let mut me = Self { watcher: None, current: None, tx };
        me.rewatch(source);
        (me, rx)
    }

    /// Re-point the watch at `source` (or stop watching when `None`). Idempotent
    /// when the path is unchanged.
    pub fn rewatch(&mut self, source: Option<PathBuf>) {
        if source == self.current {
            return;
        }
        // Drop any existing watcher (stops the old watch), then build a new one.
        self.watcher = None;
        self.current = source.clone();
        let Some(path) = source else { return };
        if !path.exists() {
            tracing::warn!("library_watcher: source {} does not exist; not watching", path.display());
            return;
        }
        let tx = self.tx.clone();
        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            match res {
                // Any event is just a "something changed" nudge — the scan
                // itself diffs mtime/size, so we don't inspect the event kind.
                Ok(_) => { let _ = tx.send(()); }
                Err(e) => tracing::warn!("library_watcher: notify error: {e}"),
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("library_watcher: failed to create watcher: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
            tracing::warn!("library_watcher: failed to watch {}: {e}", path.display());
            return;
        }
        tracing::info!("library_watcher: watching {}", path.display());
        self.watcher = Some(watcher);
    }
}
```

- [ ] **Step 6: Run the watcher test to verify it passes**

Run: `cargo test -p classick library_watcher`
Expected: PASS (2 tests). If the touch test is flaky on the runner, the 5s
timeout is generous; FSEvents latency is well under that.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/classick/Cargo.toml crates/classick/src/daemon/mod.rs crates/classick/src/daemon/library_watcher.rs Cargo.lock
git commit -m "feat(daemon): notify-based library filesystem watcher"
```

---

### Task 3: Wire the watcher into the daemon runtime

**Files:**
- Modify: `crates/classick/src/daemon/runtime.rs`
- Test: `crates/classick/tests/daemon_runtime_integration.rs`

**Interfaces:**
- Consumes: `LibraryWatcher::spawn` / `rewatch` (Task 2), `start_scan_session`
  (existing, `runtime.rs`), `LIBRARY_DEBOUNCE_WINDOW` (Task 2).
- Produces: a runtime that, on a debounced source change and at startup, calls
  `start_scan_session` (which broadcasts `library_update` on completion).

- [ ] **Step 1: Write the failing integration test**

The suite already builds a sandbox with an injected fake `spawn_scan` that
records invocations. Add a test to `crates/classick/tests/daemon_runtime_integration.rs`
modelled on the existing scan tests (reuse the sandbox helper + the scan-spawn
counter). Skeleton (adapt names to the file's existing helpers):

```rust
#[tokio::test]
async fn watcher_change_triggers_one_scan_after_debounce() {
    // sandbox() sets up: temp config with a `source` dir, a fake spawn_scan
    // that increments an AtomicUsize and returns a Completed outcome, and a
    // running daemon on a unique pipe. See the existing scan_library test.
    let sb = sandbox_with_source().await;

    // Simulate a filesystem change under the configured source.
    std::fs::write(sb.source.join("added.flac"), b"x").unwrap();

    // Wait past the debounce window; exactly one scan should have spawned.
    tokio::time::sleep(LIBRARY_DEBOUNCE_WINDOW + std::time::Duration::from_millis(500)).await;
    assert_eq!(sb.scan_spawns.load(std::sync::atomic::Ordering::SeqCst), 1,
        "one coalesced scan after a source change");

    sb.shutdown().await;
}
```

If the existing sandbox doesn't expose a configured `source` dir or a
`scan_spawns` counter, extend it minimally (the fake `spawn_scan` closure that
Task-3 tests need mirrors the existing fake `spawn_sync`). Keep the change
additive.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p classick --test daemon_runtime_integration watcher_change`
Expected: FAIL — no watcher wired yet, `scan_spawns == 0`.

- [ ] **Step 3: Construct the watcher in `run_daemon_with_deps`**

In `crates/classick/src/daemon/runtime.rs`, after `device_rx` is created
(around the `let mut device_rx = deps.watcher.start();` line), add:

```rust
    // Filesystem watcher over the configured source library. Emits coalesced
    // change ticks; the select loop debounces them and triggers a scan.
    let initial_source = config_file::load(&config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source);
    let (mut library_watcher, mut library_rx) =
        crate::daemon::library_watcher::LibraryWatcher::spawn(initial_source);
    // Deadline used to debounce a burst of FS events into a single scan.
    let mut library_scan_deadline: Option<tokio::time::Instant> = None;
```

- [ ] **Step 4: Kick a startup scan**

Immediately after `spawn_library_count(&config_path, &internal_tx);` (the
existing cold-start line), add a one-shot startup scan so the library is fresh
the moment the app opens:

```rust
    // Refresh the library index once at startup so the browser is current
    // without a user action. Guarded/incremental like any scan.
    if config_file::load(&config_path).ok().flatten().and_then(|c| c.source).is_some() {
        start_scan_session(
            &mut state, &event_tx, &spawn_scan, &internal_tx,
            &mut cancel_tx_holder, &mut prompt_tx_holder, &mut pause_tx_holder,
            &connected, &config_path, &history, library_count_cache,
        );
    }
```

- [ ] **Step 5: Add the debounced watcher select arms**

Add two arms to the `tokio::select!` in the main loop. First, the change-tick
arm (sets/extends the debounce deadline):

```rust
            Some(()) = library_rx.recv() => {
                // Coalesce: (re)arm the debounce deadline. The timer arm below
                // fires the scan once the source has been quiet for the window.
                library_scan_deadline =
                    Some(tokio::time::Instant::now() + crate::daemon::LIBRARY_DEBOUNCE_WINDOW);
            }
```

Second, the debounce-timer arm. Use a helper future that resolves at the
deadline (or never, when no deadline is pending):

```rust
            _ = async {
                match library_scan_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                library_scan_deadline = None;
                // Only scan when idle + a source is configured; otherwise drop
                // (a sync in flight will refresh the count itself, and the next
                // change re-arms the deadline).
                let has_source = config_file::load(&config_path).ok().flatten()
                    .and_then(|c| c.source).is_some();
                if has_source && state.is_idle() {
                    tracing::info!("daemon: library watcher fired a scan after debounce");
                    start_scan_session(
                        &mut state, &event_tx, &spawn_scan, &internal_tx,
                        &mut cancel_tx_holder, &mut prompt_tx_holder, &mut pause_tx_holder,
                        &connected, &config_path, &history, library_count_cache,
                    );
                }
            }
```

Note: `biased;` is already set, so the timer arm is only polled when higher
arms aren't ready — fine here. Because `select!` re-evaluates the arm's future
each iteration, the `async { ... }` deadline block is rebuilt per loop, so an
extended deadline is honored.

- [ ] **Step 6: Re-point the watch when the source changes**

In `handle_client_command`'s `DaemonCommand::SaveConfig` arm, the runtime needs
to call `library_watcher.rewatch(new_source)`. `handle_client_command` doesn't
currently receive the watcher. Rather than thread it through that large
function, re-point the watch in the main loop right after the command returns.
Change the `client_cmd` select arm so that, after `handle_client_command(...)`,
it refreshes the watch from the (possibly updated) config:

```rust
                let should_exit = handle_client_command( /* ...unchanged args... */ );
                // A SaveConfig may have changed the source path; re-point the
                // watcher. rewatch() is a no-op when the path is unchanged.
                let latest_source = config_file::load(&config_path)
                    .ok().flatten().and_then(|c| c.source);
                library_watcher.rewatch(latest_source);
                if should_exit { break ExitReason::Shutdown; }
```

- [ ] **Step 7: Run the integration test to verify it passes**

Run: `cargo test -p classick --test daemon_runtime_integration watcher_change`
Expected: PASS — one scan spawns after the debounce window.

- [ ] **Step 8: Run the whole daemon suite (serialized — it pokes pipes)**

Run: `cargo test -p classick -- --test-threads=1`
Expected: PASS. Watch for the startup scan perturbing existing tests: if a
test's sandbox configures a `source`, it will now also see a startup scan.
Adjust those assertions to tolerate the extra scan (assert `>= 1` or reset the
counter after startup) rather than weakening the new test.

- [ ] **Step 9: Commit**

```bash
git add crates/classick/src/daemon/runtime.rs crates/classick/tests/daemon_runtime_integration.rs
git commit -m "feat(daemon): auto-scan library on startup and on watched source changes"
```

---

## PHASE B — macOS app shell & lifecycle

### Task 4: Dock app lifecycle

**Files:**
- Modify: `ui/macos/Info.plist`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift`
- Test: `ui/macos/Tests/ClassickTests/SmokeTests.swift`

**Interfaces:**
- Produces: app runs as a regular Dock app; closing the main window does not
  quit; `AppDelegate.shouldReopen` behavior verified.

- [ ] **Step 1: Flip `LSUIElement`**

In `ui/macos/Info.plist`, change:

```xml
    <key>LSUIElement</key>
    <false/>
```

- [ ] **Step 2: Write the failing lifecycle test**

In `ui/macos/Tests/ClassickTests/SmokeTests.swift` add:

```swift
@MainActor
func testAppDoesNotQuitWhenLastWindowCloses() {
    let delegate = AppDelegate()
    // Hybrid app: closing the main window must leave the app (and its daemon)
    // running in the Dock + menu bar.
    XCTAssertFalse(delegate.applicationShouldTerminateAfterLastWindowClosed(NSApplication.shared))
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd ui/macos && swift test --filter testAppDoesNotQuitWhenLastWindowCloses`
Expected: FAIL — method not implemented (defaults differ / compile error).

- [ ] **Step 4: Implement the lifecycle hooks**

In `ClassickApp.swift`, add to `AppDelegate`:

```swift
    /// Hybrid app: closing the main window leaves the app running in the Dock
    /// + menu bar so the daemon keeps syncing. Quit is explicit (⌘Q).
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    /// Re-open the main window when the Dock icon is clicked with no window
    /// visible. Returning true tells AppKit we handled it.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            NSApp.activate(ignoringOtherApps: true)
            // The WindowGroup restores its window on activation; if none exists,
            // openWindow (wired in Task 5) recreates it. AppKit reopens the
            // last closed WindowGroup window automatically here.
        }
        return true
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd ui/macos && swift test --filter testAppDoesNotQuitWhenLastWindowCloses`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ui/macos/Info.plist ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Tests/ClassickTests/SmokeTests.swift
git commit -m "feat(ui): run macOS app as a Dock app (close != quit)"
```

---

### Task 5: Main window scene + sidebar skeleton

**Files:**
- Create: `ui/macos/Sources/Classick/Views/MainWindow.swift`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift` (add `WindowGroup`, open-window plumbing)

**Interfaces:**
- Consumes: `AppModel` (existing), `AppDelegate` action closures.
- Produces: `MainWindow` view with `SidebarItem` selection enum
  (`case library`, `case device`, `case history`) and a `@ViewBuilder` detail
  switch. A `"main"` `WindowGroup` scene. `AppDelegate.openMainWindow` closure
  hook for the menu-bar "Open Classick".

- [ ] **Step 1: Create `MainWindow` with the split view skeleton**

Create `ui/macos/Sources/Classick/Views/MainWindow.swift`:

```swift
import SwiftUI

/// The primary app window: a source sidebar (Library / Devices / History), a
/// detail area, and a persistent bottom device row. Detail views and the
/// device row are filled in by later tasks; this establishes the shell.
struct MainWindow: View {
    var model: AppModel
    // Action closures injected from AppDelegate (wired in later tasks).
    var onSyncNow: () -> Void
    var onPause: () -> Void
    var onCancelSync: () -> Void
    var onResume: () -> Void
    var onRetry: () -> Void
    var onPreview: (SelectionMode, [SelectionRule]) -> Void
    var onSaveSelection: (SelectionMode, [SelectionRule]) -> Void
    var onScan: () -> Void
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void
    var onSetUp: () -> Void

    enum SidebarItem: Hashable { case library, device, history }
    @State private var selection: SidebarItem = .library

    var body: some View {
        NavigationSplitView {
            List(selection: $selection) {
                Section("Library") {
                    Label("Music Library", systemImage: "music.note.list").tag(SidebarItem.library)
                }
                if model.device != nil {
                    Section("Devices") {
                        Label(model.device?.name ?? model.device?.model ?? "iPod",
                              systemImage: "ipod").tag(SidebarItem.device)
                    }
                }
                Section("History") {
                    Label("Sync History", systemImage: "clock.arrow.circlepath").tag(SidebarItem.history)
                }
            }
            .navigationSplitViewColumnWidth(min: 200, ideal: 210, max: 260)
        } detail: {
            detail
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    DeviceRow(model: model,
                              onSyncNow: onSyncNow, onPause: onPause,
                              onCancelSync: onCancelSync, onResume: onResume,
                              onRetry: onRetry)
                }
        }
        .navigationTitle("Classick")
        .frame(minWidth: 860, minHeight: 560)
    }

    @ViewBuilder
    private var detail: some View {
        if model.needsFirstRunSetup {
            SetupCallToActionView(onSetUp: onSetUp)
        } else {
            switch selection {
            case .library:
                LibraryView(model: model, onScan: onScan,
                            onPreview: onPreview, onSaveSelection: onSaveSelection)
            case .device:
                DeviceView(model: model, onSaveSettings: onSaveSettings,
                           onForgetIpod: onForgetIpod, onBackfill: onBackfill)
            case .history:
                HistoryView(model: model)
            }
        }
    }
}

/// Shown in the detail area on a fresh, unconfigured install. Reuses the
/// existing setup flow via `onSetUp`.
struct SetupCallToActionView: View {
    var onSetUp: () -> Void
    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "ipod").font(.system(size: 48)).foregroundStyle(.secondary)
            Text("Welcome to Classick").font(.title2.bold())
            Text("Choose your music folder to get started.")
                .foregroundStyle(.secondary)
            Button("Set Up Classick…", action: onSetUp)
                .keyboardShortcut(.defaultAction)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
```

Note: `DeviceRow`, `LibraryView`, `DeviceView`, `HistoryView` are created in
Tasks 6–10. Until then this file won't compile on its own — Task 5 ends with a
compiling scene by adding **temporary stubs** (next step) that later tasks
replace.

- [ ] **Step 2: Add temporary stubs so the scene compiles**

Add to the bottom of `MainWindow.swift` (each is replaced by its real task):

```swift
// TEMPORARY stubs — replaced by Tasks 6–10. Kept minimal so the scene compiles
// and the window is runnable during Phase B.
struct DeviceRow: View {
    var model: AppModel
    var onSyncNow: () -> Void = {}
    var onPause: () -> Void = {}
    var onCancelSync: () -> Void = {}
    var onResume: () -> Void = {}
    var onRetry: () -> Void = {}
    var body: some View { Text("device row").padding(8) }
}
struct DeviceView: View {
    var model: AppModel
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void = { _, _ in }
    var onForgetIpod: () -> Void = {}
    var onBackfill: () -> Void = {}
    var body: some View { Text("device view") }
}
struct HistoryView: View { var model: AppModel; var body: some View { Text("history") } }
```

(`LibraryView` already exists conceptually via `ChooseMusicWindow`; Task 7
introduces the real `LibraryView`. For Phase B, temporarily alias it:)

```swift
struct LibraryView: View {
    var model: AppModel
    var onScan: () -> Void = {}
    var onPreview: (SelectionMode, [SelectionRule]) -> Void = { _, _ in }
    var onSaveSelection: (SelectionMode, [SelectionRule]) -> Void = { _, _ in }
    var body: some View { Text("library") }
}
```

- [ ] **Step 3: Add the `WindowGroup` scene + open-window plumbing**

In `ClassickApp.swift`, add a `WindowGroup` to `body: some Scene` (before the
`MenuBarExtra`), and give the menu bar an "Open Classick" that opens it. Add
inside `ClassickApp`:

```swift
    @Environment(\.openWindow) private var openWindow

    // in body: some Scene, add first:
    WindowGroup(id: "main") {
        MainWindow(
            model: appDelegate.model,
            onSyncNow: appDelegate.syncNow,
            onPause: appDelegate.pause,
            onCancelSync: appDelegate.cancelSync,
            onResume: appDelegate.resume,
            onRetry: appDelegate.retry,
            onPreview: { mode, rules in appDelegate.previewSelection(mode: mode, rules: rules) },
            onSaveSelection: { mode, rules in appDelegate.saveSelectionDirect(mode: mode, rules: rules) },
            onScan: appDelegate.rescan,
            onSaveSettings: appDelegate.saveSettings,
            onForgetIpod: appDelegate.forgetIpod,
            onBackfill: appDelegate.backfillRockbox,
            onSetUp: appDelegate.presentSetup
        )
    }
    .windowResizability(.contentMinSize)
```

Add the small `AppDelegate` helpers referenced above (some wrap existing sends):

```swift
    func rescan() {
        Task { await daemonClient.send(.scanLibrary) }
    }
    func previewSelection(mode: SelectionMode, rules: [SelectionRule]) {
        Task { await daemonClient.send(.previewSelection(mode: mode, rules: rules)) }
    }
    /// Persist a selection without the modal "Sync now?" alert (the persistent
    /// LibraryView auto-saves; the sync-on-change offer is handled inline there).
    func saveSelectionDirect(mode: SelectionMode, rules: [SelectionRule]) {
        Task { await daemonClient.send(.saveSelection(mode: mode, rules: rules)) }
    }
```

Also ensure the model requests library + selection on launch (previously done
in `presentChooseMusic`'s `onAppear`). In `applicationDidFinishLaunching`,
after `start()`, the event stream will deliver them once we ask — add a request
after the initial handshake by sending in the existing `eventTask` setup or on
first `MainWindow` appear. Simplest: give `MainWindow` an `.task { }` that
sends `getLibrary` + `getSelection` via a new `onAppearRequests` closure. Wire
`onAppearRequests: appDelegate.requestLibraryAndSelection` where:

```swift
    func requestLibraryAndSelection() {
        Task {
            await daemonClient.send(.getLibrary)
            await daemonClient.send(.getSelection)
        }
    }
```

and in `MainWindow.body` add `.task { onAppearRequests() }` (add the closure
property `var onAppearRequests: () -> Void = {}`).

- [ ] **Step 4: Wire "Open Classick" into the menu**

In `MenuContent` (fuller pass in Task 11) add, and pass an `onOpenMain` closure
from `ClassickApp` that calls `openWindow(id: "main")` wrapped with
`NSApp.activate(ignoringOtherApps: true)`. For this task, just add the button:

```swift
        Button("Open Classick", action: onOpenMain)
        Divider()
```

- [ ] **Step 5: Build and run the app; verify the window**

Run:

```bash
cd /Users/michael/Developer/classick && cargo build --release
ui/macos/bundle.sh
open ui/macos/Classick.app
```

Expected: a Dock icon appears; a main window opens with the sidebar (Library /
History, plus Devices when an iPod is attached) and placeholder detail + device
row. Closing the window leaves the app in the Dock/menu bar; "Open Classick"
reopens it.

- [ ] **Step 6: Commit**

```bash
git add ui/macos/Sources/Classick/Views/MainWindow.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Sources/Classick/Views/MenuContent.swift
git commit -m "feat(ui): main window scene with sidebar split view (stubs for detail)"
```

---

## PHASE C — macOS views

### Task 6: AppModel — retain history + thread ETA

**Files:**
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift`
- Test: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`, `WireCodecTests.swift`

**Interfaces:**
- Produces: `AppModel.history: [HistoryEntry]` (retained from `history_update`);
  `Phase.syncing(current, total, label, etaSecs: UInt64?)`;
  `SyncEvent.trackStart(current, total, label, etaSecs: UInt64?)`.

- [ ] **Step 1: Write the failing wire-decode test**

In `WireCodecTests.swift` add:

```swift
func testTrackStartDecodesOptionalEta() throws {
    let withEta = #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#
    let noEta = #"{"type":"track_start","current":1,"total":10,"label":"Y"}"#
    let d = JSONDecoder()
    if case let .trackStart(_, _, _, eta) = try d.decode(SyncEvent.self, from: Data(withEta.utf8)) {
        XCTAssertEqual(eta, 42)
    } else { XCTFail("expected trackStart") }
    if case let .trackStart(_, _, _, eta) = try d.decode(SyncEvent.self, from: Data(noEta.utf8)) {
        XCTAssertNil(eta)
    } else { XCTFail("expected trackStart") }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd ui/macos && swift test --filter testTrackStartDecodesOptionalEta`
Expected: FAIL — `trackStart` has 3 associated values, not 4.

- [ ] **Step 3: Add `etaSecs` to `SyncEvent.trackStart`**

In `WireModels.swift`, update the case and its decode:

```swift
    case trackStart(current: Int, total: Int, label: String, etaSecs: UInt64?)
```

Add `case etaSecs = "eta_secs"` to `SyncEvent.CodingKeys`, and in the
`"track_start"` decode arm:

```swift
        case "track_start":
            let current = try container.decode(Int.self, forKey: .current)
            let total = try container.decode(Int.self, forKey: .total)
            let label = try container.decode(String.self, forKey: .label)
            let etaSecs = try container.decodeIfPresent(UInt64.self, forKey: .etaSecs)
            self = .trackStart(current: current, total: total, label: label, etaSecs: etaSecs)
```

- [ ] **Step 4: Run the wire test to verify it passes**

Run: `cd ui/macos && swift test --filter testTrackStartDecodesOptionalEta`
Expected: PASS. (This will surface a compile error at `AppModel.applySyncEvent`'s
`.trackStart` pattern — fixed in Step 6.)

- [ ] **Step 5: Write the failing reducer tests**

In `AppModelReducerTests.swift` add:

```swift
@MainActor
func testSyncingPhaseCarriesEta() {
    let m = AppModel()
    m.apply(.deviceConnected(serial: "S", modelLabel: "iPod", drive: "/V", name: nil))
    m.apply(.configUpdate(source: "/m", daemon: nil,
                          ipod: IpodIdentity(serial: "S", modelLabel: "iPod", name: nil)))
    m.apply(.syncEvent(line: #"{"type":"track_start","current":5,"total":10,"label":"X","eta_secs":42}"#))
    if case let .syncing(current, total, _, eta) = m.phase {
        XCTAssertEqual(current, 5); XCTAssertEqual(total, 10); XCTAssertEqual(eta, 42)
    } else { XCTFail("expected syncing, got \(m.phase)") }
}

@MainActor
func testHistoryRetained() {
    let m = AppModel()
    let e = HistoryEntry(timestamp: "2026-07-14T10:00:00Z", durationSecs: 5,
                         trigger: "manual", outcome: "ok")
    m.apply(.historyUpdate(entries: [e]))
    XCTAssertEqual(m.history.count, 1)
    XCTAssertEqual(m.history.first?.trigger, "manual")
}
```

- [ ] **Step 6: Update `Phase`, add `history`, thread ETA**

In `AppModel.swift`:

Change the `Phase` case:

```swift
    case syncing(current: Int, total: Int, label: String, etaSecs: UInt64?)
```

Add the stored property:

```swift
    private(set) var history: [HistoryEntry] = []
```

In `apply`, replace the `.historyUpdate` no-op with retention:

```swift
        case let .historyUpdate(entries):
            history = entries
```

(remove `.historyUpdate` from the combined `case .hello, .historyUpdate, .unknown:` line.)

In `applySyncEvent`'s `.trackStart` arm:

```swift
        case let .trackStart(current, total, label, etaSecs):
            if isScanning {
                phase = .scanning(current: current, total: total)
            } else {
                phase = .syncing(current: current, total: total, label: label, etaSecs: etaSecs)
            }
```

In `computePhase`, update the two `.syncing` constructions to include the new
argument (preserve any existing ETA when rebuilding from status; a fresh
`.syncing` from status has none):

```swift
        if case .syncing = phase { return phase }
        return .syncing(current: 0, total: 0, label: "", etaSecs: nil)
```

Fix the `phaseIsSyncing` check (pattern still matches with the extra field —
`if case .syncing = phase` is unaffected).

- [ ] **Step 7: Fix the two other `.syncing` consumers**

`MenuContent.swift` `case let .syncing(current, total, label):` → add `, _`:

```swift
        case let .syncing(current, total, label, _):
```

(`DeviceRow` in Task 8 consumes the ETA; the temporary stub ignores it.)

- [ ] **Step 8: Run the reducer + full macOS test suite**

Run: `cd ui/macos && swift test`
Expected: PASS (new tests + existing).

- [ ] **Step 9: Commit**

```bash
git add ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift
git commit -m "feat(ui): retain sync history and thread sync ETA through the reducer"
```

---

### Task 7: LibraryView (persistent, auto-saving browser)

**Files:**
- Create: `ui/macos/Sources/Classick/Views/LibraryView.swift`
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift` (remove the `LibraryView` stub)

**Interfaces:**
- Consumes: `AppModel` (library/selection/preview), `SelectionDraft` (existing),
  `onScan`, `onPreview`, `onSaveSelection` closures.
- Produces: `LibraryView` — persistent browser; auto-saves the selection
  (debounced) instead of a modal Save.

- [ ] **Step 1: Create `LibraryView` by adapting `ChooseMusicWindow`**

Create `ui/macos/Sources/Classick/Views/LibraryView.swift`. Reuse the browser
body from `ChooseMusicWindow` (header mode picker, Artists/Genres, search,
rows, capacity/impact) **minus** the modal footer's Cancel/Save. Auto-save on
debounced draft change; keep the "seed once" latch. Core structure:

```swift
import SwiftUI

/// The always-present Music Library browser: mode picker + Artists/Genres +
/// checkbox outline. Edits a local SelectionDraft and auto-saves (debounced);
/// there is no modal Save/Cancel — this is a persistent view, not a sheet.
struct LibraryView: View {
    var model: AppModel
    var onScan: () -> Void
    var onPreview: (SelectionMode, [SelectionRule]) -> Void
    var onSaveSelection: (SelectionMode, [SelectionRule]) -> Void

    @State private var draft = SelectionDraft(mode: .all, rules: [])
    @State private var seededFromModel = false
    @State private var tab: Tab = .artists
    @State private var search = ""
    @State private var previewTask: Task<Void, Never>?
    @State private var saveTask: Task<Void, Never>?

    enum Tab: String, CaseIterable { case artists = "Artists", genres = "Genres" }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .onAppear { seedDraftIfNeeded() }
        .onChange(of: model.selection) { _, _ in seedDraftIfNeeded() }
        .onChange(of: draft) { _, d in
            schedulePreview(d)
            scheduleSave(d)
        }
    }

    // header / content / browser / rows / filtering: MOVE these from
    // ChooseMusicWindow.swift (the mode picker MUST stay enabled; browser
    // grays out in .all mode). Omit the `footer` and its Cancel/Save buttons.
    // Keep `schedulePreview`, `seedDraftIfNeeded`, `filteredArtists`,
    // `filteredGenres`, `artistRow`, `genreRow` as they are.

    /// Auto-save the selection ~500ms after the last edit. No modal. The
    /// daemon echoes selection_update; the seed latch prevents clobbering.
    private func scheduleSave(_ d: SelectionDraft) {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled else { return }
            onSaveSelection(d.mode, d.rules)
        }
    }
}
```

**This is a MOVE, not a duplication.** Take the helper methods and subviews
(`header`, `content`, `browser`, `artistRow`, `genreRow`, `emptyState`,
`schedulePreview`, `seedDraftIfNeeded`, `relativeDate`, `filteredArtists`,
`filteredGenres`, and the capacity/impact readout) out of
`ChooseMusicWindow.swift` and into `LibraryView`, dropping only the `footer`'s
Cancel/Save `HStack` and `onClose`. `ChooseMusicWindow.swift` and its
controller are **deleted in Task 11**, so across the full plan the browser code
lives in exactly one place — there is no permanent duplication. (The two files
briefly coexist between Task 7 and Task 11 only because `presentChooseMusic`
still references the old window until Task 11 retires it; a reviewer seeing that
transient overlap should treat it as expected, not as duplicated logic to flag.)
The capacity bar moves into the device row in Task 8; here, keep only the
per-row counts + the scanned/empty state.

- [ ] **Step 2: Remove the temporary `LibraryView` stub**

Delete the `struct LibraryView { ... Text("library") }` stub from
`MainWindow.swift` (Task 5 Step 2).

- [ ] **Step 3: Build and verify the browser renders + auto-saves**

Run:

```bash
cd /Users/michael/Developer/classick && cargo build --release && ui/macos/bundle.sh && open ui/macos/Classick.app
```

Expected: selecting the Library sidebar item shows the browser; toggling
checkboxes updates the impact preview and (after ~500ms) persists without a
Save button. Reopen the app → selection preserved.

- [ ] **Step 4: Commit**

```bash
git add ui/macos/Sources/Classick/Views/LibraryView.swift ui/macos/Sources/Classick/Views/MainWindow.swift
git commit -m "feat(ui): persistent auto-saving Library browser view"
```

---

### Task 8: DeviceRow (bottom strip, all states + ETA)

**Files:**
- Create: `ui/macos/Sources/Classick/Views/DeviceRow.swift`
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift` (remove `DeviceRow` stub)

**Interfaces:**
- Consumes: `AppModel` (`phase`, `device`, `deviceStorage`, `storageText`,
  `syncedCount`, `libraryCount`, `lastSync`, `selectionPreview`), action closures.
- Produces: `DeviceRow` — the pinned bottom strip.

- [ ] **Step 1: Create `DeviceRow`**

Create `ui/macos/Sources/Classick/Views/DeviceRow.swift`. Pure function of the
model, one branch per phase (idle / syncing / no-device / error), matching the
approved mockup:

```swift
import SwiftUI

/// The persistent bottom device strip. iPod identity + capacity/progress +
/// status + the primary action, driven entirely by `model.phase`.
struct DeviceRow: View {
    var model: AppModel
    var onSyncNow: () -> Void
    var onPause: () -> Void
    var onCancelSync: () -> Void
    var onResume: () -> Void
    var onRetry: () -> Void

    var body: some View {
        HStack(spacing: 14) {
            content
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .frame(maxWidth: .infinity)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .idle:
            deviceIdentity
            capacityBar
            Spacer()
            statusText("\(syncedSummary) synced", model.lastSync.map { "Last sync \(shortDate($0.timestamp))" })
            Button("Sync Now", action: onSyncNow).buttonStyle(.borderedProminent)

        case let .syncing(current, total, label, etaSecs):
            deviceIdentity
            VStack(alignment: .leading, spacing: 4) {
                ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
                    .frame(maxWidth: 320)
                Text("\(current) of \(total)\(label.isEmpty ? "" : " · \(label)")")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            statusText("Adding \(total) tracks", etaSecs.map { "~\(formatEta($0)) left" })
            Button("Pause", action: onPause)
            Button("Cancel", action: onCancelSync)

        case let .paused(synced, total):
            deviceIdentity
            Spacer()
            statusText("Paused", "\(synced)\(total.map { " of \($0)" } ?? "") synced")
            Button("Resume", action: onResume).buttonStyle(.borderedProminent)

        case .scanning:
            deviceIdentity
            ProgressView().controlSize(.small)
            Text("Updating library…").font(.caption).foregroundStyle(.secondary)
            Spacer()

        case .noDevice:
            Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
            VStack(alignment: .leading) {
                Text("No iPod connected").foregroundStyle(.secondary)
                Text("Plug in your iPod to sync").font(.caption).foregroundStyle(.tertiary)
            }
            Spacer()
            statusText("\(model.libraryCount ?? 0) tracks selected", nil)
            Button("Sync Now", action: onSyncNow).disabled(true)

        case .notConfigured:
            Image(systemName: "ipod").font(.title2).foregroundStyle(.tertiary)
            Text("iPod not set up").foregroundStyle(.secondary)
            Spacer()

        case let .error(message):
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.title3)
            VStack(alignment: .leading) {
                Text("Sync failed").foregroundStyle(.red).fontWeight(.semibold)
                Text(message).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer()
            Button("Retry", action: onRetry).buttonStyle(.borderedProminent)
        }
    }

    private var deviceIdentity: some View {
        HStack(spacing: 9) {
            Image(systemName: "ipod").font(.title2).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.device?.name ?? model.device?.model ?? "iPod").fontWeight(.semibold)
                if let s = model.storageText { Text(s).font(.caption).foregroundStyle(.secondary) }
            }
        }
    }

    @ViewBuilder private var capacityBar: some View {
        if let storage = model.deviceStorage {
            let used = Double(storage.total - storage.free)
            ProgressView(value: used, total: Double(storage.total))
                .frame(maxWidth: 260)
                .tint(.accentColor)
        }
    }

    private func statusText(_ big: String, _ sub: String?) -> some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(big).fontWeight(.semibold).font(.callout)
            if let sub { Text(sub).font(.caption).foregroundStyle(.secondary) }
        }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
    }

    private func formatEta(_ secs: UInt64) -> String {
        let f = DateComponentsFormatter()
        f.allowedUnits = secs < 3600 ? [.minute, .second] : [.hour, .minute]
        f.unitsStyle = .abbreviated
        return f.string(from: TimeInterval(secs)) ?? "\(secs)s"
    }

    private func shortDate(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .omitted, time: .shortened)
    }
}
```

- [ ] **Step 2: Remove the temporary `DeviceRow` stub** from `MainWindow.swift`.

- [ ] **Step 3: Build and verify each state**

Run: `cargo build --release && ui/macos/bundle.sh && open ui/macos/Classick.app`
Expected: idle shows capacity + Sync Now; starting a sync shows progress +
current track + "~N min left" once past track 1; unplugging shows the no-device
strip; a failed sync shows the red error strip + Retry.

- [ ] **Step 4: Commit**

```bash
git add ui/macos/Sources/Classick/Views/DeviceRow.swift ui/macos/Sources/Classick/Views/MainWindow.swift
git commit -m "feat(ui): bottom device row with per-phase states and ETA"
```

---

### Task 9: DeviceView (dashboard + device controls)

**Files:**
- Create: `ui/macos/Sources/Classick/Views/DeviceView.swift`
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift` (remove `DeviceView` stub)

**Interfaces:**
- Consumes: `AppModel` (device/storage/config/counts), `onSaveSettings`,
  `onForgetIpod`, `onBackfill`.
- Produces: `DeviceView` dashboard. Reuses the debounced-save pattern from
  `SettingsView.GeneralTab` for the device-scoped toggles.

- [ ] **Step 1: Create `DeviceView`**

Create `ui/macos/Sources/Classick/Views/DeviceView.swift`:

```swift
import SwiftUI

/// Device dashboard: identity, capacity, sync status, and device-scoped
/// controls (auto-sync, Rockbox compat, backfill, forget). Reads state the
/// daemon already sends; writes via save_config / backfill / forget_ipod.
struct DeviceView: View {
    var model: AppModel
    var onSaveSettings: (_ source: String?, _ daemon: DaemonSettings) -> Void
    var onForgetIpod: () -> Void
    var onBackfill: () -> Void

    @State private var autoSync = true
    @State private var rockboxCompat = false
    @State private var saveTask: Task<Void, Never>?

    var body: some View {
        Form {
            Section {
                LabeledContent("iPod", value: model.device?.name ?? model.device?.model ?? "—")
                if let s = model.storageText { LabeledContent("Capacity", value: s) }
                LabeledContent("Synced", value: syncedSummary)
                if let last = model.lastSync {
                    LabeledContent("Last sync", value: shortDate(last.timestamp))
                }
            }
            Section("Sync") {
                Toggle("Sync automatically on plug-in", isOn: Binding(
                    get: { autoSync }, set: { autoSync = $0; scheduleSave() }))
                Toggle("Rockbox compatibility (embed tags & art)", isOn: Binding(
                    get: { rockboxCompat }, set: { rockboxCompat = $0; scheduleSave() }))
                Button("Update artwork & metadata", action: onBackfill)
            }
            Section {
                Button("Remove this iPod", role: .destructive, action: onForgetIpod)
            }
        }
        .formStyle(.grouped)
        .onAppear(perform: syncFromConfig)
        .onChange(of: model.config) { _, _ in syncFromConfig() }
    }

    private var syncedSummary: String {
        if let total = model.libraryCount { return "\(model.syncedCount) of \(total)" }
        return "\(model.syncedCount)"
    }

    private func syncFromConfig() {
        guard let d = model.config?.daemon else { return }
        autoSync = d.enabled
        rockboxCompat = d.rockboxCompat
    }

    /// Debounced save that preserves the config fields this view doesn't edit
    /// (source, schedule, launch-at-login, notify) — same pattern as Settings.
    private func scheduleSave() {
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            let cur = model.config?.daemon
            let daemon = DaemonSettings(
                enabled: autoSync,
                autostartWithWindows: cur?.autostartWithWindows ?? false,
                firstSyncMode: cur?.firstSyncMode ?? "auto_apply",
                subsequentSyncMode: cur?.subsequentSyncMode ?? "auto_apply",
                scheduleMinutes: cur?.scheduleMinutes ?? 0,
                notifyOn: cur?.notifyOn ?? "all",
                rockboxCompat: rockboxCompat)
            onSaveSettings(nil, daemon)   // nil source: don't disturb the folder
        }
    }

    private func shortDate(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
}
```

- [ ] **Step 2: Remove the temporary `DeviceView` stub** from `MainWindow.swift`.

- [ ] **Step 3: Build and verify**

Run: `cargo build --release && ui/macos/bundle.sh && open ui/macos/Classick.app`
Expected: with an iPod attached, the Devices sidebar item shows the dashboard;
toggling auto-sync / Rockbox persists (confirm via Settings reflecting the same
value); "Remove this iPod" clears the pairing.

- [ ] **Step 4: Commit**

```bash
git add ui/macos/Sources/Classick/Views/DeviceView.swift ui/macos/Sources/Classick/Views/MainWindow.swift
git commit -m "feat(ui): device dashboard view with device-scoped controls"
```

---

### Task 10: HistoryView

**Files:**
- Create: `ui/macos/Sources/Classick/Views/HistoryView.swift`
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift` (remove `HistoryView` stub)

**Interfaces:**
- Consumes: `AppModel.history: [HistoryEntry]` (Task 6).
- Produces: `HistoryView` table.

- [ ] **Step 1: Create `HistoryView`**

Create `ui/macos/Sources/Classick/Views/HistoryView.swift`:

```swift
import SwiftUI

/// Read-only table of past syncs, newest first, from AppModel.history.
struct HistoryView: View {
    var model: AppModel

    private var rows: [Row] {
        model.history.reversed().enumerated().map { Row(id: $0.offset, entry: $0.element) }
    }
    private struct Row: Identifiable { let id: Int; let entry: HistoryEntry }

    var body: some View {
        if rows.isEmpty {
            ContentUnavailableView("No syncs yet", systemImage: "clock.arrow.circlepath",
                description: Text("Your sync history will appear here."))
        } else {
            Table(rows) {
                TableColumn("When") { r in Text(when(r.entry.timestamp)) }
                TableColumn("Trigger") { r in Text(trigger(r.entry.trigger)) }
                TableColumn("Outcome") { r in Text(r.entry.outcome.capitalized) }
                TableColumn("Duration") { r in Text(duration(r.entry.durationSecs)) }
            }
        }
    }

    private func when(_ iso: String) -> String {
        guard let d = ISO8601DateFormatter().date(from: iso) else { return iso }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
    private func trigger(_ t: String) -> String {
        switch t { case "plug_in": return "Plug-in"; default: return t.capitalized }
    }
    private func duration(_ secs: UInt64) -> String {
        let f = DateComponentsFormatter(); f.allowedUnits = [.minute, .second]; f.unitsStyle = .abbreviated
        return f.string(from: TimeInterval(secs)) ?? "\(secs)s"
    }
}
```

- [ ] **Step 2: Remove the temporary `HistoryView` stub** from `MainWindow.swift`.

- [ ] **Step 3: Ensure history is requested**

`AppModel` retains `history_update` (Task 6), but the daemon sends history in
reply to `get_history`. Add to `AppDelegate.requestLibraryAndSelection` (or a
sibling called on window appear) a `get_history` request. The wire command
enum `DaemonCommand` (Swift) may not yet have `getHistory`; if absent, add it:

```swift
    case getHistory(limit: Int)
```

with encode:

```swift
        case let .getHistory(limit):
            try container.encode("get_history", forKey: .type)
            try container.encode(limit, forKey: .limit)
```

(add `case limit` to `DaemonCommand.CodingKeys`). Then in `AppDelegate`:

```swift
    func requestHistory() { Task { await daemonClient.send(.getHistory(limit: 50)) } }
```

and call it from `MainWindow`'s `.task { onAppearRequests() }` closure
(fold `getHistory` into `requestLibraryAndSelection`, or add a call).

- [ ] **Step 4: Build and verify**

Run: `cargo build --release && ui/macos/bundle.sh && open ui/macos/Classick.app`
Expected: History sidebar item shows past syncs (or the empty state on a fresh
install); a completed sync appears after it finishes.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Views/HistoryView.swift ui/macos/Sources/Classick/Views/MainWindow.swift ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui): sync history table view"
```

---

### Task 11: Condensed menu-bar extra + remove old Choose Music windows

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/MenuContent.swift`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift`
- Delete: `ui/macos/Sources/Classick/Views/ChooseMusicWindow.swift`
- Delete: `ui/macos/Sources/Classick/Views/ChooseMusicWindowController.swift`

**Interfaces:**
- Consumes: `AppModel.phase`, `onOpenMain`, existing sync-action closures.
- Produces: condensed `MenuContent` (Open Classick, phase actions, Rescan, Settings, Quit).

- [ ] **Step 1: Rewrite `MenuContent` phase body condensed**

Update `MenuContent.swift`: add `var onOpenMain: () -> Void` and
`var onRescan: () -> Void`; replace the top of `body` so "Open Classick" is
first; drop the "Choose Music…" button (the Library view replaces it); keep the
glance status + phase actions; add "Rescan Library" as the escape hatch. Update
the `.syncing` pattern to the 4-tuple. Key diffs:

```swift
    var onOpenMain: () -> Void = {}
    var onRescan: () -> Void = {}

    var body: some View {
        if let daemonFatalError { Text(daemonFatalError); Divider() }
        Button("Open Classick", action: onOpenMain)
        Divider()
        phaseContent
        Divider()
        Button("Rescan Library", action: onRescan)
        Button("Settings…", action: onOpenSettings)
        Button("Check for Updates…", action: onCheckForUpdates)
        Button("Quit Classick") { NSApplication.shared.terminate(nil) }
    }
```

In `phaseContent`, remove the `Button("Choose Music…", action: onChooseMusic)`
line and change `case let .syncing(current, total, label):` to
`case let .syncing(current, total, label, _):`.

- [ ] **Step 2: Wire `onOpenMain` / `onRescan` and drop Choose Music plumbing**

In `ClassickApp.swift`'s `MenuBarExtra`, pass:

```swift
                onOpenMain: openMainWindow,
                onRescan: appDelegate.rescan,
```

Add the helper in `ClassickApp`:

```swift
    private func openMainWindow() {
        NSApp.activate(ignoringOtherApps: true)
        openWindow(id: "main")
    }
```

Remove the `onChooseMusic:` argument from the `MenuContent(...)` call and
delete `AppDelegate.presentChooseMusic` + the `chooseMusicController` property
and `saveSelection(mode:rules:)` modal (the persistent view + `saveSelectionDirect`
replace it; keep the "Sync now?" offer by moving it into `LibraryView`'s save
path if desired, or drop it per spec's auto-save decision — the spec keeps the
offer, so re-add a lightweight confirm in `LibraryView.scheduleSave` only when
`model.selectionPreview.adds + removes > 0 && model.device != nil`).

- [ ] **Step 3: Delete the obsolete window files**

```bash
git rm ui/macos/Sources/Classick/Views/ChooseMusicWindow.swift ui/macos/Sources/Classick/Views/ChooseMusicWindowController.swift
```

Fix any remaining references (compile errors point to them).

- [ ] **Step 4: Build + full test suite**

Run: `cd ui/macos && swift test && cd ../.. && cargo build --release && ui/macos/bundle.sh`
Expected: compiles; menu bar shows the condensed menu; "Open Classick" focuses
the window; "Rescan Library" triggers a scan.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui): condense menu-bar extra; retire standalone Choose Music window"
```

---

### Task 12: End-to-end verification + version bump

**Files:**
- Modify: `ui/macos/Info.plist` (version), `ui/macos/project.yml` if it carries the version
- Modify: `LEARNINGS.md`

- [ ] **Step 1: Full end-to-end on device**

Run:

```bash
cd /Users/michael/Developer/classick && cargo build --release && ui/macos/bundle.sh && open ui/macos/Classick.app
```

Manually verify against the spec:
- Dock icon present; window opens on launch.
- Close window → app stays in Dock + menu bar; daemon keeps running (menu shows
  status). Dock click / "Open Classick" reopens.
- Library view auto-saves selection; edits show impact; reopening preserves it.
- Add/remove a FLAC file in the source folder → within ~2s the library view
  refreshes (watcher), no manual Rescan.
- Plug iPod → Devices item + dashboard appear; device row shows capacity.
- Sync Now → device row shows progress + ETA; Pause/Cancel work.
- History view lists the completed sync.

- [ ] **Step 2: Bump the app version**

In `Info.plist` set `CFBundleShortVersionString` to `0.4.0` and bump
`CFBundleVersion`. (Do not run `release-macos.sh` — that's a separate,
user-driven release step.)

- [ ] **Step 3: Record learnings**

Append to `LEARNINGS.md` (one bullet each, check for duplicates first):
- Hybrid Dock app: `applicationShouldTerminateAfterLastWindowClosed → false`
  keeps the daemon alive when the main window closes; `WindowGroup(id:"main")`
  + `openWindow` reopen it from the menu bar.
- Library auto-refresh is a `notify` watcher in the daemon that triggers the
  existing scan subprocess (debounced ~1.5s) — not an in-daemon `update_index`
  (crash isolation). Startup also scans once.
- Sync ETA is daemon-side (`EtaEstimator`, whole-run average) on `track_start`
  `eta_secs`; inner sync-event protocol is now 1.2.0.

- [ ] **Step 4: Commit**

```bash
git add ui/macos/Info.plist LEARNINGS.md
git commit -m "chore(ui): bump macOS app to 0.4.0 (Dock app + main window)"
```

---

## Self-Review (completed by plan author)

- **Spec coverage:** §1 shell→Task 4/5; §2 layout→Task 5; §3 Library→Task 7;
  §4 Device→Task 9; §5 History→Task 10; §6 device row + ETA→Task 8 (UI) + Task 1
  (Rust); §7 first-run→Task 5 (`SetupCallToActionView`); §8 menu→Task 11; §9
  watcher→Tasks 2–3; §10 wire→Task 1; §11 tests→Tasks 1,2,3,4,6; §12 rollout→Task 12.
- **Type consistency:** `Phase.syncing` carries `etaSecs: UInt64?` everywhere
  (AppModel, MenuContent, DeviceRow); `SyncEvent.trackStart` 4-tuple consistent
  across WireModels + AppModel + tests; Rust `IpcEvent::TrackStart.eta_secs:
  Option<u64>` ↔ Swift `UInt64?`. `EtaEstimator` API (`new`/`new_at`/
  `record_track_done`/`eta_secs`) consistent between Task 1 impl and its tests.
- **Ordering:** Rust (Phase A) is independent and lands first; Phase B adds the
  shell with stubs so it always compiles; Phase C replaces stubs one view at a
  time, each independently runnable.
