# Playlists Core & Wire (Plan A of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Playlists (manual .m3u8 + smart rules) as first-class core objects, per-device config v2 (selection/subscriptions/settings), the `scope ∪ playlists` sync planner, real iTunesDB playlists, the iPod mirror, and daemon protocol 1.6.0 — fully testable without the new UI.

**Architecture:** New pure modules (`playlist`, `playlist_rules`) own parsing/evaluation; `device_state` grows the v2 config files with one-shot migrations; the apply-loop plan phase unions subscribed-playlist tracks into the source set *before* the manifest diff so everything downstream (fit, artwork, checkpoints) is untouched; a post-track-loop reconcile step writes Classick-managed iTunesDB playlists by name and never touches foreign ones.

**Tech Stack:** Rust (serde, lofty-backed library_index, libgpod FFI), newline-delimited JSON IPC.

**Spec:** `docs/superpowers/specs/2026-07-17-library-playlists-devices-design.md` — read it first. Companion UI plan: `2026-07-17-macos-app-restructure.md` (Plan B; depends on this plan's wire).

## Global Constraints

- **Base:** `main` after the `trust-package` branch merges (this plan builds on its `devices/<serial>/` state, fit engine, and Replace flow). Do not start before that merge.
- **The source library is read-only. Nothing in this plan may write inside `config.source`.**
- Playlist files live in `<config>/classick/playlists/`; the iPod mirror lives at `iPod_Control/classick/playlists/` on the device. Adopt-from-mirror only when the host playlists dir is EMPTY; local wins on divergence (warn).
- Manual playlists = `.m3u8` (UTF-8, `#EXTM3U`, `#PLAYLIST:<name>`, source-relative paths, forward slashes on the wire/disk — normalize on read). Smart = `<slug>.rules.json`.
- Device content = `scope ∪ subscribed playlists` (union computed on source paths pre-diff). Subscribed tracks always sync; playlists always appear on the device.
- Foreign (non-Classick) iTunesDB playlists are never modified or deleted; the MPL is never treated as a Classick playlist (`itdb_playlist_is_mpl` guard).
- Playlist read failures are fail-visible (per-playlist error surfaced) but never fail the sync; smart rules matching zero tracks = valid empty playlist; mirror write failure = warn only.
- Per-device settings/selection supersede globals: migrations are one-shot, seeded from existing global values; deprecated fields (`custom_selection`, global `rockbox_compat`/auto-sync semantics) stay wire/config-tolerated.
- Daemon protocol 1.5.0 → **1.6.0**, additive only; `docs/ipc-protocol.md` updated in the SAME commit as wire types. Subprocess protocol unchanged (1.3.0). Windows client ignores everything (major-check only).
- No `println!` (stdout is the wire in IPC mode); `anyhow::Result` + `.context(...)`; `tracing` for logs. TDD per task. Conventional Commits; stage by name; never amend. Files ≤ ~500 LOC — new logic goes in new modules, NOT into `apply_loop.rs` (which is already over budget; its planned split is a separate follow-up).
- Fake-mount integration tests must pre-create `iPod_Control/Music/F00` (see LEARNINGS: libgpod requires existing F-dirs).

---

## Stage A — Playlist model (pure)

### Task 1: `playlist` module — types, M3U8 round-trip, slugs, store

**Files:**
- Create: `crates/classick/src/playlist.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod playlist;` alphabetically, after `pub mod manifest;`)

