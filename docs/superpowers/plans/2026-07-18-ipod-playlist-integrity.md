# iPod Playlist Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Audit every iTunesDB playlist without writing, preserve foreign playlists during track deletion, normalize only exact registered firmware-system duplicates, and publish Classick playlist ownership as recoverable device-authoritative state.

**Architecture:** Focused Rust modules turn libgpod playlist pointers into owned audit snapshots, classify snapshots structurally, and stage managed-playlist mutations without publishing ownership. Plan 3's `CheckpointCoordinator` writes and reparses the DB, verifies every candidate ID and ordered membership, then atomically publishes `/iPod_Control/classick/managed_playlists.json`; its pending journal is the recovery authority until finalization completes. One `OwnedDb` helper snapshots every playlist containing a doomed track and unlinks it before `itdb_track_remove`, so ordinary delete, replacement, wipe, and recovery share the same safe path.

**Tech Stack:** Rust stable, serde/serde_json, clap, vendored libgpod FFI, Plan 2 `AtomicFileWriter`, Plan 3 `PendingSession` and `CheckpointCoordinator`, Cargo integration tests with real write/reparse fake iPods.

## Global Constraints

- This plan depends on completed Plans 1–3 and must reuse Plan 1's exact raw device serial, Plan 2's atomic-file primitive, and Plan 3's coordinated DB/artwork/manifest checkpoint; it must not add an independent DB publication path.
- Keep `\\jupiter\data\media\music` read-only. The audit command performs no DB, manifest, ownership, or device write.
- Classick never creates a firmware-system Videos playlist. It may remove only older exact instances of either registered encoding of the `ipod-classic-video-kind-v1` semantic profile.
- Never classify, adopt, update, or remove a playlist by display name or emptiness. Unknown profiles, near matches, and unrecorded playlists are foreign and remain untouched.
- The connected device record at `/iPod_Control/classick/managed_playlists.json` is authoritative. A host per-device record is a cache and cannot grant permission to mutate an Apple playlist.
- A managed ID grants ownership only when it resolves to a normal, non-master, non-podcast, non-smart playlist. An invalid association is dropped from the candidate record while the suspect playlist is preserved.
- Before freeing any track, snapshot and unlink it from every containing normal or smart playlist; `itdb_playlist_remove_track(NULL, track)` is forbidden.
- Playlist ownership is required finalization work. A device-ownership publication failure leaves the pending journal intact and cannot be downgraded to a warning or followed by completed finalization.
- Recovery after a verified DB write verifies and publishes the journal's existing candidate IDs; it never reruns reconcile or creates replacement IDs.
- Keep every created or materially split implementation file at or below roughly 500 lines. Split fixture builders and integration harnesses instead of growing `ipod/db.rs`, `apply_loop.rs`, or `sync_transaction.rs` further.
- Add regression coverage first, run the focused test to observe the stated RED failure, implement the smallest passing change, and run Rust test processes sequentially when they touch libgpod or shared device-shaped paths.
- Before every commit, stage only the exact named files with `git add <paths>`, inspect `git diff --cached`, and use the listed Conventional Commit message. Never use `git add .`, `git add -A`, `--no-verify`, or amend.
- The physical causality gate remains release-blocking. If firmware behavior disproves the two registered exact encodings or reveals another legitimate category distinction, correct the profile and automated fixtures before release; do not broaden deletion.

---

## File and Interface Map

| File | Responsibility |
|---|---|
| `crates/classick/src/ipod/playlist_audit.rs` | Convert libgpod playlists/rules into pointer-free serde snapshots; expose the read-only inventory. |
| `crates/classick/src/ipod/playlist_profile.rs` | Structural classification and exact fixture-backed firmware profile matching. |
| `crates/classick/src/ipod/playlist_normalize.rs` | Deterministically remove only older exact firmware-profile duplicates from an in-memory DB. |
| `crates/classick/src/ipod/playlist_ownership.rs` | Versioned device-authoritative ownership DTO, strict validation, atomic device publication, best-effort host-cache refresh. |
| `crates/classick/src/ipod/device_playlists.rs` | Produce desired managed mutations and a candidate ownership record; never save ownership directly. |
| `crates/classick/src/ipod/db.rs` | Provide guarded playlist lookup and one safe all-playlist unlink primitive used by every track-removal path. |
| `crates/classick/src/playlist_audit_command.rs` | Run `--audit-playlists`, serialize deterministic JSON, and return without mutation. |
| `crates/classick/src/pending_session.rs` | Carry candidate ownership and verified membership through Plan 3 recovery. |
| `crates/classick/src/sync_transaction.rs` | Normalize before each DB write, verify candidate IDs/membership after reparse, publish device ownership, and retain the journal on failure. |
| `crates/classick/tests/fixtures/ipod-classic-video-kind-v1.json` | Exact registered firmware-system semantic profile. |
| `crates/classick/tests/fixtures/ipod-classic-video-kind-v1-libgpod-post-write.json` | Exact deterministic encoding produced when libgpod validates and writes the captured profile. |
| `crates/classick/tests/playlist_audit_integration.rs` | Read-only command and audit serialization proof. |
| `crates/classick/tests/playlist_track_unlink_integration.rs` | Normal/smart/foreign membership deletion and write/reparse proof. |
| `crates/classick/tests/playlist_ownership_integration.rs` | Managed-target guards, authority migration, publication ordering, and recovery failure injection. |
| `crates/classick/tests/playlist_normalization_integration.rs` | Zero/one/many exact profile normalization and preservation matrix. |

The stable cross-plan data contract is:

```rust
pub const MANAGED_PLAYLIST_OWNERSHIP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaylistKind {
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RockboxProjectionRecord {
    pub relative_filename: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedPlaylistEntry {
    pub apple_playlist_id: u64,
    pub expected_kind: ManagedPlaylistKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rockbox: Option<RockboxProjectionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedPlaylistOwnership {
    pub schema_version: u32,
    pub device_serial: String,
    pub playlists: BTreeMap<String, ManagedPlaylistEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredPlaylist {
    pub slug: String,
    pub display_name: String,
    pub ordered_dbids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedPlaylistMembership {
    pub slug: String,
    pub apple_playlist_id: u64,
    pub ordered_dbids: Vec<u64>,
    pub ordered_ipod_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistReconcileOutcome {
    pub candidate_ownership: ManagedPlaylistOwnership,
    pub desired_memberships: BTreeMap<String, Vec<u64>>,
    pub stats: ReconcileStats,
    pub diagnostics: Vec<PlaylistDiagnostic>,
}
```

Plan 6B consumes `ManagedPlaylistOwnership`, `RockboxProjectionRecord`, and `VerifiedPlaylistMembership`. It adds or removes only the `rockbox` projection field and must not change the Apple ID or ordered membership verified by this plan.

Plan 6B also extends `PendingSession` with this journal-only operation map:

```rust
pub struct PendingRockboxOp {
    pub previous: Option<RockboxProjectionRecord>,
    pub desired: Option<RockboxProjectionRecord>,
}

pub pending_rockbox_ops: BTreeMap<String, PendingRockboxOp>;

pub fn prepare_rockbox_projections(
    settled: &ManagedPlaylistOwnership,
    candidate: &mut ManagedPlaylistOwnership,
    verified: &[VerifiedPlaylistMembership],
    enabled: bool,
) -> Result<BTreeMap<String, PendingRockboxOp>>;
```

