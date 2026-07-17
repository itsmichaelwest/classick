# Trust Package Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the P0 trust features: per-device sync state, invisible iTunesDB auto-restore, free-space-aware album-atomic sync, empty-source hard error, an explicit Replace Library action, and an artwork audit + repair-marker invariant.

**Architecture:** All behavior lands in the Rust core (`crates/classick`); the daemon gains two additive wire surfaces (protocol 1.4.0 → 1.5.0, subprocess 1.2.0 → 1.3.0); the macOS app gains thin UI. Per-device state moves to `devices/<serial>/` under the config dir. The fit engine is a new pure module filtering the action plan before the apply loop. Artwork correctness is enforced by a dirty-marker file so every exit path (pause/cancel/crash) self-repairs on the next run.

**Tech Stack:** Rust (serde, libgpod FFI, tokio daemon), newline-delimited JSON IPC, Swift 6 / SwiftUI (macOS 15+).

**Spec:** `docs/superpowers/specs/2026-07-17-trust-package-design.md` — read it first.

## Global Constraints

- Base branch: `main` **after** `macos-desktop-app` (0.4.0) merges. Do not start implementation before that merge.
- Backup/restore is **invisible**: no GUI surface, no settings. The only manual surface is `--restore-db-backup`.
- Auto-restore triggers on **parse failure only** — never on heuristics.
- Fit engine: never split an album; deterministic first-fit in existing diff order; reserve = `max(512 MB, 2% of volume)`; if free space is unqueryable, sync without a budget (today's behavior).
- Empty source walk (0 audio files) is a **hard error**, never a plan. Selection filtering to zero is allowed (explicit user action).
- Replace Library is **irreversible** (files are deleted); UI confirmation is typed-device-name; CLI confirms interactively unless `--apply`.
- Sync is always automatic: no new confirmation gates on any sync path.
- Protocol bumps additive only: subprocess `1.2.0 → 1.3.0`, daemon `1.4.0 → 1.5.0`. `docs/ipc-protocol.md` updates in the **same commit** as the Rust wire types. Windows C# is out of scope (ignores additions safely).
- No `println!` outside examples — stdout IS the wire in IPC mode. Use `tracing`.
- Keep files ≤ ~500 LOC; the fit engine and device-state logic are **new modules**, not additions to `apply_loop.rs`.
- Rust: `anyhow::Result` + `.context(...)` at boundaries. Conventional Commits. Stage files by name (never `git add -A`). Never amend.
- Swift: after adding/removing any Swift file run `xcodegen generate` in `ui/macos` (bundle.sh does NOT run it). Wire field names are verbatim snake_case copies of Rust names.
- Build/test: `cargo test` from repo root; `cd ui/macos && swift test`. The daemon integration suite is Windows-gated; daemon-arm coverage on macOS comes from extracted pure functions + the manual gate.

---

## Stage A — Per-device state

### Task 1: `device_state` module — per-device dirs, path resolution, migration

**Files:**
- Create: `crates/classick/src/device_state.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod device_state;` alphabetically, after `pub mod config_file;`)

**Interfaces:**
- Produces:
  - `pub fn sanitize_serial(serial: &str) -> String` — uppercase, strip leading `0x`, keep only `[A-Za-z0-9_-]`, map anything else to `_`; empty input → `"UNKNOWN"`.
  - `pub fn device_dir(serial: &str) -> Result<PathBuf>` — `<config>/classick/devices/<sanitized>/`, created on demand.
  - `pub fn device_manifest_path(serial: &str) -> Result<PathBuf>` — `device_dir(serial)?.join("manifest.json")`.
  - `pub fn device_selection_path(serial: &str) -> Result<PathBuf>` — `device_dir(serial)?.join("selection.json")`.
  - `pub fn artwork_dirty_marker_path(serial: &str) -> Result<PathBuf>` — `device_dir(serial)?.join("artwork-dirty")` (used by Task 13).
  - `pub fn migrate_legacy_manifest(legacy_path: &Path, serial: &str) -> Result<PathBuf>` — if `legacy_path` exists and `device_manifest_path(serial)` does not, `fs::rename` it there (fall back to copy+delete across filesystems); returns the per-device path either way.
  - All path fns take an optional test override via a sibling `*_in(root: &Path, serial: &str)` variant (same pattern as `config_file::default_path` vs explicit-path fns); the no-suffix fns call the `_in` variant with `dirs::config_dir()?.join(crate::PROJECT_DIR)`.