**Interfaces:**
- Produces:
  - `pub fn slugify(name: &str) -> String` — lowercase, alphanumerics kept, runs of anything else → single `-`, trimmed; empty result → `"playlist"`. Uniqueness is the store's job, not slugify's.
  - `pub enum Playlist { Manual(ManualPlaylist), Smart(SmartPlaylist) }` with `pub fn name(&self) -> &str`, `pub fn slug(&self) -> &str`.
  - `pub struct ManualPlaylist { pub slug: String, pub name: String, pub tracks: Vec<PathBuf> }` — tracks are SOURCE-RELATIVE paths.
  - `pub struct SmartPlaylist { pub slug: String, pub name: String, pub rules: crate::playlist_rules::SmartRules }` (rules type defined in Task 2 — for THIS task stub it as a `#[derive(Serialize,Deserialize,...)] pub struct SmartRules;` placeholder in a new `playlist_rules.rs` with a `// Task 2 fills this` doc comment so Task 1 compiles standalone).
  - `pub fn parse_m3u8(text: &str, slug: &str) -> Result<ManualPlaylist>` — tolerates BOM, CRLF, `#EXTINF` lines (ignored), blank lines; `#PLAYLIST:` sets name (fallback: slug); backslashes in paths normalized to `/`.
  - `pub fn render_m3u8(p: &ManualPlaylist) -> String` — `#EXTM3U\n#PLAYLIST:{name}\n` + one relative path per line, forward slashes.
  - `pub struct PlaylistStore { root: PathBuf }` with `pub fn open(root: PathBuf) -> Result<Self>` (creates dir), `pub fn default_root() -> Result<PathBuf>` (`<config>/classick/playlists/`), `pub fn list(&self) -> Result<Vec<Playlist>>` (reads every `*.m3u8` + `*.rules.json`; unreadable file → skipped + pushed onto `pub fn last_errors(&self) -> &[(PathBuf, String)]`), `pub fn load(&self, slug: &str) -> Result<Option<Playlist>>`, `pub fn save(&self, p: &Playlist) -> Result<()>` (atomic tmp+rename; manual→`<slug>.m3u8`, smart→`<slug>.rules.json`), `pub fn delete(&self, slug: &str) -> Result<bool>`, `pub fn unique_slug(&self, name: &str) -> Result<String>` (slugify, then `-2`, `-3`… on collision with EITHER file kind).
  - `pub fn resolve_manual(p: &ManualPlaylist, source_root: &Path, existing: &dyn Fn(&Path) -> bool) -> (Vec<PathBuf>, usize)` — absolute paths of tracks whose file exists (checked via injected closure so tests need no fs), plus missing-count.

- [ ] **Step 1: Failing tests** (`#[cfg(test)]` in playlist.rs; tempdirs per the `device_state.rs` counter pattern):

```rust
#[test]
fn slugify_basics() {
    assert_eq!(slugify("Favorites"), "favorites");
    assert_eq!(slugify("Bla Bla Bla!"), "bla-bla-bla");
    assert_eq!(slugify("日本語のみ"), "playlist"); // non-ascii-alnum collapses away
    assert_eq!(slugify("  --  "), "playlist");
}

#[test]
fn m3u8_round_trip_preserves_order_and_name() {
    let p = ManualPlaylist { slug: "gym".into(), name: "Gym".into(),
        tracks: vec!["Artist/Album/01.flac".into(), "B/C/02.flac".into()] };
    let parsed = parse_m3u8(&render_m3u8(&p), "gym").unwrap();
    assert_eq!(parsed.name, "Gym");
    assert_eq!(parsed.tracks, p.tracks);
}

#[test]
fn m3u8_parse_tolerates_bom_crlf_extinf_and_backslashes() {
    let text = "\u{feff}#EXTM3U\r\n#PLAYLIST:Mix\r\n#EXTINF:123,Artist - Title\r\nA\\B\\01.flac\r\n\r\n";
    let p = parse_m3u8(text, "mix").unwrap();
    assert_eq!(p.name, "Mix");
    assert_eq!(p.tracks, vec![PathBuf::from("A/B/01.flac")]);
}

#[test]
fn store_saves_lists_loads_deletes_and_uniquifies() { /* open store in tempdir; save manual "Gym"
    + smart stub named "Gym" via unique_slug -> "gym", "gym-2"; list() returns both; delete("gym")
    true then load none; corrupt file on disk -> list() skips it and last_errors() records it */ }

#[test]
fn resolve_manual_skips_missing_and_counts() {
    let p = ManualPlaylist { slug: "x".into(), name: "X".into(),
        tracks: vec!["a/1.flac".into(), "gone/2.flac".into()] };
    let (found, missing) = resolve_manual(&p, Path::new("/src"), &|p| !p.starts_with("/src/gone"));
    assert_eq!(found, vec![PathBuf::from("/src/a/1.flac")]);
    assert_eq!(missing, 1);
}
```