The settled `ManagedPlaylistEntry.rockbox` remains the single desired projection record. During an incomplete Rockbox rename or delete, only the device ownership record plus the journal's `previous`/`desired` pair authorizes both paths. Plan 6A must preserve unknown/defaulted journal extension fields during recovery and must not infer an old Rockbox path by scanning or by treating settled ownership alone as a deletion history.

`PendingPhase::RockboxProjectionsPrepared` is the durable planning boundary shared with Plan 6B. It comes after verified membership and `DeviceManifestPublished`, but before `PlaylistOwnershipPublished`: the enriched candidate ownership and the complete operation map must be journaled before either becomes device truth. A valid empty operation map at `RockboxProjectionsPrepared` means planning completed with zero filesystem operations; `DeviceManifestPublished` means planning has not completed and must be resumed. The empty fast path is legal in a pre-6B build only when both settled device ownership and candidate ownership contain no `rockbox` record. It is never a toggle-off implementation for a previously projected device.

---

### Task 1: Pointer-free playlist inventory and exact firmware profile

**Files:**
- Create: `crates/classick/src/ipod/playlist_audit.rs`
- Create: `crates/classick/src/ipod/playlist_profile.rs`
- Create: `crates/classick/tests/fixtures/ipod-classic-video-kind-v1.json`
- Modify: `crates/classick/src/ipod/mod.rs`

**Interfaces:**
- Consumes: `OwnedDb::as_ptr() -> *mut ffi::Itdb_iTunesDB`, libgpod `Itdb_Playlist`, `Itdb_SPLPref`, `Itdb_SPLRules`, and `Itdb_SPLRule` fields.
- Produces: `audit_playlists(db: &OwnedDb, managed: &ManagedPlaylistOwnership) -> PlaylistAudit`; `classify_playlist(playlist: &PlaylistSnapshot, managed: &ManagedPlaylistOwnership) -> PlaylistClassification`; `firmware_profile(id: FirmwareProfileId) -> &'static FirmwarePlaylistProfile`.

- [ ] **Step 1: Write the exact versioned firmware profile fixture**

Create `crates/classick/tests/fixtures/ipod-classic-video-kind-v1.json` with the complete captured pre-write semantic payload below, plus `crates/classick/tests/fixtures/ipod-classic-video-kind-v1-libgpod-post-write.json` with the exact deterministic result of libgpod rule validation (`tovalue = fromvalue` and `tounits = fromunits` for both rules). Together these two payloads are the closed encoding set for one profile ID. `name`, `id`, and `timestamp` are deliberately absent because they are diagnostic fields, not matching fields:

```json
{
  "profile_id": "ipod-classic-video-kind-v1",
  "is_master": false,
  "is_podcast": false,
  "is_smart": true,
  "member_count": 0,
  "preferences": {
    "liveupdate": 1,
    "checkrules": 1,
    "checklimits": 0,
    "limittype": 3,
    "limitsort": 2,
    "limitvalue": 25,
    "matchcheckedonly": 0,
    "reserved_int1": 0,
    "reserved_int2": 0,
    "reserved1_is_null": true,
    "reserved2_is_null": true
  },
  "rules_header": {
    "unk004": 65537,
    "match_operator": 0,
    "reserved_int1": 0,
    "reserved_int2": 0,
    "reserved1_is_null": true,
    "reserved2_is_null": true
  },
  "rules": [
    {
      "field": 60,
      "action": 1024,
      "string": null,
      "fromvalue": 3138,
      "fromdate": 0,
      "fromunits": 1,
      "tovalue": 0,
      "todate": 0,
      "tounits": 0,
      "unk052": 0,
      "unk056": 0,
      "unk060": 0,
      "unk064": 0,
      "unk068": 0,
      "reserved_int1": 0,
      "reserved_int2": 0,
      "reserved1_is_null": true,
      "reserved2_is_null": true
    },
    {
      "field": 60,
      "action": 33555456,
      "string": null,
      "fromvalue": 2138116,
      "fromdate": 0,
      "fromunits": 1,
      "tovalue": 0,
      "todate": 0,
      "tounits": 0,
      "unk052": 0,
      "unk056": 0,
      "unk060": 0,
      "unk064": 0,
      "unk068": 0,
      "reserved_int1": 0,
      "reserved_int2": 0,
      "reserved1_is_null": true,
      "reserved2_is_null": true
    }
  ]
}
```

- [ ] **Step 2: Write failing unit tests for snapshot serialization and classification**

Add tests inside `playlist_profile.rs` that construct snapshots without FFI:

```rust
#[test]
fn exact_profile_matches_independent_of_name_id_and_timestamp() {
    let mut first = fixture_snapshot("Videos", 7, 100);
    let mut localized = fixture_snapshot("Videos locales", 99, 200);
    first.name = Some("Videos".into());
    localized.name = Some("Vidéos".into());
    assert_eq!(match_firmware_profile(&first), Some(FirmwareProfileId::IpodClassicVideoKindV1));
    assert_eq!(match_firmware_profile(&localized), Some(FirmwareProfileId::IpodClassicVideoKindV1));
}

#[test]
fn every_near_match_is_foreign() {
    for mutate in near_match_mutations() {
        let mut snapshot = fixture_snapshot("Videos", 7, 100);
        mutate(&mut snapshot);
        assert_eq!(match_firmware_profile(&snapshot), None);
    }
}

#[test]
fn managed_requires_exact_id_and_normal_structure() {
    let ownership = ownership("SERIAL", "mix", 42);
    let normal = normal_snapshot("Mix", 42);
    let mut smart = normal.clone();
    smart.is_smart = true;
    assert!(matches!(classify_playlist(&normal, &ownership), PlaylistClassification::Managed { slug } if slug == "mix"));
    assert!(matches!(classify_playlist(&smart, &ownership), PlaylistClassification::Foreign { reason: ForeignReason::InvalidManagedTarget }));
}
```

`near_match_mutations()` must return one mutation for each semantic field: member count, master/podcast/smart flags, all seven preferences, every preference reserved field/pointer-null marker, both rules-header fields and reserved values, rule count/order, and every rule scalar/string/reserved value.

- [ ] **Step 3: Run the profile tests and confirm RED**

Run: `cargo test -p classick ipod::playlist_profile -- --test-threads=1`

Expected: FAIL to compile with unresolved module `playlist_profile` or missing `match_firmware_profile`.

- [ ] **Step 4: Implement owned audit DTOs and exact classification**