- Consumes: `crate::PROJECT_DIR`, `dirs::config_dir()`.

- [ ] **Step 1: Write the failing tests** (in `device_state.rs` `#[cfg(test)] mod tests`, operating on tempdirs under `target/test-tmp/` with a per-test atomic counter, same pattern as `source.rs` walker tests):

```rust
#[test]
fn sanitize_serial_uppercases_and_strips_0x() {
    assert_eq!(sanitize_serial("0x000A27002138B0A8"), "000A27002138B0A8");
    assert_eq!(sanitize_serial("abc-123"), "ABC-123");
    assert_eq!(sanitize_serial("weird/serial:name"), "WEIRD_SERIAL_NAME");
    assert_eq!(sanitize_serial(""), "UNKNOWN");
}

#[test]
fn device_paths_nest_under_devices_dir() {
    let root = tempdir_under_target();
    let p = device_manifest_path_in(&root, "0xABC").unwrap();
    assert_eq!(p, root.join("devices").join("ABC").join("manifest.json"));
    assert!(p.parent().unwrap().is_dir(), "device_dir is created on demand");
}

#[test]
fn migrate_moves_legacy_manifest_once() {
    let root = tempdir_under_target();
    let legacy = root.join("manifest.json");
    std::fs::write(&legacy, r#"{"version":1,"tracks":[]}"#).unwrap();
    let dst = migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
    assert_eq!(dst, root.join("devices").join("SER1").join("manifest.json"));
    assert!(!legacy.exists(), "legacy file moved, not copied");
    assert!(dst.exists());
    // Second call: legacy gone, per-device present — no-op, same path back.
    let dst2 = migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
    assert_eq!(dst, dst2);
}

#[test]
fn migrate_never_clobbers_existing_device_manifest() {
    let root = tempdir_under_target();
    let legacy = root.join("manifest.json");
    std::fs::write(&legacy, r#"{"version":1,"tracks":[]}"#).unwrap();
    let dst = device_manifest_path_in(&root, "SER1").unwrap();
    std::fs::write(&dst, r#"{"version":1,"ipod_serial":"SER1","tracks":[]}"#).unwrap();
    migrate_legacy_manifest_in(&root, &legacy, "SER1").unwrap();
    let kept = std::fs::read_to_string(&dst).unwrap();
    assert!(kept.contains("SER1"), "existing per-device manifest wins; legacy left in place");
    assert!(legacy.exists());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p classick device_state` → FAIL (unresolved names).
- [ ] **Step 3: Implement the module** to the interface above. Migration rule: if the per-device manifest already exists, do nothing (leave legacy in place, log a `tracing::warn!`).
- [ ] **Step 4: Run** — `cargo test -p classick device_state` → PASS; full `cargo test` still green.
- [ ] **Step 5: Commit** — `git add crates/classick/src/device_state.rs crates/classick/src/lib.rs && git commit -m "feat(manifest): device_state module — per-device dirs, path sanitization, legacy migration"`

### Task 2: Sync uses the per-device manifest and writes `ipod_serial`

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (manifest load at `:132`, saves at `:650`/`:674`, `build_rebuild_manifest` at `:1353-1375`)
- Test: apply_loop unit tests module (bottom of file) + `crates/classick/src/manifest.rs` tests

**Interfaces:**
- Consumes: `device_state::{device_manifest_path, migrate_legacy_manifest, sanitize_serial}` (Task 1); the resolved device `identity` already in scope before `OwnedDb::open` (apply_loop.rs ~:318-334, the same identity fed to `sysinfo_provision`) — its serial string is the key.
- Produces: `manifest.ipod_serial = Some(sanitized_serial)` written on every save; `pub(crate) fn manifest_is_foreign(manifest: &Manifest, serial: &str) -> bool` (pure, unit-testable).

**Behavior:** after device identity resolution and before manifest load, compute `device_state::device_manifest_path(&serial)`, call `migrate_legacy_manifest(&config.manifest_path, &serial)`, and use the returned path for load and all saves this run. `config.manifest_path` keeps its current meaning (the legacy/root path, still overridable in tests); the per-device path derives from it at run time. On load, if `manifest.ipod_serial` is `Some(s)` and `s != sanitized_serial`, treat as foreign: log `tracing::warn!`, proceed as if the manifest were empty, and emit the existing recovery-hint copy pointing at `--rebuild-manifest`. `build_rebuild_manifest` now takes the serial and sets `ipod_serial: Some(serial)` (update `:1375`).