- [ ] **Step 2:** `cargo test -p classick playlist` → FAIL (unresolved). **Step 3:** implement. **Step 4:** focused then full `cargo test` → PASS.
- [ ] **Step 5: Commit** — `git add crates/classick/src/playlist.rs crates/classick/src/playlist_rules.rs crates/classick/src/lib.rs && git commit -m "feat(playlist): playlist module — M3U8 round-trip, slug store, resolution"`

### Task 2: `playlist_rules` — smart evaluation

**Files:**
- Modify: `crates/classick/src/playlist_rules.rs` (replace Task 1's stub), `crates/classick/src/lib.rs` (module decl if not already)

**Interfaces:**
- Consumes: `crate::library_index::{LibraryIndex, IndexedTrack}` (each entry has `facts()` → `TrackFacts{artist, album_artist, album, genre}`, plus size/mtime/year fields — read `library_index.rs:19-46` for exact names before writing code).
- Produces:
  - `pub struct SmartRules { pub version: u32, pub matching: Match, pub rules: Vec<Rule>, pub limit: Option<Limit>, pub order: Order, pub seed: u64 }` with `pub enum Match { All, Any }`, `pub struct Rule { pub field: Field, pub op: Op, pub value: String }`, `pub enum Field { Artist, Album, Genre, Year }`, `pub enum Op { Is, Contains, Gte, Lte }`, `pub enum Limit { Bytes(u64), Tracks(usize) }`, `pub enum Order { RecentlyModified, RandomStable, Alpha }` — all serde snake_case, `#[serde(default)]` where a legacy file could omit (`order` defaults Alpha, `seed` defaults 0, `limit` None).
  - `pub fn evaluate(rules: &SmartRules, index: &LibraryIndex) -> Vec<PathBuf>` — filter by rules (case-insensitive; Year parses the value, non-numeric year rule matches nothing), order (RecentlyModified = mtime desc; RandomStable = hash(seed, path) sort — deterministic across runs; Alpha = path), then apply limit (Bytes uses the index entry's size, first-fit in order — track granularity, NOT album-atomic: the fit engine handles device atomicity later; a one-line doc comment says so).

- [ ] **Step 1: Failing tests** — build a synthetic `LibraryIndex` in-memory (use `LibraryIndex::empty` + push entries; read the struct to see how entries are stored):

```rust
#[test] fn all_vs_any_matching() { /* two rules genre=Ambient, artist contains "eno";
    All -> only tracks matching both; Any -> union */ }
#[test] fn year_gte_and_non_numeric_rule_matches_nothing() { /* year gte 2000 filters; op value "abc" -> empty */ }
#[test] fn random_stable_is_deterministic_and_seed_sensitive() { /* same seed twice -> same order; different seed -> different order (with 10 tracks, assert orders differ) */ }
#[test] fn byte_limit_takes_prefix_in_order() { /* alpha order, limit Bytes fits first two of three */ }
#[test] fn rules_json_round_trip_with_defaults() { /* serde: minimal JSON w/o order/seed/limit decodes; encode->decode stable */ }
```

- [ ] **Step 2–4: RED → implement → full `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(playlist): smart-rule evaluation over the library index"` (stage the two files by name).

## Stage B — Per-device config v2

### Task 3: `device_state` v2 files + migrations

**Files:**
- Modify: `crates/classick/src/device_state.rs` (paths), Create: `crates/classick/src/device_config.rs`, Modify: `crates/classick/src/lib.rs`

**Interfaces:**
- Produces in `device_state`: `pub fn device_subscriptions_path(serial: &str) -> Result<PathBuf>`, `pub fn device_settings_path(serial: &str) -> Result<PathBuf>` (+ `_in` variants, same pattern as existing fns).
- Produces in `device_config`:
  - `pub struct Subscriptions { pub version: u32, pub playlists: Vec<String> }` (slugs; `load_or_default`/`save_atomic` mirroring `selection.rs` patterns).
  - `pub struct DeviceSettings { pub version: u32, pub auto_sync: bool, pub rockbox_compat: bool }` with `load_or_migrate(serial, global: &config_file::PersistedConfig) -> DeviceSettings` — if the device file exists, read it; else seed from the global `DaemonSettings` (`enabled` → auto_sync, `rockbox_compat` → rockbox_compat), SAVE the seeded file, return it. One-shot by construction.
  - Selection migration already exists (`selection::seed_custom_selection`); add `pub fn effective_device_selection_path(serial: &str) -> Result<PathBuf>` in `selection.rs` that ALWAYS returns the per-device path, seeding from the shared root `selection.json` if the device file is absent (subsumes the custom_selection branch; `custom_selection` field no longer consulted — leave the field + serde default in place, mark `#[deprecated]` in a doc comment only, not the attribute, to avoid warning churn).

- [ ] **Step 1: Failing tests** — settings migration seeds-once-and-persists (second call with a DIFFERENT global returns the persisted first values); subscriptions round-trip + absent→default; effective_device_selection_path seeds from shared exactly once then ignores shared changes; paths nest under `devices/<serial>/`.
- [ ] **Step 2–4: RED → implement → PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): per-device config v2 — subscriptions + settings with one-shot migrations, per-device selection always"`

### Task 4: Daemon consumes per-device settings

**Files:**
- Modify: `crates/classick/src/daemon/runtime.rs` (plug-in auto-sync gate + wherever `rockbox_compat` is read to build the subprocess command), `crates/classick/src/daemon/sync_orchestrator.rs` (if the flag pass-through lives there — follow the code), `crates/classick/src/apply_loop.rs` (the persisted-config read that picks rockbox_compat for the sync — switch to the device settings file; anchor on the Task-3 trust-package pattern of re-reading persisted config)

**Interfaces:**
- Consumes: `device_config::DeviceSettings::load_or_migrate`. The daemon's `auto_sync_enabled()`-style gate becomes per-device: on device-connected auto-sync, load the CONNECTED device's settings by serial. Extract pure `pub(crate) fn should_auto_sync(settings: &DeviceSettings) -> bool` for the test.
- The sync subprocess resolves rockbox_compat the same way (device settings for the resolved serial; CLI `--rockbox-compat` still force-overrides for one-shot runs).

- [ ] **Step 1: Failing tests** — `should_auto_sync` trivial truth; apply-loop-side: extract and test `pub(crate) fn effective_rockbox(cli_flag: bool, device: &DeviceSettings) -> bool` (cli true → true; else device value). Migration behavior already covered by Task 3.
- [ ] **Step 2–4: RED → implement → full `cargo test` PASS** (Windows-gated integration file may need mechanical DaemonDeps edits — textual consistency check in the report, same as prior tasks).
- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): auto-sync and rockbox gates read per-device settings"`