Implement these public types in files that remain below 500 lines:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistSnapshot {
    pub id: u64,
    pub name: Option<String>,
    pub timestamp: i64,
    pub member_count: usize,
    pub sort_order: u32,
    pub is_master: bool,
    pub is_podcast: bool,
    pub is_smart: bool,
    pub preferences: SmartPreferencesSnapshot,
    pub rules_header: SmartRulesHeaderSnapshot,
    pub rules: Vec<SmartRuleSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaylistClassification {
    Managed { slug: String },
    FirmwareSystem { profile: FirmwareProfileId },
    Foreign { reason: ForeignReason },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InternalCategoryVisibility {
    UnsupportedByVendoredLibgpod,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistAudit {
    pub playlists: Vec<ClassifiedPlaylistSnapshot>,
    pub internal_mhsd5_categories: InternalCategoryVisibility,
}
```

`snapshot_playlists` must walk `(*db.as_ptr()).playlists` once, copy every C string before returning, walk each `members` and `splrules.rules` `GList` without mutation, and derive master/podcast through `itdb_playlist_is_mpl` and `itdb_playlist_is_podcasts`. Return playlists in DB order. `classify_playlist` checks a valid managed ID first, then the exact firmware profile, otherwise returns `Foreign` with a reason that states whether the ID was unrecorded, structurally invalid, or an unknown system signature.

- [ ] **Step 5: Run profile and module tests GREEN**

Run: `cargo test -p classick ipod::playlist_profile -- --test-threads=1 && cargo test -p classick ipod::playlist_audit -- --test-threads=1`

Expected: PASS; both exact registered encodings match independent of localized name, ID, and timestamp. Every one-field mutation of either encoding is foreign unless it produces the other registered encoding exactly.

- [ ] **Step 6: Commit the inventory/profile unit**

```bash
git add crates/classick/src/ipod/mod.rs crates/classick/src/ipod/playlist_audit.rs crates/classick/src/ipod/playlist_profile.rs crates/classick/tests/fixtures/ipod-classic-video-kind-v1.json
git diff --cached
git commit -m "feat(ipod): classify playlist structures exactly"
```

---

### Task 2: Read-only playlist audit command

**Files:**
- Create: `crates/classick/src/playlist_audit_command.rs`
- Create: `crates/classick/tests/playlist_audit_integration.rs`
- Modify: `crates/classick/src/lib.rs`
- Modify: `crates/classick/src/cli.rs`
- Modify: `crates/classick/src/orchestrator.rs`

**Interfaces:**
- Consumes: `audit_playlists(&OwnedDb, &ManagedPlaylistOwnership) -> PlaylistAudit`; `DeviceOwnershipStore::load_device_read_only()`, introduced in Task 4 but represented here by an empty record if the device file is absent.
- Produces: CLI `--audit-playlists`; `run(ipod: Option<&str>, progress: &Progress) -> Result<PlaylistAudit>`; deterministic pretty JSON on stdout in plain mode and a single `progress.log` payload in TUI/IPC mode.

- [ ] **Step 1: Write the failing CLI and mutation-sentinel tests**

Create an integration fixture with a real libgpod DB containing a master playlist, foreign normal playlist, arbitrary empty smart playlist, and exact profile. Snapshot `iTunesDB` bytes plus every file path/size/mtime before and after:

```rust
#[test]
fn audit_json_is_deterministic_and_device_is_byte_unchanged() {
    let fixture = AuditFixture::new();
    let before = fixture.tree_digest();
    let first = playlist_audit_command::run_at(&fixture.mount, &fixture.serial).unwrap();
    let second = playlist_audit_command::run_at(&fixture.mount, &fixture.serial).unwrap();
    assert_eq!(serde_json::to_string_pretty(&first).unwrap(), serde_json::to_string_pretty(&second).unwrap());
    assert_eq!(fixture.tree_digest(), before);
    assert_eq!(first.playlists.len(), 4);
    assert_eq!(first.internal_mhsd5_categories, InternalCategoryVisibility::UnsupportedByVendoredLibgpod);
}

#[test]
fn audit_flag_conflicts_with_every_mutating_one_shot() {
    for other in ["--apply", "--backfill-rockbox", "--restore-db-backup", "--replace-library"] {
        assert!(Cli::try_parse_from(["classick", "--audit-playlists", other]).is_err(), "accepted {other}");
    }
}
```

- [ ] **Step 2: Run the command tests and confirm RED**

Run: `cargo test -p classick --test playlist_audit_integration -- --test-threads=1`

Expected: FAIL to compile because `playlist_audit_command` and `Cli::audit_playlists` do not exist.

- [ ] **Step 3: Add the one-shot flag and read-only dispatch**

Add this CLI-only field; do not thread it through persisted or resolved `Config` because the audit must work without a configured source library:

```rust
/// Emit a complete structural iTunesDB playlist inventory as JSON, then exit.
/// Opens the DB and ownership record read-only and performs no device write.
#[arg(long, conflicts_with_all = [
    "apply", "dry_run", "rebuild_manifest", "backfill_rockbox", "scan_library",
    "restore_db_backup", "replace_library", "verify_artwork",
])]
pub audit_playlists: bool,
```

Dispatch it in `orchestrator::orchestrate` immediately after the config-parse/reset loop and before `ensure_source_or_wizard` or `config::resolve`, so a machine with no source configuration can still inspect an iPod. Resolve only the explicit `--ipod` mount or `ipod::detect_ipod_mount()`, read the raw serial with `read_firewire_guid`, open `OwnedDb`, read the device ownership file without creating its parent, audit, emit pretty JSON, and return `RunOutcome::Completed`. Do not call `set_firewire_guid`, `backup_itunesdb`, reconciliation, `db.write`, `ManifestStore::publish`, or `AtomicFileWriter`.

- [ ] **Step 4: Run command, CLI, and config tests GREEN**

Run: `cargo test -p classick --test playlist_audit_integration -- --test-threads=1 && cargo test -p classick cli::tests -- --test-threads=1`

Expected: PASS; the tree digest is identical and audit succeeds without a source path or persisted configuration.

- [ ] **Step 5: Commit the read-only diagnostic**

```bash
git add crates/classick/src/playlist_audit_command.rs crates/classick/tests/playlist_audit_integration.rs crates/classick/src/lib.rs crates/classick/src/cli.rs crates/classick/src/orchestrator.rs
git diff --cached
git commit -m "feat(ipod): add read-only playlist audit"
```

---

### Task 3: Safe track unlink from every containing playlist

**Files:**
- Create: `crates/classick/tests/playlist_track_unlink_integration.rs`
- Modify: `crates/classick/src/ipod/db.rs`
- Modify: `crates/classick/src/apply_loop.rs`
- Modify: `crates/classick/tests/wipe_all_tracks_integration.rs`

**Interfaces:**
- Consumes: `itdb_playlist_contains_track`, `itdb_playlist_remove_track`, `itdb_track_remove`, and Plan 3's `OwnedDb::unlink_track_keep_file(dbid)`.
- Produces: `OwnedDb::unlink_track_from_all_playlists(track: *mut ffi::Itdb_Track) -> Result<usize>` as a private unsafe-boundary helper; `OwnedDb::remove_track(dbid: u64, file: TrackFileDisposition) -> Result<bool>`; `TrackFileDisposition::{DeleteAfterCommit, Keep}`; `wipe_all_tracks` delegates to `remove_track`.

- [ ] **Step 1: Write the failing normal/smart membership regression**

Build a real DB with one doomed track and one retained track. Add the doomed track to master, a foreign normal playlist, a foreign smart playlist, and a Classick-managed normal playlist:

```rust
#[test]
fn delete_unlinks_every_playlist_before_free_and_reparses_cleanly() {
    let fixture = UnlinkFixture::with_shared_track();
    assert_eq!(fixture.memberships_for(fixture.doomed_dbid), vec!["iPod", "Foreign", "Foreign Smart", "Managed"]);
    assert!(fixture.db.remove_track(fixture.doomed_dbid, TrackFileDisposition::DeleteAfterCommit).unwrap());
    fixture.db.write().unwrap();
    let reopened = OwnedDb::open(&fixture.mount).unwrap();
    assert!(reopened.find_track(fixture.doomed_dbid).is_none());
    assert_eq!(fixture.playlist_dbids(&reopened, "Foreign"), vec![fixture.retained_dbid]);
    assert_eq!(fixture.playlist_dbids(&reopened, "Foreign Smart"), vec![fixture.retained_dbid]);
    assert_eq!(fixture.playlist_dbids(&reopened, "Managed"), vec![fixture.retained_dbid]);
}
```

Add a second test proving `TrackFileDisposition::Keep` leaves audio on disk for Plan 3's deferred cleanup while still removing every membership.

- [ ] **Step 2: Run the unlink integration and confirm RED**

Run: `cargo test -p classick --test playlist_track_unlink_integration -- --test-threads=1`

Expected: FAIL to compile because `TrackFileDisposition` and `OwnedDb::remove_track` do not exist; the pre-fix test using `delete_track` must expose a dangling foreign/smart member after write/reparse on the vendored libgpod.

- [ ] **Step 3: Implement one guarded removal primitive**

Use this exact algorithm; never pass a null playlist:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackFileDisposition {
    DeleteAfterCommit,
    Keep,
}

unsafe fn unlink_track_from_all_playlists(&self, track: *mut ffi::Itdb_Track) -> usize {
    let mut containing = Vec::new();
    let mut node = (*self.0).playlists;
    while !node.is_null() {
        let playlist = (*node).data as *mut ffi::Itdb_Playlist;
        if !playlist.is_null() && ffi::itdb_playlist_contains_track(playlist, track) != 0 {
            containing.push(playlist);
        }
        node = (*node).next;
    }
    for playlist in &containing {
        ffi::itdb_playlist_remove_track(*playlist, track);
    }
    containing.len()
}

pub fn remove_track(&self, dbid: u64, disposition: TrackFileDisposition) -> Result<bool> {
    unsafe {
        let track = self.find_track_by_dbid(dbid);
        if track.is_null() { return Ok(false); }
        let file = self.track_file_path(track);
        self.unlink_track_from_all_playlists(track);
        ffi::itdb_track_remove(track);
        if disposition == TrackFileDisposition::DeleteAfterCommit {
            if let Some(path) = file { std::fs::remove_file(path).or_else(ignore_not_found)?; }
        }
        Ok(true)
    }
}
```