- [ ] **Step 1: Write failing tests** — `manifest_is_foreign` truth table (matching serial → false; `None` → false, legacy manifests adopt the device; mismatch → true); rebuild sets serial.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** (load-site wiring + save-site serial stamping + foreign check).
- [ ] **Step 4: `cargo test`** → PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(manifest): per-device manifest path + ipod_serial stamping + foreign-manifest guard"` (stage the two files by name).

### Task 3: Shared vs custom selection (`custom_selection` on IpodIdentity)

**Files:**
- Modify: `crates/classick/src/config_file.rs` (IpodIdentity), `crates/classick/src/apply_loop.rs` (`:118` `apply_to_sources` call site), `crates/classick/src/daemon/runtime.rs` (SaveConfig arm; selection command arms resolve the effective path), `docs/ipc-protocol.md` (§ config_update/save_config: new field)
- Test: config_file tests, new pure-fn tests

**Interfaces:**
- Produces: `IpodIdentity.custom_selection: bool` (`#[serde(default)]` — absent = false = shared; rides the existing `config_update`/`save_config` wire with **no new commands**); `pub fn effective_selection_path(identity: Option<&IpodIdentity>) -> Result<PathBuf>` in `crate::selection` — custom → `device_state::device_selection_path(serial)`, else `selection::default_selection_path()`.
- Seeding: in the daemon SaveConfig arm, when `custom_selection` flips false→true and the per-device file doesn't exist, copy the shared `selection.json` (if any) to `device_selection_path(serial)` before persisting. Flipping true→false leaves the custom file dormant.
- **Beware the 0.2.1 lesson:** every Swift `IpodIdentity`/`DaemonSettings` construction site must carry the new field or saves will silently reset it — Task 15 handles the Swift side; the Rust default keeps old clients harmless.