## Stage C — Planner union + device playlists

### Task 5: Union planner

**Files:**
- Create: `crates/classick/src/sync_set.rs`, Modify: `crates/classick/src/apply_loop.rs` (call site where `selection::apply_to_sources` currently runs), `crates/classick/src/lib.rs`

**Interfaces:**
- Produces: `pub struct EffectiveSet { pub sources: Vec<SourceEntry>, pub playlist_tracks: Vec<(String, Vec<PathBuf>)>, pub missing_playlist_tracks: usize, pub playlist_errors: Vec<(String, String)> }` and `pub fn compute(walk: Vec<SourceEntry>, selection: &Selection, subs: &Subscriptions, store: &PlaylistStore, index: &LibraryIndex, source_root: &Path) -> EffectiveSet` — scope-filter the walk (existing `selection::filter` semantics), resolve each subscribed playlist (manual via `resolve_manual` against a set built from the WALK — the walk is the existence oracle, no extra fs; smart via `playlist_rules::evaluate`), union by absolute path (playlist tracks not in scope re-attach their `SourceEntry` from the walk map — a playlist can only add tracks that exist in the walk, which also enforces read-only-source), preserve walk order with playlist-only additions appended in playlist order.
- `playlist_tracks` keeps per-playlist resolved ABSOLUTE paths for Task 6's reconcile. Unknown subscription slug → entry in `playlist_errors`, sync proceeds.
- Apply-loop change is ~6 lines: build store/subs/index (all already loaded or cheap), call `compute`, use `.sources` where the filtered walk was used, stash the rest for Task 6 + summary logging.