Keep `delete_track(dbid)` as a compatibility wrapper calling `remove_track(dbid, DeleteAfterCommit).map(|_| ())`. Implement Plan 3's `unlink_track_keep_file` through `remove_track(dbid, Keep)`. Refactor `wipe_all_tracks` to collect DBIDs, not track pointers, and call the same method for each DBID. The file-deletion timing in Plan 3 remains journal-controlled; production Plan 3 code must use `Keep` until its verified cleanup phase.

- [ ] **Step 4: Route ordinary remove, modify, replace/wipe, reconcile recovery, and Plan 3 replay through the helper**

Use `rg -n "itdb_track_(remove|unlink)|itdb_playlist_remove_track|delete_track\(" crates/classick/src` and change every track-freeing production call site outside `ipod/db.rs` to `remove_track` or `unlink_track_keep_file`. The final search must show no null-playlist removal and no raw `itdb_track_remove` outside the wrapper.

- [ ] **Step 5: Run focused deletion suites GREEN**

Run: `cargo test -p classick --test playlist_track_unlink_integration -- --test-threads=1 && cargo test -p classick --test wipe_all_tracks_integration -- --test-threads=1 && cargo test -p classick --test device_playlists_integration -- --test-threads=1`

Expected: PASS; foreign normal/smart memberships retain only live tracks after write/reparse, and wipe reports the original track count.

- [ ] **Step 6: Commit safe unlinking**

```bash
git add crates/classick/src/ipod/db.rs crates/classick/src/apply_loop.rs crates/classick/tests/playlist_track_unlink_integration.rs crates/classick/tests/wipe_all_tracks_integration.rs
git diff --cached
git commit -m "fix(ipod): unlink tracks from every playlist safely"
```

---

### Task 4: Device-authoritative managed ownership store

**Files:**
- Create: `crates/classick/src/ipod/playlist_ownership.rs`
- Create: `crates/classick/tests/playlist_ownership_integration.rs`
- Modify: `crates/classick/src/ipod/mod.rs`
- Modify: `crates/classick/src/device_state.rs`
- Modify: `crates/classick/src/ipod/layout.rs`
- Modify: `crates/classick/src/atomic_file.rs`

**Interfaces:**
- Consumes: Plan 2 `AtomicFileWriter`; raw serial from Plan 1; device path `/iPod_Control/classick/managed_playlists.json`; host cache `devices/<sanitized-serial>/managed_playlists.json`.
- Produces: `ManagedPlaylistOwnership`; `ManagedPlaylistEntry`; `DeviceOwnershipStore`; strict `validate_for_serial`; read-only load with authority origin.

- [ ] **Step 1: Write failing schema, authority, and atomic-publication tests**

```rust
#[test]
fn present_invalid_device_record_never_falls_back_to_host_cache() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    fixture.write_host_cache(ownership("RAW-Serial", "mix", 7));
    fixture.write_device_bytes(b"{broken");
    let error = fixture.store.load_device().unwrap_err();
    assert!(format!("{error:#}").contains("invalid device playlist ownership"));
}

#[test]
fn missing_device_record_is_empty_authority_not_host_permission() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    fixture.write_host_cache(ownership("RAW-Serial", "mix", 7));
    assert_eq!(fixture.store.load_device().unwrap().playlists, BTreeMap::new());
}

#[test]
fn publish_device_is_replace_atomic_and_validates_serial() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let wrong = ownership("OTHER", "mix", 7);
    assert!(fixture.store.publish_device(&wrong).is_err());
    assert!(!fixture.device_path().exists());
    let right = ownership("RAW-Serial", "mix", 7);
    fixture.store.publish_device(&right).unwrap();
    assert_eq!(fixture.store.load_device().unwrap(), right);
}
```

Also test schema-version rejection, zero Apple ID rejection, empty/unsafe slug rejection, unknown expected kind rejection, deterministic BTreeMap serialization, temp-write failure preserving the previous device file, and host-cache failure returning a warning without changing device bytes.

- [ ] **Step 2: Run ownership tests and confirm RED**

Run: `cargo test -p classick --test playlist_ownership_integration -- --test-threads=1`

Expected: FAIL to compile because `playlist_ownership` and `DeviceOwnershipStore` do not exist.

- [ ] **Step 3: Implement strict ownership DTO and store**

```rust
pub enum OwnershipOrigin { Device, Missing }

pub struct LoadedPlaylistOwnership {
    pub value: ManagedPlaylistOwnership,
    pub origin: OwnershipOrigin,
}

pub struct DeviceOwnershipStore {
    mount: PathBuf,
    serial: String,
    host_cache: PathBuf,
    atomic_writer: AtomicFileWriter,
}

impl DeviceOwnershipStore {
    pub fn new(mount: PathBuf, serial: String, host_cache: PathBuf, atomic_writer: AtomicFileWriter) -> Self;
    pub fn load_device(&self) -> Result<ManagedPlaylistOwnership>;
    pub fn load_device_read_only(&self) -> Result<ManagedPlaylistOwnership>;
    pub fn publish_device(&self, candidate: &ManagedPlaylistOwnership) -> Result<()>;
    pub fn refresh_host_cache(&self, candidate: &ManagedPlaylistOwnership) -> Result<Option<String>>;
}
```

Both load methods must avoid creating directories. Missing device file returns an empty v1 record for the connected raw serial. A present invalid device file fails closed. `publish_device` validates schema, exact raw serial, safe non-empty slugs, nonzero IDs, and `expected_kind == Normal`, then atomically replaces the device path and reparses bytes from disk. `refresh_host_cache` runs only after device publication; it returns `Ok(Some(warning))` for cache failure and never edits device state.