- [ ] **Step 1: Failing tests** — `IpodIdentity` TOML round-trip with/without the field (absent deserializes false); `effective_selection_path` resolution both modes; seed-on-switch copies shared → custom exactly once (pure helper `seed_custom_selection(shared: &Path, custom: &Path) -> Result<()>` so it's testable without the daemon).
- [ ] **Step 2: Verify failure. Step 3: Implement. Step 4: `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): per-device custom selection with shared default (additive wire field)"` — include the `docs/ipc-protocol.md` hunk in this commit.

## Stage B — Invisible backup & auto-restore

### Task 4: `restore_itunesdb_from_backup` + auto-restore on parse failure

**Files:**
- Modify: `crates/classick/src/ipod/db.rs` (below `backup_itunesdb`, `:500`), `crates/classick/src/apply_loop.rs` (both `OwnedDb::open` sites, `:126` and `:334`)
- Test: `crates/classick/tests/` new integration test with fixture DBs (`tests/fixtures/` already holds sync fixtures; add `corrupt-itunesdb.bin` = 64 random bytes, and reuse an existing valid fixture DB as the backup)

**Interfaces:**
- Produces in `db.rs`:
  - `pub const ITUNESDB_CORRUPT_ASIDE_NAME: &str = "iTunesDB.corrupt";`
  - `pub fn restore_itunesdb_from_backup(ipod_mount: &Path) -> Result<()>` — errors if backup missing; **opens the backup with libgpod first** (`OwnedDb::open_file`-style parse of the backup path — add a small `parse_check(path) -> Result<()>` helper using `itdb_parse_file`) and errors if it doesn't parse. To minimize the no-live-DB crash window (device detection requires iTunesDB to exist), the restore sequence is: (1) copy backup → `.tmp` via the same pattern as `backup_itunesdb`, (2) rename live DB → `iTunesDB.corrupt` (replace-existing), (3) rename `.tmp` → live `iTunesDB`. If step 1 or 3 fails, clean up `.tmp`.
  - `pub fn open_with_auto_restore(ipod_mount: &Path, on_restore: impl FnOnce()) -> Result<OwnedDb>` — try `OwnedDb::open`; on parse error, attempt `restore_itunesdb_from_backup` then re-open; `on_restore` fires only when a restore actually happened (callers log + emit the IPC log line + mark the history note). If restore also fails, return the **original** open error wrapped with context naming both remedies (`--rebuild-manifest`, `--restore-db-backup`).
- Consumes: `ITUNESDB_BACKUP_NAME`, `crate::ipod::layout::itunes_db_path`.

- [ ] **Step 1: Failing integration test** — build a fake mount dir (`iPod_Control/iTunes/`), place corrupt bytes as `iTunesDB` and a valid fixture as `iTunesDB.classick-backup`; assert `open_with_auto_restore` returns Ok, `iTunesDB.corrupt` exists with the corrupt bytes, live DB now parses, and the callback fired. Second test: corrupt backup too → Err, live DB untouched, no `.corrupt` file created.
- [ ] **Step 2: Verify failure. Step 3: Implement. Step 4: `cargo test` PASS.**
- [ ] **Step 5: Replace both `OwnedDb::open` call sites** in `apply_loop.rs` with `open_with_auto_restore`, wiring `on_restore` to `progress.log("Restored iPod database from backup after detecting corruption")`.
- [ ] **Step 6: `cargo test` PASS. Commit** — `git commit -m "feat(apply-loop): auto-restore iTunesDB from session backup on parse failure"`

### Task 5: `--restore-db-backup` CLI escape hatch

**Files:**
- Modify: `crates/classick/src/cli.rs` (new flag after `scan_library`, `conflicts_with_all = ["backfill_rockbox", "scan_library"]`), `crates/classick/src/config.rs` (field + `resolve_with` plumb, mirror `backfill_rockbox`), `crates/classick/src/orchestrator.rs` (run-and-exit branch, mirror the `--backfill-rockbox` dispatch)

**Interfaces:** `Cli.restore_db_backup: bool` → `Config.restore_db_backup: bool` → orchestrator resolves mount (existing preflight mount detection), calls `db::restore_itunesdb_from_backup`, prints outcome via `progress.log`, exits. Not persisted to config.toml (one-shot, like `backfill_rockbox`).

- [ ] **Step 1: Failing CLI tests** (`parses_restore_db_backup_flag`, conflicts test — same shape as `scan_library_conflicts_with_backfill_rockbox`). **Step 2: verify fail. Step 3: implement. Step 4: `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat: --restore-db-backup one-shot recovery flag"`

## Stage C — Fit engine

### Task 6: Core `free_space` query (move Windows impl, add Unix)

**Files:**
- Create: `crates/classick/src/free_space.rs`
- Modify: `crates/classick/src/daemon/device_storage.rs` (re-export/delegate to the new module; keep `StorageInfo` where the daemon wire expects it — move the struct to `free_space.rs` and `pub use crate::free_space::StorageInfo;` from `device_storage.rs` so `ipc_daemon.rs:10` keeps compiling), `crates/classick/src/lib.rs`, `crates/classick/Cargo.toml` (add `libc = "0.2"` for unix — it is already a transitive dep; make it direct, unix-only: `[target.'cfg(unix)'.dependencies] libc = "0.2"`)

**Interfaces:**
- Produces: `pub struct StorageInfo { pub total_bytes: u64, pub free_bytes: u64 }` (moved verbatim); `pub fn query(path: &Path) -> Option<StorageInfo>` — `#[cfg(windows)]` = the existing `GetDiskFreeSpaceExW` body moved from `device_storage.rs`; `#[cfg(unix)]` = `libc::statvfs` (`free = f_bavail * f_frsize`, `total = f_blocks * f_frsize`). Returns `None` on any failure (never errors).

- [ ] **Step 1: Failing test** — `query(Path::new("."))` returns `Some` with `total_bytes > 0 && free_bytes <= total_bytes`; `query` on a nonexistent path returns `None`. (Runs on both platforms.)
- [ ] **Step 2–4: fail → implement → `cargo test` PASS** (verify the daemon still compiles: the Windows body is a move, not a copy).
- [ ] **Step 5: Commit** — `git commit -m "refactor(daemon): hoist free-space query into core free_space module; add unix statvfs impl"`

### Task 7: `fit` module — album grouping + first-fit plan filter (pure)

**Files:**
- Create: `crates/classick/src/fit.rs`
- Modify: `crates/classick/src/lib.rs` (module decl + reserve constant next to the checkpoint constants: `pub const FIT_RESERVE_MIN_BYTES: u64 = 512 * 1024 * 1024;` and `pub const FIT_RESERVE_FRACTION: f64 = 0.02;` with a doc comment explaining the FAT32-at-100% rationale)

**Interfaces:**
- Produces:
  - `pub fn album_key(source_path: &Path, album_tag: Option<&str>) -> String` — album tag if non-empty, else the parent directory's lossy string.
  - `pub struct FitOutcome { pub kept: Vec<Action>, pub deferred: Vec<DeferredAlbum> }`, `pub struct DeferredAlbum { pub key: String, pub tracks: usize, pub bytes: u64 }`
  - `pub fn plan_fit(actions: Vec<Action>, budget_bytes: Option<u64>, album_tag_of: impl Fn(&Path) -> Option<String>) -> FitOutcome` — `budget_bytes: None` (unqueryable free space) keeps everything. Non-Add actions always kept. Adds grouped by `album_key` **preserving first-seen order**, then first-fit: keep the album if its summed `SourceEntry` sizes fit the remaining budget, else defer it whole.
  - `pub fn reserve_bytes(total_bytes: u64) -> u64` — `max(FIT_RESERVE_MIN_BYTES, total * FIT_RESERVE_FRACTION)`.
  - Budget computation lives with the caller (Task 8): `free + Σ(remove entry sizes) − reserve`, saturating at 0.
- Consumes: `manifest::Action`, `source::SourceEntry.size`. `album_tag_of` is injected so the apply loop can back it with a `library_index` lookup and tests can use a closure — `fit.rs` must NOT read the index itself.

- [ ] **Step 1: Failing tests** (build `Action::Add`s from synthetic `SourceEntry`s):

```rust
#[test]
fn no_budget_keeps_everything() { /* budget None → kept == input, deferred empty */ }

#[test]
fn album_never_splits() {
    // Album A: 3 tracks × 40 bytes; budget 100 → whole album deferred (120 > 100),
    // NOT 2 tracks kept.
}

#[test]
fn first_fit_skips_big_album_but_keeps_later_small_one() {
    // Order: A(120), B(60), C(50); budget 100 → A deferred, B kept (40 left), C deferred, D(30) kept.
}

#[test]
fn removes_and_modifies_always_kept() { /* mixed action list; only Adds participate */ }

#[test]
fn album_key_prefers_tag_falls_back_to_parent_dir() {
    assert_eq!(album_key(Path::new("/m/Artist/Album X/01.flac"), Some("Album X")), "Album X");
    assert_eq!(album_key(Path::new("/m/Artist/Album X/01.flac"), None), "/m/Artist/Album X");
}

#[test]
fn reserve_floor_and_fraction() {
    assert_eq!(reserve_bytes(10 * 1024 * 1024 * 1024), FIT_RESERVE_MIN_BYTES); // 2% of 10GB < 512MB
    assert_eq!(reserve_bytes(100 * 1024 * 1024 * 1024), (100.0 * 1024.0 * 1024.0 * 1024.0 * 0.02) as u64);
}
```

- [ ] **Step 2–4: fail → implement → PASS. Step 5: Commit** — `git commit -m "feat(apply-loop): fit module — album-atomic first-fit plan filter + reserve"`

### Task 8: Apply-loop integration + subprocess protocol 1.3.0

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (between `manifest::diff` (`:142`) and the review/apply phase; end-of-loop deferred retry; final summary), `crates/classick/src/ipc.rs` (`PROTOCOL_VERSION` → `"1.3.0"`; `Finish` gains optional fields), `crates/classick/src/progress.rs` (thread the new summary through `ProgressEvent::Finish` equivalents), `docs/ipc-protocol.md` (§4.11 + §1 version table — **same commit**)

**Interfaces:**
- `IpcEvent::Finish` becomes:

```rust
Finish {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_for_space: Option<SkippedForSpace>,   // pub struct { albums: usize, tracks: usize, bytes: u64 }
    #[serde(skip_serializing_if = "Option::is_none")]
    artwork: Option<ArtworkSummary>,              // pub struct { embedded: usize, eligible: usize, failed_sources: usize } — populated by Task 13
}
```

- Apply-loop flow: query `free_space::query(mount)`; compute budget (`None` free-space → `None` budget); `fit::plan_fit` the diffed actions with `album_tag_of` backed by a one-shot `library_index` load (`Option<LibraryIndex>`; absent index → closure returns `None`, directory fallback engages); run the apply loop over `kept`; track actual bytes written per Add (post-transcode file size — already stat'd for the DB entry) in a running tally; after the main loop, if `deferred` is non-empty re-query free space and run one more `plan_fit` over the deferred albums' original Add actions, appending newly-fitting ones to the work list (single retry pass, no loop); whatever still doesn't fit becomes the `SkippedForSpace` rollup on Finish.
- `--dry-run` prints the deferral outcome in the plan summary but writes nothing (existing dry-run path).

- [ ] **Step 1: Failing wire tests** in `ipc.rs` (Finish with/without the new fields serializes; absent fields omitted for old-client compat) + apply-loop unit test for budget math (`budget = free + removes − reserve`, saturating).
- [ ] **Step 2–4: fail → implement → `cargo test` PASS.**
- [ ] **Step 5: Update `docs/ipc-protocol.md`** (§1 table: subprocess 1.3.0; new §4.11 fields with JSON examples). **Commit** — `git commit -m "feat(ipc): subprocess 1.3.0 — fit engine wired into apply loop, skipped_for_space on finish"`

### Task 9: Daemon surfacing + protocol 1.5.0

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs` (`DAEMON_PROTOCOL_VERSION` → `"1.5.0"`), `crates/classick/src/daemon/history.rs` (`SyncSummary` gains `#[serde(default)] pub skipped_for_space_tracks: usize, #[serde(default)] pub skipped_for_space_bytes: u64, #[serde(default)] pub artwork_failed_sources: usize`), `crates/classick/src/daemon/runtime.rs` (populate from the subprocess Finish event where the existing summary is recorded), `docs/ipc-protocol.md` (§7 — same commit)

- [ ] **Step 1: Failing tests** — history entry round-trips with the new fields; pre-existing `history.json` entries (fields absent) deserialize to zeros; `DAEMON_PROTOCOL_VERSION == "1.5.0"` (update the existing `protocol_version_is_1_4_0` test).
- [ ] **Step 2–4: fail → implement → PASS. Step 5: Commit** (include protocol doc) — `git commit -m "feat(daemon): protocol 1.5.0 — skipped-for-space + artwork summary on history"`

## Stage D — Empty-source error & Replace Library

### Task 10: Empty source walk is a hard error

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (immediately after the source walk, **before** `selection::apply_to_sources` at `:118`)

**Behavior:** if the walk returned zero entries, `bail!` with exactly: `Source library at {path} contains no audio files — not syncing. If you meant to empty this iPod, use Replace Library.` The error flows through the existing error-event → daemon-error-state path untouched. Selection filtering to zero remains allowed.

- [ ] **Step 1: Failing test** — call the walk-then-guard helper (extract `fn guard_nonempty_walk(sources: &[SourceEntry], root: &Path) -> Result<()>` so it's unit-testable) with an empty slice → Err containing "contains no audio files"; non-empty → Ok. Also assert a selection that filters everything still proceeds (existing `apply_to_sources` test extended).
- [ ] **Step 2–4: fail → implement → PASS. Step 5: Commit** — `git commit -m "feat(apply-loop): empty source walk is a hard error, never a removal plan"`

### Task 11: `--replace-library` core mode

**Files:**
- Modify: `crates/classick/src/cli.rs` (flag; conflicts with `backfill_rockbox`, `scan_library`, `restore_db_backup`, `dry_run`, `rebuild_manifest`), `crates/classick/src/config.rs`, `crates/classick/src/apply_loop.rs` (new `pub fn replace_library(config: &mut Config, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<RunOutcome>`), `crates/classick/src/orchestrator.rs` (dispatch)
- Reference: `crates/classick/examples/wipe-tracks.rs` (the proven wipe sequence: `itdb_playlist_remove_track(NULL, t)` then `itdb_track_remove`, deleting the on-disk file via `itdb_filename_on_ipod` first, `g_free` the path)

**Interfaces:**
- `replace_library` sequence: session backup (existing) → confirmation (below) → open DB (with auto-restore) → wipe every track + its file → `db.write()` → reset manifest to `Manifest::empty()` with `ipod_serial` stamped, save → fall through to the normal `run()` sync of the effective selection.
- Confirmation: reuse the existing `try_with_prompt`/`Progress::prompt` machinery — a single prompt `This erases all N tracks on the iPod, then syncs your selection. This cannot be undone.` with options `["Erase and sync", "Abort"]`; plain/non-TTY mode without `--apply` falls into the out-of-range-default → Abort (the Phase-3.z rule); `--apply` skips the prompt. Daemon mode (Task 12) sends the command only after the UI's typed confirmation, and spawns with `--replace-library --apply`.

- [ ] **Step 1: Failing CLI tests** (flag parse + conflict matrix). **Step 2–4: fail → implement → PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(apply-loop): --replace-library explicit erase-and-sync mode"`

### Task 12: Daemon `replace_library` command

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs` (`DaemonCommand::ReplaceLibrary`), `crates/classick/src/daemon/runtime.rs` (arm mirrors `BackfillRockbox`: reject with `SyncRejected { reason: AlreadySyncing }` when busy, else spawn the subprocess with `--replace-library --apply`; history trigger records it), `docs/ipc-protocol.md` (same commit)

- [ ] **Step 1: Failing test** — `{"type":"replace_library"}` deserializes (same shape as `backfill_rockbox_deserializes`). **Step 2–4: fail → implement → PASS.** (Daemon-arm behavior is covered by the Windows-gated integration suite pattern; on macOS rely on the pure deserialize test + manual gate.)
- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): replace_library command (protocol 1.5.0)"`

## Stage E — Artwork

### Task 13: Artwork dirty-marker invariant + summary

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (checkpoint sites; the artwork-refresh gate at `:705-748`; pause/cancel exits), `crates/classick/src/device_state.rs` (marker path from Task 1)

**Behavior (mechanism is diagnosis-independent, so it lands before the on-device diagnosis):**
- When a checkpoint `db.write()` runs mid-loop, create the marker file `devices/<serial>/artwork-dirty` (write once; cheap `Path::exists` guard).
- The end-of-run `rebuild_apple_artwork` gate (`:716`) becomes: run the rebuild when `(changed > 0 && unchanged > 0)` **or the marker exists** — i.e. a previous pause/cancel/crash left checkpointed writes unrepaired, so this run repairs even if its own diff is all-Unchanged.
- Delete the marker only after `rebuild_apple_artwork` returns Ok (or when the run completed with no checkpoints and no rebuild needed).
- Pause/cancel paths change **nothing else** — they stay fast; the marker makes the *next* run repair. This satisfies the spec invariant ("every exit path repairs before or at the next session") without slowing pause.
- Populate `ArtworkSummary { embedded, eligible, failed_sources }` (wire struct from Task 8) from the run's per-track art outcomes and emit on Finish; failures logged per-track with the source path and reason (never silent).

- [ ] **Step 1: Failing tests** — extract the gate into `pub(crate) fn should_rebuild_artwork(changed: usize, unchanged: usize, marker_present: bool) -> bool` and unit-test the truth table (incl. `(0, N, true) → true`, the pause-then-noop-resume case); marker create/delete round-trip via `device_state` tempdir.
- [ ] **Step 2–4: fail → implement → PASS. Step 5: Commit** — `git commit -m "fix(apply-loop): artwork dirty-marker — paused/cancelled syncs repair art on next run"`

### Task 14: `--verify-artwork` audit mode

**Files:**
- Modify: `crates/classick/src/cli.rs` (flag, conflicts with the other one-shots), `crates/classick/src/config.rs`, `crates/classick/src/orchestrator.rs` (dispatch)
- Create: `crates/classick/src/art_audit.rs` (productize `examples/art-audit.rs`)

**Interfaces:** `pub fn verify_artwork(config: &Config, progress: &Progress) -> Result<ArtAuditReport>` — for every manifest entry with a known source: source-has-embedded-art (lofty/ffprobe via the existing probe path) vs DB-track `has_artwork` vs expected ithmb presence on the mount; `pub struct ArtAuditReport { pub checked: usize, pub ok: usize, pub failures: Vec<ArtAuditFailure> }`, `pub struct ArtAuditFailure { pub source_path: PathBuf, pub reason: String }` (reasons: `"source has no embedded art"`, `"db track has_artwork=0"`, `"ithmb file missing: F1069_1.ithmb"`). Output via `progress.log` lines + non-zero exit when failures > 0 (scriptable).

- [ ] **Step 1: Failing tests** — report aggregation from synthetic inputs (pure fns; FFI-touching parts covered by the on-device gate); CLI flag parse/conflicts.
- [ ] **Step 2–4: fail → implement → PASS. Step 5: Commit** — `git commit -m "feat: --verify-artwork audit mode (diagnostic + regression harness)"`

### Task 15: ⛔ DIAGNOSIS CHECKPOINT (user + maintainer, on-device) — plan amendment point

**Not a coding task.** With Tasks 13–14 built:
1. User runs `--verify-artwork` against the current device state → captures the failure list.
2. Reproduce the two reported symptoms: (a) art blanking after an incremental sync (test with a pause→resume cycle to confirm/refute the checkpoint hypothesis — Task 13 should already fix this class; verify); (b) the specific never-art albums — collect those source files as fixtures.
3. **Amend this plan** with concrete fix tasks for whatever (b) reveals (likely: normalize-step decode failures on specific image shapes — fixture-driven tests in `artwork.rs`). Do not proceed to Stage F sign-off with unexplained audit failures.

## Stage F — macOS UI

### Task 16: Swift wire mirrors + reducer

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift` (IpodIdentity gains `custom_selection: Bool` — **defaulted in the decoder AND carried by every construction site**, per the 0.2.1 wizard-clobber lesson; `Finish` gains optional `skipped_for_space` + `artwork` structs; `replace_library` command case), `ui/macos/Sources/Classick/Model/AppModel.swift` (reducer state for skipped/artwork summaries), matching tests in `ui/macos/Tests/`
- Run `xcodegen generate` if any file is added.

- [ ] **Step 1: Failing Swift tests** — decode `finish` with and without the new fields; decode `ipod` identity without `custom_selection` (defaults false); encode `replace_library`; **grep test**: a test asserting every `IpodIdentity(` construction site count matches the number of sites passing `customSelection:` (or simpler: unit test that `AppDelegate.setupDaemonSettings`-style save paths preserve a true value — mirror `testSetupWizardPreservesRockboxCompat`).
- [ ] **Step 2–4: fail → implement → `swift test` PASS. Step 5: Commit** — `git commit -m "feat(ui): wire mirrors for protocol 1.5.0 (custom selection, skipped-for-space, artwork summary, replace_library)"`

### Task 17: Device view UI — Replace Library + selection-mode toggle + skipped-for-space copy

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/DeviceView.swift` (device-scoped controls: "Replace Library…" destructive button → confirmation sheet with a `TextField` armed only when input == device name, calls `client.send(.replaceLibrary)`; a `Picker("Selection", selection:)` with Shared/Custom writing `custom_selection` through the existing SaveConfig path), `ui/macos/Sources/Classick/Views/DeviceRow.swift` ("Synced N of M — X albums didn't fit (Y GB)" line when `skipped_for_space` present; an artwork line "Art missing for X tracks" when `artwork.failed_sources > 0` or `embedded < eligible`), `ui/macos/Sources/Classick/Views/SettingsView.swift` (one static reassurance line: "Classick backs up your iPod's database before every sync." — spec §2's only visible trace; add the same line to the setup wizard's final page)
- Test: reducer/format tests (`ui/macos/Tests/`), e.g. `testReplaceConfirmationArmsOnlyOnExactName`, `testSkippedForSpaceLabelFormatting` (bytes → GB one-decimal, existing byte-formatter reuse).

- [ ] **Step 1: Failing tests → Step 2–4: implement → `swift test` PASS**, plus `ui/macos/bundle.sh` builds.
- [ ] **Step 5: Commit** — `git commit -m "feat(ui): Replace Library (typed confirmation), selection mode toggle, skipped-for-space device row"`

### Task 18: Manual on-device gate (user)

Checklist (record outcomes in `LEARNINGS.md` / the PR):
- [ ] Corrupt the live iTunesDB deliberately (truncate it) with a valid backup present → next sync auto-restores, logs the restore line, completes; Music.app still reads the device.
- [ ] Over-full sync: select more than free space → album-atomic deferral observed, end-of-run retry fills newly-freed space, Device row shows the skipped summary; no partial albums on device.
- [ ] Replace Library on a device carrying foreign (iTunes-era) tracks → typed confirmation → device ends with exactly the selection; Music.app reads it.
- [ ] Artwork invariant: fresh sync → pause mid-sync → resume → `--verify-artwork` reports 0 failures; cancelled-then-rerun likewise.
- [ ] Empty-source: point source at an empty dir → sync errors with the new copy; nothing removed.
- [ ] `--restore-db-backup` round-trip on real hardware.

---

## Self-review notes (kept for executors)

- Spec §1 → Tasks 1–3; §2 → Tasks 4–5; §3 → Tasks 6–8 (+9 surfacing); §4 → Tasks 10–12; §5 → Tasks 13–15 (+4 invariants verified in Task 18); §6 → Tasks 3, 8, 9, 12, 16 (doc updates ride each wire commit). Music.app read-compat lives in Task 18's checklist.
- The fit engine's `album_tag_of` indirection exists so `fit.rs` stays pure and index-free — do not "simplify" by reading `library_index.json` inside `fit.rs`.
- Task 13 lands **before** diagnosis on purpose: the marker mechanism is correct regardless of what diagnosis finds, and it converts the strongest hypothesis into a fix without slowing pause.
- Windows UI intentionally untouched; every wire change is additive and the C# client checks major version only.