- [ ] **Step 1: Failing tests** (pure, synthetic walks):

```rust
#[test] fn subscribed_tracks_outside_include_scope_still_sync() { /* Include scope selects artist A;
    playlist references artist B's track; EffectiveSet.sources contains both */ }
#[test] fn playlists_only_device_empty_include_plus_subscription() { /* Include mode, zero rules,
    one playlist -> sources == playlist tracks exactly */ }
#[test] fn union_dedups_and_keeps_walk_order_then_appends() { }
#[test] fn playlist_track_absent_from_walk_counts_missing_never_invents_source() { }
#[test] fn unknown_slug_is_error_not_failure() { }
```

- [ ] **Step 2–4: RED → implement → full `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(apply-loop): sync set = scope union subscribed playlists (sync_set module)"`

### Task 6: iTunesDB playlist reconcile (FFI) + mirror + adopt

**Files:**
- Modify: `crates/classick/src/ipod/db.rs` (FFI wrappers), Create: `crates/classick/src/ipod/device_playlists.rs` (reconcile orchestration; register in `ipod/mod.rs`), Modify: `crates/classick/src/apply_loop.rs` (call after the track loop, before final `db.write()`; mirror write + adopt at session start), Test: `crates/classick/tests/device_playlists_integration.rs`

**Interfaces:**
- db.rs produces (unsafe inside, safe API out): `pub fn list_playlists(db: &OwnedDb) -> Vec<(String, bool /*is_mpl*/)>`, `pub fn ensure_playlist(db: &OwnedDb, name: &str, dbids: &[u64]) -> Result<()>` (find by name via `itdb_playlist_by_name`; create with `itdb_playlist_new(name, false)` + `itdb_playlist_add` if absent; then clear its member list and re-add tracks by dbid — look up each track via the DB track list dbid map built once; missing dbid skipped with warn), `pub fn remove_playlist_by_name(db: &OwnedDb, name: &str) -> Result<bool>` (never the MPL — `itdb_playlist_is_mpl` guard; `itdb_playlist_remove` frees).
- device_playlists produces: `pub fn reconcile(db: &OwnedDb, desired: &[(String, Vec<u64>)], managed_marker: &str) -> Result<ReconcileStats>` — Classick-managed playlists are identified by a name PREFIX-free strategy: we manage exactly the set recorded in the device dir's `managed_playlists.json` (written by this fn each run) — NOT by name heuristics, so foreign playlists with coincidental names are safe. Playlists in the managed record but not in `desired` → removed; in `desired` → ensured. `ReconcileStats { created, updated, removed }`.
- Mapping source path → dbid comes from the freshly-saved manifest (entries carry `ipod_dbid`); apply_loop builds `Vec<(name, Vec<u64>)>` from Task 5's `playlist_tracks` joined against the manifest.
- Mirror: after successful `db.write()`, copy every playlist file + `subscriptions.json` to `iPod_Control/classick/playlists/` (create dirs; warn-only on failure). Adopt: at session start, if the host store is EMPTY and the device mirror is non-empty, copy device→host once, `tracing::warn!("adopted N playlists from device mirror")`.

- [ ] **Step 1: Failing integration test** (fake mount + hand-rolled DB per `fit_retry_integration.rs`, `Music/F00` pre-created): add 3 tracks; reconcile with desired=[("Gym",[dbid1,dbid2])] → reparse: playlist exists with 2 members, MPL untouched; re-reconcile with desired=[] → playlist gone, MPL + a manually-created "Foreign" playlist (created via ensure_playlist then removed from managed record by writing the record directly) untouched. Unit tests for the managed-record round-trip.
- [ ] **Step 2–4: RED → implement → full `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(ipod): Classick-managed iTunesDB playlists — reconcile, mirror, adopt"`