Treat the existing host `ManagedPlaylists { names }` file as an untrusted legacy cache. Preserve it for diagnostics, but do not migrate its IDs into connected-device authority. The first device-authoritative reconcile may create fresh managed playlists and leave formerly host-recorded playlists foreign; this one-time duplication is safer than host-granted deletion authority.

- [ ] **Step 4: Run ownership tests GREEN**

Run: `cargo test -p classick --test playlist_ownership_integration -- --test-threads=1`

Expected: PASS; invalid/present device truth blocks mutation, absent device truth is empty even with a populated host cache, and cache failure cannot roll back device truth.

- [ ] **Step 5: Commit the authority store**

```bash
git add crates/classick/src/ipod/playlist_ownership.rs crates/classick/tests/playlist_ownership_integration.rs crates/classick/src/ipod/mod.rs crates/classick/src/device_state.rs crates/classick/src/ipod/layout.rs crates/classick/src/atomic_file.rs
git diff --cached
git commit -m "feat(ipod): make playlist ownership device authoritative"
```

---

### Task 5: Guarded managed reconcile that stages candidate ownership

**Files:**
- Modify: `crates/classick/src/ipod/device_playlists.rs`
- Modify: `crates/classick/src/ipod/db.rs`
- Modify: `crates/classick/src/apply_loop.rs`
- Modify: `crates/classick/tests/device_playlists_integration.rs`
- Modify: `crates/classick/tests/playlists_e2e.rs`

**Interfaces:**
- Consumes: `DesiredPlaylist`; connected `ManagedPlaylistOwnership`; `OwnedDb::playlist_kind_by_id(id) -> Option<PlaylistStructuralKind>`.
- Produces: `reconcile_candidate(db: &OwnedDb, desired: &[DesiredPlaylist], previous: &ManagedPlaylistOwnership) -> Result<PlaylistReconcileOutcome>`; no filesystem writes.

- [ ] **Step 1: Replace old reconcile expectations with failing structural-guard tests**

```rust
#[test]
fn stale_managed_ids_never_grant_ownership_to_special_playlists() {
    for target in [SpecialTarget::Master, SpecialTarget::Podcast, SpecialTarget::Smart] {
        let fixture = ReconcileFixture::with_special_target(target);
        let prior = ownership(&fixture.serial, "mix", fixture.special_id);
        let outcome = reconcile_candidate(&fixture.db, &[fixture.desired_mix()], &prior).unwrap();
        assert!(fixture.playlist_exists(fixture.special_id));
        let replacement = &outcome.candidate_ownership.playlists["mix"];
        assert_ne!(replacement.apple_playlist_id, fixture.special_id);
        assert_eq!(replacement.expected_kind, ManagedPlaylistKind::Normal);
        assert!(outcome.diagnostics.iter().any(|d| matches!(d, PlaylistDiagnostic::InvalidManagedAssociation { .. })));
    }
}

#[test]
fn unsubscription_removes_only_a_recorded_valid_normal_playlist() {
    let fixture = ReconcileFixture::with_foreign_name_collision();
    let prior = ownership(&fixture.serial, "mix", fixture.managed_normal_id);
    let outcome = reconcile_candidate(&fixture.db, &[], &prior).unwrap();
    assert!(!fixture.playlist_exists(fixture.managed_normal_id));
    assert!(fixture.playlist_exists(fixture.foreign_same_name_id));
    assert!(outcome.candidate_ownership.playlists.is_empty());
}
```

Add preservation cases for master, podcast, On-The-Go normal, arbitrary empty smart, foreign normal, corrupt/missing recorded ID, two desired slugs with the same display name, and rename-by-slug.

- [ ] **Step 2: Run device playlist tests and confirm RED**

Run: `cargo test -p classick --test device_playlists_integration -- --test-threads=1`

Expected: FAIL because old `reconcile_in` writes host ownership immediately and `ensure_managed_playlist` rejects only master, not podcast or smart.

- [ ] **Step 3: Add exact normal-target guard to `OwnedDb`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistStructuralKind { Normal, Master, Podcast, Smart }

pub fn playlist_kind_by_id(&self, id: u64) -> Option<PlaylistStructuralKind> {
    unsafe {
        let playlist = ffi::itdb_playlist_by_id(self.0, id);
        if playlist.is_null() { return None; }
        Some(if ffi::itdb_playlist_is_mpl(playlist) != 0 {
            PlaylistStructuralKind::Master
        } else if ffi::itdb_playlist_is_podcasts(playlist) != 0 || (*playlist).podcastflag != 0 {
            PlaylistStructuralKind::Podcast
        } else if (*playlist).is_spl != 0 {
            PlaylistStructuralKind::Smart
        } else {
            PlaylistStructuralKind::Normal
        })
    }
}
```

Reuse a recorded ID in `ensure_managed_playlist` only when this returns `Normal`. Remove by ID only when it returns `Normal`. Delete `remove_playlist_by_name` from production reconcile; a name-only legacy host entry has no device authority and cannot authorize removal.

- [ ] **Step 4: Refactor reconcile into a filesystem-pure candidate operation**

Change the input from tuples to `DesiredPlaylist`. Load previous authority before calling; do not load/save inside this module. For each desired slug, reuse only a valid prior normal ID or create a fresh normal playlist. Preserve and diagnose invalid targets. For each no-longer-desired entry, remove only an exact prior ID that still resolves to normal. Return the candidate record and stats. An individual playlist mutation error must fail the coordinated checkpoint; do not swallow it and do not publish a partial candidate.

Remove `reconcile`, `reconcile_in`, `ManagedPlaylists::save`, and the warn-only `reconcile_playlists_step` behavior. `apply_loop` stages the returned candidate into Plan 3's journal and lets checkpoint finalization decide success. Playlist failure is no longer a warning-only convenience because ownership coherence is required.

- [ ] **Step 5: Run reconcile and end-to-end playlist tests GREEN**

Run: `cargo test -p classick --test device_playlists_integration -- --test-threads=1 && cargo test -p classick --test playlists_e2e -- --test-threads=1`

Expected: PASS; no test reads a host record as authority, all special/foreign targets survive, and desired membership order persists after write/reparse.

- [ ] **Step 6: Commit guarded candidate reconcile**

```bash
git add crates/classick/src/ipod/device_playlists.rs crates/classick/src/ipod/db.rs crates/classick/src/apply_loop.rs crates/classick/tests/device_playlists_integration.rs crates/classick/tests/playlists_e2e.rs
git diff --cached
git commit -m "fix(ipod): guard managed playlist targets structurally"
```

---

### Task 6: Exact firmware-system duplicate normalization

**Files:**
- Create: `crates/classick/src/ipod/playlist_normalize.rs`
- Create: `crates/classick/tests/playlist_normalization_integration.rs`
- Modify: `crates/classick/src/ipod/mod.rs`
- Modify: `crates/classick/src/sync_transaction.rs`

**Interfaces:**
- Consumes: `snapshot_playlists`; `match_firmware_profile`; Plan 3's in-memory candidate DB immediately before every `OwnedDb::write`.
- Produces: `normalize_firmware_playlists(db: &OwnedDb) -> Result<FirmwareNormalizationReport>`.

- [ ] **Step 1: Write the zero/one/many and preservation matrix**

```rust
#[test]
fn many_exact_instances_keep_newest_then_highest_id() {
    let fixture = NormalizationFixture::new();
    let old = fixture.add_exact("Alt", 100, 10);
    let tied_low = fixture.add_exact("Videos", 200, 20);
    let tied_high = fixture.add_exact("Vidéos", 200, 30);
    let report = normalize_firmware_playlists(&fixture.db).unwrap();
    assert_eq!(report.kept, vec![tied_high]);
    assert_eq!(report.removed, vec![old, tied_low]);
    assert!(fixture.playlist_exists(tied_high));
}

#[test]
fn near_matches_and_foreign_categories_are_untouched() {
    let fixture = NormalizationFixture::with_preservation_matrix();
    let before = fixture.snapshot_ids_and_payloads();
    normalize_firmware_playlists(&fixture.db).unwrap();
    assert_eq!(fixture.snapshot_ids_and_payloads(), before);
}
```

The matrix must include zero exact, one exact, different/localized names, one semantic field changed at a time, master, podcast, On-The-Go, arbitrary empty smart, foreign normal, and unknown smart rule profiles.

- [ ] **Step 2: Run normalization integration and confirm RED**

Run: `cargo test -p classick --test playlist_normalization_integration -- --test-threads=1`

Expected: FAIL to compile because `normalize_firmware_playlists` does not exist.

- [ ] **Step 3: Implement deterministic exact-only normalization**

```rust
pub fn normalize_firmware_playlists(db: &OwnedDb) -> Result<FirmwareNormalizationReport> {
    let audit = snapshot_playlists(db)?;
    let mut exact: Vec<_> = audit.into_iter()
        .filter(|p| match_firmware_profile(p) == Some(FirmwareProfileId::IpodClassicVideoKindV1))
        .collect();
    exact.sort_by_key(|p| (p.timestamp, p.id));
    let Some(keep) = exact.pop() else { return Ok(FirmwareNormalizationReport::default()); };
    let mut removed = Vec::new();
    for duplicate in exact {
        db.remove_firmware_playlist_exact(duplicate.id, FirmwareProfileId::IpodClassicVideoKindV1)?;
        removed.push(duplicate.id);
    }
    Ok(FirmwareNormalizationReport { kept: vec![keep.id], removed })
}
```

`remove_firmware_playlist_exact` must re-snapshot the target immediately before `itdb_playlist_remove` and fail if its payload no longer exactly matches. Never create a profile instance. Use highest timestamp, then highest ID as the explicit tie-break. Sort removed IDs by the same `(timestamp, id)` order for deterministic diagnostics.

- [ ] **Step 4: Invoke normalization at every coordinated iTunesDB publication**

Call it inside `CheckpointCoordinator` after all candidate track/playlist replay and before every `db.write`, including normal checkpoints, cancellation/pause finalization, replace-library, legacy-dirty repair, and recovery paths that actually rewrite a DB. Do not invoke it in recovery phases where the DB is already verified and only ownership publication remains.

- [ ] **Step 5: Run normalization and transaction tests GREEN**

Run: `cargo test -p classick --test playlist_normalization_integration -- --test-threads=1 && cargo test -p classick sync_transaction -- --test-threads=1`

Expected: PASS; zero/one exact instances are byte-stable, many keeps the newest/highest ID, and near matches are untouched.

- [ ] **Step 6: Commit firmware normalization**

```bash
git add crates/classick/src/ipod/playlist_normalize.rs crates/classick/tests/playlist_normalization_integration.rs crates/classick/src/ipod/mod.rs crates/classick/src/sync_transaction.rs
git diff --cached
git commit -m "fix(ipod): normalize exact firmware playlist duplicates"
```

---

### Task 7: Integrate recoverable ownership with Plan 3 checkpoint publication

**Files:**
- Modify: `crates/classick/src/pending_session.rs`
- Modify: `crates/classick/src/sync_transaction.rs`
- Modify: `crates/classick/src/apply_loop.rs`
- Modify: `crates/classick/tests/playlist_ownership_integration.rs`
- Modify: `crates/classick/tests/playlists_e2e.rs`

**Interfaces:**
- Consumes: Plan 3 `PendingSession`, `CheckpointCoordinator::publish`, `PendingPhase::{Staging, ReadyToPublish, DatabaseVerified, DeviceManifestPublished, CleanupComplete}`, Plan 2 `ManifestStore`.
- Produces: journaled `candidate_playlist_ownership`; exact `desired_playlist_memberships: BTreeMap<String, Vec<u64>>` copied from `PlaylistReconcileOutcome::desired_memberships`; post-reparse `verified_playlist_memberships`; shared phases `RockboxProjectionsPrepared`, `PlaylistOwnershipPublished`, and `RockboxProjectionsPublished`; `verify_managed_playlists`. Plan 6B plugs in the canonical `prepare_rockbox_projections(settled, candidate, verified, enabled) -> Result<BTreeMap<String, PendingRockboxOp>>` boundary defined above.

- [ ] **Step 1: Write failure-injection tests for the exact publication order**

Extend the existing Plan 3 injectable writer/failure harness with these points:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistFailurePoint {
    BeforeDatabaseWrite,
    AfterDatabaseVerified,
    BeforeProjectionPlanPersist,
    AfterProjectionPlanPrepared,
    BeforeDeviceOwnershipRename,
    AfterDeviceOwnershipRename,
    BeforeHostCacheRefresh,
}
```

Add tests asserting:

```rust
#[test]
fn ownership_failure_keeps_journal_and_recovery_reuses_verified_ids() {
    let fixture = TransactionFixture::with_desired_playlist("mix");
    fixture.inject(PlaylistFailurePoint::BeforeDeviceOwnershipRename);
    let error = fixture.publish().unwrap_err();
    assert!(format!("{error:#}").contains("publish device playlist ownership"));
    let pending = fixture.load_journal();
    assert_eq!(pending.phase, PendingPhase::RockboxProjectionsPrepared);
    let candidate_id = pending.candidate_playlist_ownership.as_ref().unwrap().playlists["mix"].apple_playlist_id;
    assert!(fixture.device_ownership_path().exists() == false);
    fixture.clear_failure();
    fixture.recover().unwrap();
    assert_eq!(fixture.device_ownership().playlists["mix"].apple_playlist_id, candidate_id);
    assert_eq!(fixture.count_playlists_named("Mix"), 1);
    assert!(!fixture.journal_path().exists());
}
```

Cover failure before DB write, DB verify failure/rollback, projection planning before/after the prepared phase, ownership temp write, ownership rename, post-device host cache, and unplug at every boundary. Add an explicit pre-6B zero-operation test proving recovery distinguishes `DeviceManifestPublished` (planning incomplete) from `RockboxProjectionsPrepared` with an empty map (planning complete) only when settled and candidate ownership contain no Rockbox records. Add a toggle-off test where settled ownership contains `rockbox: Some(previous)`: planning must produce `PendingRockboxOp { previous: Some(previous), desired: None }`, publish candidate ownership with `rockbox: None`, retain the journal as authorization, delete the exact previous file, then persist `RockboxProjectionsPublished`. Assert host-cache failure is warning-only after device truth, but device ownership failure is fatal/incomplete. Assert completed finalization is never emitted before ownership.

- [ ] **Step 2: Run ownership transaction tests and confirm RED**

Run: `cargo test -p classick --test playlist_ownership_integration publication -- --test-threads=1`

Expected: FAIL because `PendingSession` has no candidate ownership or ownership publication phases, and old reconcile saves host state before DB write.

- [ ] **Step 3: Extend the journal schema additively**