## Stage D — Wire 1.6.0

### Task 7: Wire types + daemon arms + docs

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs` (`DAEMON_PROTOCOL_VERSION` → "1.6.0"; commands `ListPlaylists`, `GetPlaylist{slug}`, `SavePlaylist{playlist: PlaylistPayload}`, `DeletePlaylist{slug}`, `GetDeviceConfig{serial}`, `SaveDeviceConfig{serial, selection?, subscriptions?, settings?}`, `PreviewDevice{serial}`; events `PlaylistsUpdate{playlists: Vec<PlaylistSummary>}`, `DeviceConfigUpdate{serial, selection, subscriptions, settings}`, `DevicePreview{selected_tracks, selected_bytes, playlist_extra_tracks, playlist_extra_bytes, projected_free_bytes: Option<u64>}`; `PlaylistPayload` = tagged manual{slug?,name,tracks}/smart{slug?,name,rules} — absent slug = create via `unique_slug`; `PlaylistSummary{slug,name,kind,tracks,bytes,error?}`), `crates/classick/src/daemon/runtime.rs` (arms; preview reuses `daemon/library.rs` sizing + `sync_set::compute` against the cached index), `crates/classick/src/daemon/library.rs` (helper for summary sizing), `docs/ipc-protocol.md` (v1.6.0 section — SAME commit)
- Test: serde round-trips in ipc_daemon.rs tests; pure preview-math helper test in library.rs.

**Interfaces:** consumes everything above (`PlaylistStore`, `device_config`, `sync_set::compute`). Version test renamed to `protocol_version_is_1_6_0`. Deprecation notes in the doc for `get_selection`/`save_selection`/`custom_selection` (kept, operating on the configured device's per-device selection).

- [ ] **Step 1: Failing tests** — every new command deserializes from its snake_case JSON; events serialize with exact field names; version constant. **Step 2–4: RED → implement → full `cargo test` PASS.**
- [ ] **Step 5: Commit** — `git commit -m "feat(daemon): protocol 1.6.0 — playlist CRUD, device config, device preview"`

### Task 8: End-to-end core smoke + LEARNINGS

**Files:**
- Create: `crates/classick/tests/playlists_e2e.rs` — fake mount + tagged.flac source tree: save a manual playlist referencing a track OUTSIDE an Include scope, subscribe, run the plan-relevant seam (`sync_set::compute` + full apply via the fit-retry harness pattern), assert the outside-scope track lands on the device AND in a reparsed DB playlist; unsubscribe → next run removes the playlist but NOT the track if it's now in scope, or removes both if not (assert the diff does it — this pins union↔diff interplay).
- Modify: `LEARNINGS.md` — one entry: playlists are reconciled from `managed_playlists.json` records, never name heuristics; MPL guard; mirror/adopt semantics.

- [ ] **Steps: test RED where it can be (new file) → wire any gaps found → full `cargo test` PASS → commit** `git commit -m "test(playlist): end-to-end union + device-playlist round-trip; record learnings"`

---

## Self-review notes (for executors)

- Spec §1 → Tasks 1, 2, 6 (mirror/adopt); §2 → Task 3; §3 → Tasks 5, 6; §4 → Task 7; §6 error rules → asserted across Tasks 1 (last_errors), 5 (playlist_errors), 6 (warn-only mirror); §7 core rows → Tasks 1–8.
- `SmartRules` stub in Task 1 exists ONLY so Task 1 compiles alone; Task 2 replaces it — do not ship the stub past Task 2.
- Union is computed from the WALK as existence oracle — no playlist can pull a file the walk didn't see; this is the read-only-source guarantee, don't "optimize" it away.
- Reconcile identifies managed playlists by the persisted record, never by name matching — that's the foreign-playlist safety property; the integration test pins it.