```rust
pub enum PendingPhase {
    Staging,
    ReadyToPublish,
    DatabaseVerified,
    DeviceManifestPublished,
    RockboxProjectionsPrepared,
    PlaylistOwnershipPublished,
    RockboxProjectionsPublished,
    CleanupComplete,
}

pub struct PendingSession {
    pub version: u32,
    pub session_id: SessionId,
    pub serial: String,
    pub phase: PendingPhase,
    pub albums: Vec<PendingAlbum>,
    pub staged_files: Vec<StagedFile>,
    pub obsolete_files: Vec<ObsoleteFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_playlist_ownership: Option<ManagedPlaylistOwnership>,
    #[serde(default)]
    pub desired_playlist_memberships: BTreeMap<String, Vec<u64>>,
    #[serde(default)]
    pub verified_playlist_memberships: Vec<VerifiedPlaylistMembership>,
}
```

Old Plan 3 journals deserialize with `None`/empty fields and skip playlist publication. Persist the candidate record and `PlaylistReconcileOutcome::desired_memberships` in `Staging` before any DB write. Persist exact IDs assigned by `itdb_playlist_add`; never rediscover by display name.

Keep this journal schema additive for Plan 6B's defaulted `pending_rockbox_ops: BTreeMap<String, PendingRockboxOp>`. Recovery serializers must round-trip that field once present; no 6A recovery rewrite may discard it. `RockboxProjectionsPrepared` and `RockboxProjectionsPublished` are shared phase boundaries. `reconcile_candidate` preserves each surviving slug's settled `rockbox` record until projection planning deliberately changes it; it never clears projection ownership itself. A pre-6B build may persist an empty `RockboxProjectionsPrepared`/`RockboxProjectionsPublished` fast path only when neither settled nor candidate ownership contains a Rockbox record. If either contains one and no projection planner is available, finalization remains incomplete with the journal intact.

Once Plan 6B is present, its planner owns both transitions for both enabled and disabled settings. After verified membership and `DeviceManifestPublished`, it computes the complete plan, persists the enriched candidate ownership plus `pending_rockbox_ops`, and only then persists `RockboxProjectionsPrepared`. Enabled planning produces required create/update/rename/delete operations. Disabled planning converts every exact settled projection into `PendingRockboxOp { previous: Some(record), desired: None }` and clears that slug's candidate `rockbox` field; it must not use an empty plan while any settled projection exists. After ownership publication, the journal's previous/desired pairs remain the authority for exact deletes. Only after executing or confirming every operation may Plan 6B persist `RockboxProjectionsPublished`.

- [ ] **Step 4: Verify the reopened DB before ownership publication**

Implement:

```rust
pub fn verify_managed_playlists(
    reopened: &OwnedDb,
    candidate: &ManagedPlaylistOwnership,
    desired_memberships: &BTreeMap<String, Vec<u64>>,
) -> Result<Vec<VerifiedPlaylistMembership>>;
```

For every slug, resolve its exact Apple ID, require `PlaylistStructuralKind::Normal`, walk members in GList order, reject null members, collect DBIDs and normalized absolute device paths (`/iPod_Control/Music/F00/file.m4a`), and compare DBIDs exactly with the desired order journaled for that slug. Any missing, duplicate, wrong-kind, or order mismatch fails verification and restores Plan 3's full DB/artwork rollback snapshot. Persist the verified list before moving beyond `DatabaseVerified`.

- [ ] **Step 5: Implement the combined publication and recovery state machine**

The exact order is:

1. Persist staged playlist mutations and `candidate_playlist_ownership` in the pending journal.
2. Normalize exact firmware duplicates in the candidate DB.
3. Write, reopen, and verify DB/artwork/tracks.
4. Verify every candidate managed ID and ordered membership; persist `DatabaseVerified` plus `verified_playlist_memberships`.
5. Publish Plan 2 device manifest and persist `DeviceManifestPublished`.
6. Prepare the projection plan. In a pre-6B build, use the empty prepared fast path only when settled and candidate ownership both contain no `rockbox` record; otherwise fail incomplete and retain the journal. Once Plan 6B is present, it derives operations from settled ownership, `candidate_playlist_ownership`, and `verified_playlist_memberships` for either setting, persists the enriched candidate and complete operation map, then persists `RockboxProjectionsPrepared`. When disabled, every settled projection becomes a delete-only operation and the prepared candidate drops its `rockbox` field. A valid zero-operation Plan 6B result still persists the prepared phase.
7. Atomically publish the prepared device playlist ownership and persist `PlaylistOwnershipPublished`.
8. In the legal pre-6B empty fast path, persist `RockboxProjectionsPublished` without filesystem operations. With Plan 6B, execute or idempotently confirm every prepared operation—including toggle-off deletes authorized by the journal's `previous` record—before persisting `RockboxProjectionsPublished`.
9. Refresh the host ownership cache best-effort; record any warning in `CheckpointResult`.
10. Perform Plan 3 obsolete/pending cleanup, persist `CleanupComplete`, then remove the journal.

Recovery branches on phase. Before `DatabaseVerified`, use Plan 3's rollback/inspection rules. At or after `DatabaseVerified`, reopen and call `verify_managed_playlists` against the journal candidate. At `DeviceManifestPublished`, projection planning is incomplete: delegate to Plan 6B when installed; otherwise use the empty fast path only after proving both settled and candidate ownership contain no Rockbox record. A recorded projection without an available planner is an incomplete-finalization error, never an empty plan. At `RockboxProjectionsPrepared`, the journaled candidate and operation map are complete even when the map is empty; recovery must not re-plan them. After `PlaylistOwnershipPublished`, toggle-off deletion remains authorized by the journal's delete-only operations even though settled ownership now has `rockbox: None`. Resume projection publication from those operations, never from directory scanning. Never call `reconcile_candidate` during recovery. A device ownership file already equal to the prepared candidate is an idempotent success.

- [ ] **Step 6: Run transaction, playlist, and full Rust suites GREEN**

Run: `cargo test -p classick --test playlist_ownership_integration -- --test-threads=1 && cargo test -p classick --test playlists_e2e -- --test-threads=1 && cargo test -p classick -- --test-threads=1`

Expected: PASS; injected failures retain the journal, recovery keeps the same IDs and creates no duplicate playlists, host-cache failure is the only warning-only ownership failure, and finalization cannot complete before device truth.

- [ ] **Step 7: Commit checkpoint ownership integration**

```bash
git add crates/classick/src/pending_session.rs crates/classick/src/sync_transaction.rs crates/classick/src/apply_loop.rs crates/classick/tests/playlist_ownership_integration.rs crates/classick/tests/playlists_e2e.rs
git diff --cached
git commit -m "feat(sync): publish playlist ownership recoverably"
```

---

### Task 8: Automated preservation gate and physical causality gate

**Files:**
- Modify: `crates/classick/tests/playlist_audit_integration.rs`
- Modify: `crates/classick/tests/playlist_normalization_integration.rs`
- Modify: `crates/classick/tests/playlist_track_unlink_integration.rs`
- Modify: `crates/classick/tests/playlist_ownership_integration.rs`
- Modify: `LEARNINGS.md`

**Interfaces:**
- Consumes: `--audit-playlists`, Plan 3 coordinated checkpoint/recovery, exact profile `ipod-classic-video-kind-v1`.
- Produces: release-blocking evidence for the Apple-firmware causality question; no broadened deletion behavior.

- [ ] **Step 1: Add one aggregate automated preservation test**

The test creates master, podcast, On-The-Go, foreign normal, arbitrary empty smart, one exact profile, one near match, and managed normal playlists; deletes a shared track; stages a desired managed update; publishes; reparses; and asserts exact survival/payload/membership:

```rust
#[test]
fn coordinated_publication_preserves_every_non_owned_non_exact_playlist() {
    let fixture = FullIntegrityFixture::new();
    let preserved_before = fixture.foreign_payloads();
    fixture.stage_delete_and_playlist_update();
    fixture.publish().unwrap();
    let reopened = fixture.reopen();
    assert_eq!(fixture.foreign_payloads_from(&reopened), preserved_before.without_deleted_track());
    assert_eq!(fixture.exact_profile_count(&reopened), 1);
    assert_eq!(fixture.verified_managed_order(&reopened, "mix"), fixture.desired_mix_dbids());
    assert_eq!(fixture.device_ownership().playlists["mix"].expected_kind, ManagedPlaylistKind::Normal);
}
```

- [ ] **Step 2: Run the complete automated gate GREEN**

Run, sequentially:

```bash
cargo test -p classick --test playlist_audit_integration -- --test-threads=1
cargo test -p classick --test playlist_normalization_integration -- --test-threads=1
cargo test -p classick --test playlist_track_unlink_integration -- --test-threads=1
cargo test -p classick --test playlist_ownership_integration -- --test-threads=1
cargo test -p classick --test device_playlists_integration -- --test-threads=1
cargo test -p classick --test playlists_e2e -- --test-threads=1
cargo test -p classick --test wipe_all_tracks_integration -- --test-threads=1
cargo test -p classick -- --test-threads=1
```

Expected: every command exits 0; audit tree digests are unchanged; all write/reparse suites contain no dangling playlist members.

- [ ] **Step 3: Capture the physical baseline without writing**

With the source share mounted read-only and the iPod mounted, set shell variables explicitly, then capture hashes and audit:

```bash
export IPOD_MOUNT="/Volumes/IPOD"
export GATE_DIR="$HOME/Desktop/classick-playlist-gate-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$GATE_DIR"
cp "$IPOD_MOUNT/iPod_Control/iTunes/iTunesDB" "$GATE_DIR/iTunesDB.before"
shasum -a 256 "$GATE_DIR/iTunesDB.before" > "$GATE_DIR/iTunesDB.before.sha256"
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --audit-playlists > "$GATE_DIR/audit.before.json"
shasum -a 256 "$IPOD_MOUNT/iPod_Control/iTunes/iTunesDB" > "$GATE_DIR/iTunesDB.after-audit.sha256"
diff -u "$GATE_DIR/iTunesDB.before.sha256" "$GATE_DIR/iTunesDB.after-audit.sha256"
```

Expected: `diff` emits no output. Record the six exact profile IDs/timestamps from `audit.before.json`. If the baseline does not contain the expected six exact instances, stop and investigate rather than manufacturing the expected state.

- [ ] **Step 4: Isolate Classick-write causality**

Run one coordinated Classick publication with no firmware boot between write and audit. This write is authorized only after Plan 3 and Tasks 1–7 are GREEN:

```bash
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --apply
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --audit-playlists > "$GATE_DIR/audit.after-classick-before-boot.json"
cp "$IPOD_MOUNT/iPod_Control/iTunes/iTunesDB" "$GATE_DIR/iTunesDB.after-classick"
shasum -a 256 "$GATE_DIR/iTunesDB.after-classick" > "$GATE_DIR/iTunesDB.after-classick.sha256"
```

Expected: at most one exact `ipod-classic-video-kind-v1` instance, with every near match and foreign playlist unchanged except for safe removal of tracks deliberately deleted by the sync.

- [ ] **Step 5: Isolate first-boot and second-boot firmware causality**

Eject through Classick/the OS, boot Apple firmware once, remount without running Classick, and save `audit.after-boot-1.json`. Eject, boot Apple firmware a second time without a Classick write, remount, and save `audit.after-boot-2.json`:

```bash
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --audit-playlists > "$GATE_DIR/audit.after-boot-1.json"
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --audit-playlists > "$GATE_DIR/audit.after-boot-2.json"
```

Record exact IDs/timestamps after each boot. Do not run normalization between these two observations.

- [ ] **Step 6: Apply the release-blocking causality decision**

Release Plan 6A only if every newly observed Videos instance exactly matches one of the two registered `ipod-classic-video-kind-v1` encodings and the observations support one coherent category. If either boot produces a near match, a third representation, a second legitimate semantic payload, or a category distinction that the fixtures collapse, stop release, preserve all captured DB/audit evidence, add the new distinction to fixture-backed classification, and rerun Tasks 1, 6, and 8 before another device write. Never change matching to name/emptiness and never delete the unexplained record. This physical gate remains release-blocking after all automated checks pass.

- [ ] **Step 7: Re-run normalization and verify managed playlist playback**

Run one coordinated publication, then audit:

```bash
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --apply
cargo run -p classick --release -- --ipod "$IPOD_MOUNT" --audit-playlists > "$GATE_DIR/audit.final.json"
```

Expected: zero or one exact profile; all foreign and near-match IDs remain; every recorded managed ID is normal and resolves to the ordered membership in the journal/device record. Eject, boot Apple firmware, confirm the manual and smart-derived Classick playlists show the expected order, and play the first/middle/last track of each. Plan 6B owns Rockbox `.m3u8` playback and is not part of this gate.

- [ ] **Step 8: Record only the non-obvious physical result in `LEARNINGS.md`**

Add one concise bullet stating whether duplicates appeared after Classick write, first firmware boot, or second firmware boot; include the captured gate directory and profile ID. Do not add routine test-pass notes.

- [ ] **Step 9: Commit the final automated gate and learning**

```bash
git add crates/classick/tests/playlist_audit_integration.rs crates/classick/tests/playlist_normalization_integration.rs crates/classick/tests/playlist_track_unlink_integration.rs crates/classick/tests/playlist_ownership_integration.rs LEARNINGS.md
git diff --cached
git commit -m "test(ipod): gate playlist integrity on physical causality"
```

## Plan 6A Completion Criteria

- `--audit-playlists` serializes every observable playlist field and classification while leaving the entire device tree unchanged.
- The profile's two registered exact encodings are name-independent, one-field near matches of either are foreign unless they equal the other registered encoding exactly, and zero/one/many normalization keeps newest timestamp then highest ID without creating a profile record.
- Every track-freeing path uses the safe containing-playlist snapshot helper, and real write/reparse tests prove no dangling member survives in foreign normal or smart playlists.
- Managed ownership exists on the device with exact raw serial, schema version, expected normal kind, and Apple IDs; the host file is cache-only.
- Reconcile never adopts/removes by name, never trusts stale IDs targeting master/podcast/smart playlists, and stages a complete candidate record before DB publication.
- Plan 3 recovery resumes from journaled verified IDs without recreating playlists; device ownership failures retain the journal and block completed finalization.
- Projection preparation is crash-distinguishable from projection publication: the pre-6B empty fast path requires no settled/candidate Rockbox records, while Plan 6B owns enabled and disabled planning; toggle-off produces journal-authorized delete-only operations and cannot orphan a recorded `.m3u8` file.
- `VerifiedPlaylistMembership` supplies Plan 6B one authoritative ordered Apple membership and normalized device path list per slug.
- The still-release-blocking physical gate establishes when exact firmware records appear. Any contradictory, third, or unexplained payload blocks release without broadening deletion.
