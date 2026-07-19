# Rockbox Playlist Projection (Plan 6B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish every subscribed logical playlist as a safe, durable Rockbox `.m3u8` projection that is byte/order-equivalent to its verified Apple playlist, and recoverably remove only projection files Classick has positively recorded as its own.

**Architecture:** Pure projection code converts Plan 6A's verified ordered device paths into deterministic UTF-8 content and collision-safe FAT filenames. A narrow filesystem layer validates the managed directory and exact recorded filenames, then performs sibling-temp/fsync/atomic-no-replace publication and recorded-only deletes. A damaged same-name projection is repaired at the next deterministic collision filename, never replaced in place. Plan 3's pending-session journal carries every previous/desired projection operation; after 6A publishes device-authoritative ownership, finalization executes or resumes those operations without rerunning playlist reconcile or inventing new Apple playlist IDs.

**Tech Stack:** Rust, serde JSON, BLAKE3 (already present), libgpod-backed Plan 6A verification, Plan 3 checkpoint journal, FAT-compatible mounted-device filesystem behavior on Windows and macOS.

## Global Constraints

- This is Plan 6B. Do not begin until Plan 3's coordinated checkpoint and Plan 6A's playlist-integrity/ownership interfaces are implemented and GREEN.
- Rockbox output lives only at `/Playlists/Classick/`; do not write Rockbox's tag database, configuration, or any source-library path.
- Every subscribed playlist has one ordered logical membership. Apple and Rockbox representations must derive from the same `VerifiedPlaylistMembership` produced after the iTunesDB write/reopen/verification checkpoint.
- Files are UTF-8 `.m3u8` without BOM. Each line is an absolute slash-separated device path such as `/iPod_Control/Music/F00/ABC123.m4a`. Empty membership produces a zero-byte file.
- Device paths come only from the verified final device-track record. Reject NUL, CR, or LF and reject paths outside `iPod_Control/Music`; never serialize a host source path.
- Filenames use a readable FAT-safe display-name stem plus a stable ten-hex BLAKE3 slug hash. The exact relative filename is persisted; it is never rediscovered by scanning or display-name matching.
- Classick may remove only an exact path positively authorized by settled device ownership or by a surviving Plan 3 journal. Projection publication is always no-replace; a mismatched recorded target selects another deterministic collision filename. Unrecorded files are foreign even inside `/Playlists/Classick/` and remain byte-for-byte untouched.
- A stored projection path must be one Unicode filename directly below `/Playlists/Classick`: reject absolute paths, `/` or `\`, empty/`.`/`..`, non-`.m3u8`, NUL/control characters, and every symlink/reparse escape.
- A recorded `content_hash` is exactly 64 lowercase ASCII hexadecimal characters. Bad length, uppercase, and non-hex records fail closed before authorization.
- Rename/update publication order is new sibling temp write, file fsync, atomic no-replace rename, managed-directory fsync where supported, then old recorded file removal. On Unix, removal first moves the exact previous file to `.classick-delete-{BLAKE3(previous filename + NUL + previous hash)}.tmp`; that name is authorized only by the surviving previous journal record, and retry must validate and remove it before `RockboxProjectionsPublished`. A failed old-file removal keeps the journal's previous record for retry.
- Projection finalization has one writer. No other process may mutate `/Playlists/Classick/` while Classick is finalizing it. Held ancestor handles/descriptors, exact spelling, content hashes, reparse checks, and post-mutation identity checks fail closed for changes observable before a syscall. Supported macOS/Linux APIs do not provide an inode-compare-and-swap unlink, so an uncooperative concurrent leaf replacement in the final validation-to-syscall micro-window is outside this threat model; do not describe the implementation as safe against a hostile same-user concurrent writer.
- Unsubscribe removes the Apple playlist and its exact recorded Rockbox file in the same recoverable finalization. Disabling Rockbox compatibility removes recorded projections only after the Apple DB and device ownership checkpoint is safe.
- Replace Plan 6A's pre-6B shortcut “disabled means immediately `RockboxProjectionsPublished`” with “advance immediately only when `pending_rockbox_ops` is empty”; disabled-with-recorded-projections stages delete-only operations and must execute them first.
- Projection failure is finalization failure, not a warning. Preserve the pending journal, report incomplete finalization, keep the iPod connected, and recover on the next run.
- Recovery verifies Plan 6A's already-published Apple IDs and resumes the recorded projection operations. It must not rerun reconcile, create replacement playlists, or produce new Apple IDs.
- The implementation is core Rust and must behave identically on Windows and macOS. Use platform-specific atomic rename primitives only behind a safe shared interface.
- Keep every new/modified source file at or below roughly 500 lines. Split representation, filesystem, planning, and integration orchestration as named below.
- Fake iPod mounts must create `iPod_Control/Music/F00`. The source share remains read-only. Do not run the live mounted-iPod gate until Plans 1-6A automated gates are GREEN and backups are captured.
- No new dependency is required: use existing `blake3`, `serde`, `anyhow`, Windows `windows-sys`, and Unix `libc` dependencies. Do not add a general filesystem library for these operations.

---

## Required Cross-Plan Interfaces

Plan 6B consumes these exact Plan 6A interfaces; adapt 6A before starting if its implementation differs:

```rust
pub const MANAGED_PLAYLIST_OWNERSHIP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaylistKind { Normal }

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

pub struct DeviceOwnershipStore {
    mount: PathBuf,
    serial: String,
    host_cache: PathBuf,
    atomic_writer: AtomicFileWriter,
}
impl DeviceOwnershipStore {
    pub fn new(
        mount: PathBuf,
        serial: String,
        host_cache: PathBuf,
        atomic_writer: AtomicFileWriter,
    ) -> Self;
    pub fn load_device(&self) -> Result<ManagedPlaylistOwnership>;
    pub fn load_device_read_only(&self) -> Result<ManagedPlaylistOwnership>;
    pub fn publish_device(&self, candidate: &ManagedPlaylistOwnership) -> Result<()>;
    pub fn refresh_host_cache(&self, candidate: &ManagedPlaylistOwnership) -> Result<Option<String>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedPlaylistMembership {
    pub slug: String,
    pub apple_playlist_id: u64,
    pub ordered_dbids: Vec<u64>,
    pub ordered_ipod_paths: Vec<String>,
}
```

Plans 3 and 6A supply `PendingSession.candidate_playlist_ownership: Option<ManagedPlaylistOwnership>` and the playlist-publication checkpoint. Plan 6B adds the following journal payload plus prepared/published Rockbox phases:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRockboxOp {
    pub previous: Option<RockboxProjectionRecord>,
    pub desired: Option<RockboxProjectionRecord>,
}

pub struct PendingSession {
    // Plan 3 and Plan 6A fields remain unchanged.
    pub pending_rockbox_ops: BTreeMap<String, PendingRockboxOp>,
}

pub enum PendingPhase {
    // Plan 3 phases remain ordered before this point.
    RockboxProjectionsPrepared,
    PlaylistOwnershipPublished,
    RockboxProjectionsPublished,
    // Plan 3 cleanup phase follows.
}
```

The exact phase order is `DatabaseVerified → DeviceManifestPublished → RockboxProjectionsPrepared → PlaylistOwnershipPublished → RockboxProjectionsPublished → CleanupComplete`. During incomplete finalization, the authorization set is the union of settled device ownership and every value's `PendingRockboxOp.previous`/`desired` records in the durable journal map. The map key is the playlist slug; the operation never duplicates it. This is the only exception needed to bridge a rename or toggle-off: settled ownership has the desired single record shape, while the journal retains the exact old path until deletion succeeds. Once every operation succeeds, the coordinator persists `RockboxProjectionsPublished`, continues cleanup, and removes the journal; settled ownership then contains only desired records.

## File Map

- Create `crates/classick/src/rockbox_playlist.rs`: FAT-safe filename generation, recorded-name validation, verified path conversion, UTF-8 rendering, and content hashes.
- Create `crates/classick/src/rockbox_projection.rs`: pure desired-state/collision planner over verified memberships and settled ownership.
- Create `crates/classick/src/rockbox_projection_fs.rs`: managed-root validation, symlink/reparse rejection, durable atomic write, exact recorded delete, and injectable failure seam.
- Create `crates/classick/tests/rockbox_projection_integration.rs`: fake-mount publication, ownership, retry, foreign-file, equivalence, and recovery tests.
- Modify `crates/classick/src/lib.rs`: export the three focused modules.
- Modify `crates/classick/src/ipod/layout.rs`: add `/Playlists/Classick` path constants/helper; do not reuse the separate `iPod_Control/classick/playlists` backup mirror.
- Modify `crates/classick/src/ipod/device_playlists.rs`: extend 6A ownership records with projection data; no Rockbox filesystem I/O belongs here.
- Modify `crates/classick/src/pending_session.rs`: serde-defaulted slug-keyed projection operations plus `RockboxProjectionsPrepared` and `RockboxProjectionsPublished` phases.
- Modify `crates/classick/src/sync_transaction.rs`: plan, publish, execute, recover, and finalize Rockbox projection after `PlaylistOwnershipPublished`.
- Modify `crates/classick/src/apply_loop.rs`: pass the same verified playlist membership into the checkpoint; do not independently rebuild membership.
- Modify `LEARNINGS.md`: add one concise invariant after the full automated and physical gate.

---

### Task 1: Pure `.m3u8` Representation and Stable FAT-safe Names

**Files:**
- Create: `crates/classick/src/rockbox_playlist.rs`
- Modify: `crates/classick/src/lib.rs`
- Modify: `crates/classick/src/ipod/layout.rs`

**Interfaces:**

```rust
pub const ROCKBOX_PLAYLIST_DIR: &str = "Playlists/Classick";
pub const ROCKBOX_STEM_UTF16_LIMIT: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedRockboxPlaylist {
    pub relative_filename: String,
    pub bytes: Vec<u8>,
    pub content_hash: String,
}

pub fn validate_recorded_filename(value: &str) -> Result<()>;
pub fn validate_projection_record(record: &RockboxProjectionRecord) -> Result<()>;
pub fn candidate_filename(display_name: &str, slug: &str, collision_index: u32) -> String;
pub fn render_verified_paths(membership: &VerifiedPlaylistMembership) -> Result<Vec<u8>>;
pub fn render_verified_playlist(
    display_name: &str,
    membership: &VerifiedPlaylistMembership,
    collision_index: u32,
) -> Result<RenderedRockboxPlaylist>;

// ipod/layout.rs
pub fn rockbox_playlists_dir(mount: &Path) -> PathBuf;
```

Filename rules are exact: replace FAT-forbidden characters (`< > : " / \\ | ? *`), ASCII controls, and whitespace runs with one `-`; trim leading/trailing spaces, dots, and hyphens; use `Playlist` if empty; prefix `_` for case-insensitive DOS device stems `CON`, `PRN`, `AUX`, `NUL`, `COM1`…`COM9`, and `LPT1`…`LPT9`; truncate without splitting a scalar to at most 80 UTF-16 code units. Suffix `--{hash}.m3u8`, where attempt zero uses the first ten lowercase hex digits of `BLAKE3(slug UTF-8)` and later deterministic collision attempts use `BLAKE3(slug UTF-8 || 0x00 || collision_index little-endian)`.

Path rendering accepts either libgpod backslashes or slashes, strips leading separators, requires components `iPod_Control/Music/...` case-insensitively, rejects empty/dot/dot-dot components and NUL/CR/LF, and emits exactly one leading `/` plus slash components. Preserve `ordered_ipod_paths` order and duplicates exactly because 6A owns logical membership semantics. Non-empty output ends in `\n`; empty output is `Vec::new()`. `content_hash` is the full lowercase BLAKE3 hex of the exact bytes.

- [ ] **Step 1: Add the focused RED tests at the bottom of `rockbox_playlist.rs`.**

```rust
#[test]
fn name_is_readable_fat_safe_reserved_safe_and_stable() {
    let expected_hash = &blake3::hash(b"road-trip").to_hex().to_string()[..10];
    assert_eq!(candidate_filename("Road: Trip?", "road-trip", 0),
               format!("Road-Trip--{expected_hash}.m3u8"));
    assert!(candidate_filename("CON", "console", 0).starts_with("_CON--"));
    assert!(candidate_filename("日本語 プレイリスト", "jp", 0)
        .starts_with("日本語-プレイリスト--"));
    let long = "🎵".repeat(81);
    let stem = candidate_filename(&long, "long", 0).split("--").next().unwrap().to_string();
    assert!(stem.encode_utf16().count() <= ROCKBOX_STEM_UTF16_LIMIT);
    assert_ne!(candidate_filename("Road Trip", "road-trip", 0),
               candidate_filename("Road Trip", "road-trip", 1));
}

#[test]
fn recorded_filename_rejects_every_escape_shape() {
    for bad in ["", ".", "..", "/Gym.m3u8", "C:\\Gym.m3u8", "a/b.m3u8",
                "a\\b.m3u8", "Gym.m3u", "Gym.M3U8", "Gym\n.m3u8", "Gym\0.m3u8"] {
        assert!(validate_recorded_filename(bad).is_err(), "accepted {bad:?}");
    }
    assert!(validate_recorded_filename("Gym--0123456789.m3u8").is_ok());
}

#[test]
fn recorded_hash_requires_exact_lowercase_blake3_hex() {
    let valid = RockboxProjectionRecord {
        relative_filename: "Gym--0123456789.m3u8".into(),
        content_hash: "a".repeat(64),
    };
    assert!(validate_projection_record(&valid).is_ok());
    for bad in ["a".repeat(63), "a".repeat(65), "A".repeat(64),
                format!("{}g", "a".repeat(63))] {
        let mut record = valid.clone();
        record.content_hash = bad;
        assert!(validate_projection_record(&record).is_err());
    }
}

#[test]
fn render_is_utf8_without_bom_absolute_slash_ordered_and_hashed() {
    let m = VerifiedPlaylistMembership {
        slug: "mix".into(), apple_playlist_id: 41,
        ordered_dbids: vec![102, 101],
        ordered_ipod_paths: vec![
            r"iPod_Control\Music\F02\B.m4a".into(),
            "iPod_Control/Music/F00/A.m4a".into(),
        ],
    };
    let rendered = render_verified_playlist("Mix", &m, 0).unwrap();
    assert_eq!(rendered.bytes,
        b"/iPod_Control/Music/F02/B.m4a\n/iPod_Control/Music/F00/A.m4a\n");
    assert!(!rendered.bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    assert_eq!(rendered.content_hash, blake3::hash(&rendered.bytes).to_hex().to_string());
}

#[test]
fn render_empty_is_a_valid_zero_byte_playlist() {
    let m = VerifiedPlaylistMembership {
        slug: "empty".into(), apple_playlist_id: 9,
        ordered_dbids: vec![], ordered_ipod_paths: vec![],
    };
    assert_eq!(render_verified_playlist("Empty", &m, 0).unwrap().bytes, Vec::<u8>::new());
}

#[test]
fn render_rejects_host_traversal_and_line_injection() {
    for path in ["/Users/me/Music/a.flac", r"C:\Music\a.flac",
                 "iPod_Control/Music/../Device/SysInfo", "iPod_Control/Music/F00/a\n.m4a"] {
        let m = VerifiedPlaylistMembership {
            slug: "bad".into(), apple_playlist_id: 1,
            ordered_dbids: vec![1],
            ordered_ipod_paths: vec![path.into()],
        };
        assert!(render_verified_playlist("Bad", &m, 0).is_err(), "accepted {path:?}");
    }
}
```

- [ ] **Step 2: Run the focused tests and verify RED.**

Run: `cargo test -p classick rockbox_playlist -- --nocapture`

Expected: compilation fails with unresolved `rockbox_playlist` module/functions; after the module declaration is added, assertions fail until the exact sanitizer/path renderer exists.

- [ ] **Step 3: Implement the interfaces with the rules above.** Use `char::len_utf16`, `blake3::Hasher`, `std::path::MAIN_SEPARATOR` only for filesystem joining (never rendered content), `anyhow::bail!` for invalid paths, and `ROCKBOX_PLAYLIST_DIR.split('/')` in `rockbox_playlists_dir` so Windows does not interpret the slash string as one component.

- [ ] **Step 4: Run focused and library tests GREEN.**

Run: `cargo test -p classick rockbox_playlist && cargo test -p classick ipod::layout`

Expected: all `rockbox_playlist` and `ipod::layout` tests pass; no ignored tests and no BOM/path-order differences.

- [ ] **Step 5: Commit the named files.**

```bash
git add crates/classick/src/rockbox_playlist.rs crates/classick/src/lib.rs crates/classick/src/ipod/layout.rs
git commit -m "feat(playlist): render safe Rockbox projections"
```

---

### Task 2: Secure Recorded-only Projection Filesystem

**Files:**
- Create: `crates/classick/src/rockbox_projection_fs.rs`
- Modify: `crates/classick/src/lib.rs`
- Test: unit tests in `rockbox_projection_fs.rs`

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState { Missing, RecordedFile, ForeignFile }

pub trait ProjectionIo {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState>;
    fn write_durable(
        &self,
        name: &str,
        bytes: &[u8],
        authorized: &HashSet<String>,
        replace_recorded: bool,
    ) -> Result<()>;
    fn remove_recorded(&self, name: &str, authorized: &HashSet<String>) -> Result<bool>;
}

pub struct DeviceProjectionFs { mount: PathBuf }
impl DeviceProjectionFs {
    pub fn new(mount: PathBuf) -> Self;
    pub fn validate_managed_root(&self) -> Result<PathBuf>;
}
```

`validate_managed_root` walks `mount`, `mount/Playlists`, and `mount/Playlists/Classick` with `symlink_metadata`; any symlink is an error. It creates missing directories one component at a time and revalidates after creation. Canonicalized managed root must remain below canonicalized mount. Every operation calls this validation immediately before touching a target and rejects a symlink target.

`write_durable` requires `name` in `authorized` and revalidates the target immediately before mutation. It uses a unique sibling `.{name}.classick-{pid}-{counter}.tmp` opened with `create_new(true)`, writes all bytes, calls `File::sync_all`, then atomically renames. With `replace_recorded=false`, the platform primitive must fail if the destination appeared: Windows `MoveFileExW` with `MOVEFILE_WRITE_THROUGH` and without `MOVEFILE_REPLACE_EXISTING`; macOS `renamex_np(..., RENAME_EXCL)`; Linux/test Unix `renameat2(..., RENAME_NOREPLACE)` with a same-directory hard-link/unlink fallback only for `ENOSYS`. With `replace_recorded=true`, require the existing target to be a regular non-symlink authorized file before using replace-existing + write-through semantics. Sync the managed directory on Unix after rename/delete; treat unsupported directory sync on Windows as success. Remove the temp on every error.

`target_state` returns `RecordedFile` only when `name` is in `authorized` and is a regular non-symlink file. An existing name absent from `authorized` is `ForeignFile`; directories and special files are also foreign. `remove_recorded` requires `name` in `authorized`, revalidates the target, removes one regular file, directory-syncs, and returns false for already absent. Thus a caller cannot turn a foreign overwrite/delete into an authorized mutation with a bare Boolean.

- [ ] **Step 1: Add RED filesystem tests.** Use the project `target/test-tmp/<pid>-<counter>` pattern and, on Unix, real `std::os::unix::fs::symlink` values.

```rust
#[test]
fn durable_write_has_exact_bytes_and_leaves_no_temp() {
    let fs = fixture();
    let authorized = HashSet::from(["Gym--0123456789.m3u8".to_string()]);
    fs.write_durable("Gym--0123456789.m3u8", b"/a\n", &authorized, false).unwrap();
    assert_eq!(std::fs::read(fs.root().join("Gym--0123456789.m3u8")).unwrap(), b"/a\n");
    assert!(std::fs::read_dir(fs.root()).unwrap()
        .all(|e| !e.unwrap().file_name().to_string_lossy().contains(".classick-")));
}

#[test]
fn foreign_collision_is_classified_and_never_replaced() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    std::fs::write(fs.root().join(name), b"foreign").unwrap();
    assert_eq!(fs.target_state(name, &HashSet::new()).unwrap(), TargetState::ForeignFile);
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs.write_durable(name, b"classick", &authorized, false).is_err());
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"foreign");
}

#[cfg(unix)]
#[test]
fn symlinked_root_and_target_are_rejected_without_touching_escape() {
    let outside = temp_dir("outside");
    let mount = temp_dir("mount");
    std::fs::create_dir_all(mount.join("Playlists")).unwrap();
    std::os::unix::fs::symlink(&outside, mount.join("Playlists/Classick")).unwrap();
    let fs = DeviceProjectionFs::new(mount);
    let authorized = HashSet::from(["x.m3u8".to_string()]);
    assert!(fs.write_durable("x.m3u8", b"owned", &authorized, false).is_err());
    assert!(!outside.join("x.m3u8").exists());
}

#[test]
fn recorded_delete_is_idempotent_and_rejects_traversal() {
    let fs = fixture();
    let name = "Gym--0123456789.m3u8";
    std::fs::write(fs.root().join(name), b"owned").unwrap();
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs.remove_recorded(name, &authorized).unwrap());
    assert!(!fs.remove_recorded(name, &authorized).unwrap());
    assert!(fs.remove_recorded("../foreign.m3u8", &authorized).is_err());
}
```

- [ ] **Step 2: Run tests and verify RED.**

Run: `cargo test -p classick rockbox_projection_fs -- --nocapture`

Expected: compilation fails because `DeviceProjectionFs`, `ProjectionIo`, and the platform no-replace rename helper do not exist.

- [ ] **Step 3: Implement the shared filesystem API and private `cfg(windows)`, `cfg(target_os="macos")`, and fallback Unix rename helpers.** Keep the public module below 500 lines; if platform glue pushes it over, create private sibling files `rockbox_projection_fs/windows.rs` and `rockbox_projection_fs/unix.rs` and expose one `rename_atomic(source, destination, replace)` function.

- [ ] **Step 4: Run focused tests GREEN on the current platform, then cross-check compilation.**

Run on macOS: `cargo test -p classick rockbox_projection_fs && cargo check -p classick`

Run on Windows: `cargo test -p classick rockbox_projection_fs; cargo check -p classick`

Expected: all tests pass on both platforms; collision and symlink tests do not alter the foreign/escape file; `cargo check` emits no missing platform symbol/import error.

- [ ] **Step 5: Commit the named files.**

```bash
git add crates/classick/src/rockbox_projection_fs.rs crates/classick/src/rockbox_projection_fs crates/classick/src/lib.rs
git commit -m "feat(playlist): add durable recorded-only projection I/O"
```

If platform subdirectories were not needed, omit that nonexistent path from `git add`.

---

### Task 3: Pure Projection Planning, Collision Selection, and Ownership Candidate

**Files:**
- Create: `crates/classick/src/rockbox_projection.rs`
- Modify: `crates/classick/src/lib.rs`
- Modify: `crates/classick/src/ipod/device_playlists.rs`
- Test: unit tests in `rockbox_projection.rs`

**Interfaces:**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredVerifiedPlaylist {
    pub display_name: String,
    pub membership: VerifiedPlaylistMembership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlan {
    pub candidate_ownership: ManagedPlaylistOwnership,
    pub operations: BTreeMap<String, PendingRockboxOp>,
}

pub fn plan_projection(
    serial: &str,
    enabled: bool,
    desired: &[DesiredVerifiedPlaylist],
    settled: &ManagedPlaylistOwnership,
    candidate: &ManagedPlaylistOwnership,
    io: &dyn ProjectionIo,
) -> Result<ProjectionPlan>;
```

Fail closed unless `settled.device_serial == serial` and `candidate.device_serial == serial`. Validate every prior/candidate `RockboxProjectionRecord` with `validate_projection_record` before it enters the authorization set. Every desired slug must exist in 6A's candidate ownership with matching `apple_playlist_id` and `expected_kind == ManagedPlaylistKind::Normal`; this explicitly permits a newly created 6A playlist that is absent from prior settled ownership. A mismatch means 6A verification/ownership is incoherent; do not stage any projection. Clone and enrich `candidate`; use `settled` only as old Rockbox authority for `previous` records, recorded replacements, and deletes.

When enabled, render each desired playlist's membership in its verified order and derive the preferred filename from its current display name. Try `candidate_filename(..., collision_index)` from zero upward: reuse the settled filename only when it equals that current candidate and its bytes match; accept a missing candidate; advance past a foreign or content-mismatched collision. Thus a display-name change produces a readable new filename while an unchanged name remains stable when its content is unchanged. Cap attempts at 256 and return an error if exhausted. Grant delete authority for the settled `rockbox` record only when its current bytes match the persisted old hash; if externally modified bytes occupy that name, preserve them as foreign and leave `previous` absent while ownership moves to the new deterministic candidate. Build candidate ownership with the desired filename/hash and insert a `PendingRockboxOp` under its slug key when bytes/hash/name differ or a still-verified old name must retire.

When disabled, keep every Apple ownership entry but set `rockbox=None`; create delete-only operations for every valid recorded projection. When a slug is absent from `desired`, 6A owns its Apple removal and removes it from candidate ownership; Plan 6B creates a delete-only operation from the settled record. Do not scan for any other `.m3u8`.

- [ ] **Step 1: Add pure RED tests with an in-memory `ProjectionIo`.**

```rust
#[test]
fn plans_same_order_and_hash_as_verified_membership() {
    let (settled, desired) = two_playlist_fixture();
    let candidate = candidate_from_desired("SERIAL", &desired);
    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &MemoryIo::default()).unwrap();
    assert_eq!(plan.operations.keys().map(String::as_str).collect::<Vec<_>>(),
               vec!["manual", "smart"]);
    let membership = &desired.iter().find(|p| p.membership.slug == "manual").unwrap().membership;
    assert_eq!(render_verified_paths(membership).unwrap(),
               b"/iPod_Control/Music/F00/A.m4a\n/iPod_Control/Music/F01/B.m4a\n");
}

#[test]
fn foreign_collision_selects_another_stable_hash_without_claiming_foreign() {
    let (settled, desired) = one_playlist_fixture("Road Trip", "road-trip");
    let first = candidate_filename("Road Trip", "road-trip", 0);
    let io = MemoryIo::with_foreign(&first, b"foreign");
    let candidate = candidate_from_desired("SERIAL", &desired);
    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();
    let chosen = &plan.operations["road-trip"].desired.as_ref().unwrap().relative_filename;
    assert_eq!(chosen, &candidate_filename("Road Trip", "road-trip", 1));
    assert_eq!(io.bytes(&first), b"foreign");
}

#[test]
fn rename_writes_new_then_records_old_for_retirement() {
    let (mut settled, desired) = one_playlist_fixture("New Name", "stable-slug");
    let old = RockboxProjectionRecord {
        relative_filename: candidate_filename("Old Name", "stable-slug", 0),
        content_hash: "old-hash".into(),
    };
    settled.playlists.get_mut("stable-slug").unwrap().rockbox = Some(old.clone());
    let candidate = candidate_from_desired("SERIAL", &desired);
    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &MemoryIo::recorded(&old)).unwrap();
    assert_eq!(plan.operations["stable-slug"].previous, Some(old));
    assert!(plan.operations["stable-slug"].desired.is_some());
}

#[test]
fn unsubscribe_and_toggle_off_plan_only_recorded_deletes() {
    let (settled, _) = two_playlist_fixture();
    let candidate = candidate_from_desired("SERIAL", &[]);
    let off = plan_projection("SERIAL", false, &[], &settled, &candidate, &MemoryIo::from_ownership(&settled)).unwrap();
    assert!(off.operations.values().all(|o| o.previous.is_some() && o.desired.is_none()));
    assert!(off.candidate_ownership.playlists.values().all(|e| e.rockbox.is_none()));
}

#[test]
fn apple_id_serial_or_kind_mismatch_fails_before_any_operation() {
    let (mut settled, desired) = one_playlist_fixture("Mix", "mix");
    settled.device_serial = "OTHER".into();
    let candidate = candidate_from_desired("SERIAL", &desired);
    assert!(plan_projection("SERIAL", true, &desired, &settled, &candidate, &MemoryIo::default()).is_err());
}

#[test]
fn newly_created_candidate_playlist_does_not_need_prior_settled_entry() {
    let settled = empty_ownership("SERIAL");
    let (_, desired) = one_playlist_fixture("New", "new");
    let candidate = candidate_from_desired("SERIAL", &desired);
    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &MemoryIo::default()).unwrap();
    assert_eq!(plan.candidate_ownership.playlists["new"].apple_playlist_id,
               desired[0].membership.apple_playlist_id);
    assert!(plan.operations["new"].previous.is_none());
}

#[test]
fn malformed_recorded_hash_never_grants_authority() {
    let (mut settled, desired) = one_playlist_fixture("Mix", "mix");
    settled.playlists.get_mut("mix").unwrap().rockbox = Some(RockboxProjectionRecord {
        relative_filename: "Mix--0123456789.m3u8".into(),
        content_hash: "A".repeat(64),
    });
    let candidate = candidate_from_desired("SERIAL", &desired);
    assert!(plan_projection("SERIAL", true, &desired, &settled, &candidate,
                            &MemoryIo::from_ownership(&settled)).is_err());
}
```

- [ ] **Step 2: Run tests and verify RED.**

Run: `cargo test -p classick rockbox_projection -- --nocapture`

Expected: compilation fails with unresolved planner types/functions.

- [ ] **Step 3: Implement `plan_projection` and extend the 6A serde DTOs exactly as shown in the cross-plan block.** Add `#[serde(default)]` only to the new `rockbox` field so an ownership record written by 6A before 6B upgrades safely to `None`; do not tolerate a missing schema version or serial.

- [ ] **Step 4: Run planner and ownership serialization tests GREEN.**

Run: `cargo test -p classick rockbox_projection && cargo test -p classick ipod::device_playlists`

Expected: all tests pass; legacy 6A records decode with `rockbox=None`; new records round-trip filename/hash exactly.

- [ ] **Step 5: Commit the named files.**

```bash
git add crates/classick/src/rockbox_projection.rs crates/classick/src/lib.rs crates/classick/src/ipod/device_playlists.rs
git commit -m "feat(playlist): plan Rockbox projection from verified ownership"
```

---

### Task 4: Plan 3 Journal Integration, Required Finalization, and Recovery

**Files:**
- Modify: `crates/classick/src/pending_session.rs`
- Modify: `crates/classick/src/sync_transaction.rs`
- Modify: `crates/classick/src/apply_loop.rs`
- Test: existing unit tests in `pending_session.rs` and `sync_transaction.rs`

**Interfaces:**

```rust
pub fn stage_playlist_projection(
    journal: &mut PendingSession,
    plan: ProjectionPlan,
) -> Result<()>;

pub fn publish_playlist_finalization(
    journal_store: &PendingSessionStore,
    journal: &mut PendingSession,
    ownership_store: &DeviceOwnershipStore,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()>;

pub fn recover_playlist_finalization(
    journal_store: &PendingSessionStore,
    journal: &mut PendingSession,
    ownership_store: &DeviceOwnershipStore,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()>;

fn execute_projection_ops(
    journal: &mut PendingSession,
    projection_io: &dyn ProjectionIo,
    verified_memberships: &BTreeMap<String, VerifiedPlaylistMembership>,
) -> Result<()>;
```

After `DatabaseVerified`, Plan 3 publishes the device manifest. Projection planning occurs only at `DeviceManifestPublished`: reuse 6A's verified exact-ID memberships, build `DesiredVerifiedPlaylist` values, then call `plan_projection` with both prior settled device ownership and 6A's candidate Apple ownership. `stage_playlist_projection` replaces `candidate_playlist_ownership` with the enriched candidate, stores `pending_rockbox_ops`, sets `RockboxProjectionsPrepared`, and durably republishes the journal as one transition. `publish_playlist_finalization` requires `RockboxProjectionsPrepared`; it publishes ownership, persists `PlaylistOwnershipPublished`, executes projection operations from freshly verified membership bytes, persists `RockboxProjectionsPublished`, calls `refresh_host_cache` best-effort (log its returned warning string or error without changing device truth), then lets Plan 3 continue cleanup.

For each `(slug, operation)`, compute its authorization set from the just-loaded settled device ownership plus every `previous`/`desired` record in the persisted journal map. Validate all names before the first write/delete. If `desired` exists, require the freshly verified membership under the same slug, require its Apple ID to equal candidate ownership, regenerate bytes with `render_verified_paths`, and verify their hash equals `desired.content_hash` before writing. Pass `replace_recorded=true` only when `previous.relative_filename == desired.relative_filename`; a newly selected name always uses no-replace, so a file racing into place remains foreign. Write desired durably first. Only after that succeeds, remove `previous` when its filename differs. Delete-only operations remove `previous` and need no membership. Missing recorded old files count as success only after syncing and revalidating the managed directory, which durably orders a prior successful unlink before journal phase advancement. On any error, return immediately without advancing `RockboxProjectionsPublished`; retain unchanged map entries and journal for retry. Re-running is idempotent: a desired file already holding the expected hash is not rewritten, and an already-removed previous file succeeds after that directory sync.

Recovery at `DeviceManifestPublished` deterministically runs read-only 6A verification and `plan_projection`, then persists the enriched candidate/map and `RockboxProjectionsPrepared`. Recovery at `RockboxProjectionsPrepared` reuses that persisted plan and publishes ownership; it never replans a collision choice, including the meaningful zero-operation plan. Recovery at `PlaylistOwnershipPublished` reloads/verifies published candidate Apple IDs/kinds, obtains a `BTreeMap<slug, VerifiedPlaylistMembership>`, and executes the already-persisted map. None of these paths calls `reconcile`, mutates Apple memberships, or allocates an iTunesDB playlist. Recovery from `RockboxProjectionsPublished` skips projection and resumes cleanup. Earlier phases follow Plan 3/6A DB ambiguity resolution.

- [ ] **Step 1: Add journal serde and transition RED tests.**

```rust
#[test]
fn old_journal_defaults_projection_ops_empty() {
    let journal: PendingSession = serde_json::from_str(include_str!(
        "../tests/fixtures/pending-session-plan3.json"
    )).unwrap();
    assert!(journal.pending_rockbox_ops.is_empty());
}

#[test]
fn cannot_publish_playlist_ownership_before_device_manifest() {
    let (store, mut journal, ownership, io, verified) = transaction_fixture(PendingPhase::DatabaseVerified);
    assert!(publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).is_err());
    assert_eq!(journal.phase, PendingPhase::DatabaseVerified);
    assert_eq!(io.write_count(), 0);
}

#[test]
fn prepared_zero_op_plan_is_not_replanned_after_crash() {
    let (store, mut journal, ownership, io, verified) = no_change_fixture();
    stage_playlist_projection(&mut journal, ProjectionPlan {
        candidate_ownership: ownership.load_device().unwrap(),
        operations: BTreeMap::new(),
    }).unwrap();
    assert_eq!(journal.phase, PendingPhase::RockboxProjectionsPrepared);
    let planned_journal_bytes = std::fs::read(store.path()).unwrap();
    recover_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();
    assert!(!planned_journal_bytes.is_empty());
    assert_eq!(io.collision_probe_count(), 0);
}

#[test]
fn rename_orders_new_write_before_old_delete() {
    let (store, mut journal, ownership, io, verified) = rename_fixture();
    publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();
    assert_eq!(io.events(), vec!["write:new.m3u8", "delete:old.m3u8"]);
    assert_eq!(journal.phase, PendingPhase::RockboxProjectionsPublished);
}

#[test]
fn failed_old_delete_retains_journal_authority_and_retries_without_rewrite() {
    let (store, mut journal, ownership, io, verified) = rename_fixture();
    io.fail_delete_once("old.m3u8");
    assert!(publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).is_err());
    assert_eq!(journal.phase, PendingPhase::PlaylistOwnershipPublished);
    assert_eq!(journal.pending_rockbox_ops["stable"].previous.as_ref().unwrap().relative_filename,
               "old.m3u8");
    publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();
    assert_eq!(io.events().iter().filter(|e| *e == "write:new.m3u8").count(), 1);
    assert_eq!(io.events().iter().filter(|e| *e == "delete:old.m3u8").count(), 2);
}

#[test]
fn unplug_during_write_keeps_finalization_incomplete() {
    let (store, mut journal, ownership, io, verified) = create_fixture();
    io.fail_next_write(std::io::ErrorKind::NotConnected);
    assert!(publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).is_err());
    assert_eq!(journal.phase, PendingPhase::PlaylistOwnershipPublished);
    assert!(store.path().exists());
}
```

- [ ] **Step 2: Run tests and verify RED.**

Run: `cargo test -p classick pending_session -- --nocapture && cargo test -p classick sync_transaction -- --nocapture`

Expected: compilation fails because `PendingRockboxOp`, `RockboxProjectionsPrepared`, `RockboxProjectionsPublished`, and finalization functions are absent.

- [ ] **Step 3: Implement journal fields/transitions and coordinator wiring.** Add `#[serde(default)]` to `pending_rockbox_ops`. At `DeviceManifestPublished`, thread the `BTreeMap<slug, VerifiedPlaylistMembership>` from 6A verification into `plan_projection`, persist the enriched candidate/map plus `RockboxProjectionsPrepared`, and only then publish ownership. Recovery at `RockboxProjectionsPrepared` reuses persisted choices; recovery at `PlaylistOwnershipPublished` executes the persisted map. Remove any independent source-path-to-manifest rejoin. Errors must retain Plan 3's finalizing state and surface the existing “Keep the iPod connected” path.

- [ ] **Step 4: Run transaction, apply-loop, and full Rust tests GREEN.**

Run: `cargo test -p classick pending_session && cargo test -p classick sync_transaction && cargo test -p classick apply_loop && cargo test -p classick`

Expected: all tests pass; injected projection failures never advance `RockboxProjectionsPublished`, remove the journal, or report `Completed`/`Cancelled`.

- [ ] **Step 5: Commit the named files.**

```bash
git add crates/classick/src/pending_session.rs crates/classick/src/sync_transaction.rs crates/classick/src/apply_loop.rs
git commit -m "feat(sync): finalize Rockbox playlists through the journal"
```

---

### Task 5: End-to-end Equivalence, Foreign Safety, Toggle/Unsubscribe, and Retry

**Files:**
- Create: `crates/classick/tests/rockbox_projection_integration.rs`
- Modify: `crates/classick/tests/device_playlists_integration.rs`
- Modify: `crates/classick/tests/playlists_e2e.rs`

**Interfaces:** Uses the production Plan 6A checkpoint seam and `DeviceProjectionFs`; no test-only publication shortcut may bypass device ownership or the pending journal.

Build fake mounts with `iPod_Control/Music/F00`, a writable iTunesDB, `/Playlists/Classick`, and an unrelated `/Playlists/Classick/Handmade.m3u8`. Use the committed `tagged.flac` fixture when a real track add is required; do not shell out to ffmpeg on macOS. Parse generated `.m3u8` with `std::str::from_utf8(bytes)?.lines()` and compare directly to `VerifiedPlaylistMembership.ordered_ipod_paths` normalized through Task 1.

Create this test harness in the integration file. Its implementation must call the production 6A reconcile/verify, Plan 3 journal store/coordinator, and `DeviceProjectionFs`; it must not write an ownership file or projection directly. `FailurePoint` is consumed once and returns `io::ErrorKind::NotConnected` at the named production I/O seam.

```rust
struct Harness {
    root: PathBuf,
    mount: PathBuf,
    serial: String,
    ownership_store: DeviceOwnershipStore,
    journal_store: PendingSessionStore,
    apple_write_count: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
enum FailurePoint { BeforeOwnershipPublish, ProjectionWrite, ProjectionRename, ProjectionDelete }

impl Harness {
    fn new() -> Self;
    fn sync(&mut self, enabled: bool, playlists: Vec<TestPlaylist>) -> Result<SyncResult>;
    fn recover(&mut self) -> Result<SyncResult>;
    fn fail_once(&mut self, point: FailurePoint);
    fn write_foreign(&self, name: &str, bytes: &[u8]);
    fn write_raw_device_ownership(&self, bytes: &[u8]);
    fn read_projection(&self, record: &RockboxProjectionRecord) -> Vec<u8>;
    fn projection_exists(&self, record: &RockboxProjectionRecord) -> bool;
    fn ownership(&self) -> ManagedPlaylistOwnership;
    fn journal(&self) -> Option<PendingSession>;
    fn foreign_hash(&self, name: &str) -> blake3::Hash;
    #[cfg(unix)]
    fn replace_managed_root_with_symlink(&self, outside: &Path);
}

struct TestPlaylist {
    slug: String,
    name: String,
    ordered_track_indexes: Vec<usize>,
}

struct SyncResult {
    verified: Vec<VerifiedPlaylistMembership>,
    ownership: ManagedPlaylistOwnership,
    completed: bool,
}

fn playlist(slug: &str, name: &str, ordered_track_indexes: &[usize]) -> TestPlaylist {
    TestPlaylist {
        slug: slug.into(),
        name: name.into(),
        ordered_track_indexes: ordered_track_indexes.to_vec(),
    }
}

fn rendered_lines(bytes: &[u8]) -> Vec<String> {
    std::str::from_utf8(bytes).unwrap().lines().map(str::to_owned).collect()
}

fn normalized_paths(membership: &VerifiedPlaylistMembership) -> Vec<String> {
    membership.ordered_ipod_paths.iter().map(|path| {
        let normalized = path.replace('\\', "/");
        format!("/{}", normalized.trim_start_matches('/'))
    }).collect()
}
```

- [ ] **Step 1: Add the complete integration matrix as named tests.**

```rust
#[test]
fn manual_and_smart_projection_match_verified_apple_order_byte_for_byte() {
    let mut h = Harness::new();
    let result = h.sync(true, vec![
        playlist("manual", "Manual", &[0, 1]),
        playlist("smart", "Smart", &[1, 0]),
    ]).unwrap();
    assert!(result.completed);
    for membership in &result.verified {
        let record = result.ownership.playlists[&membership.slug].rockbox.as_ref().unwrap();
        assert_eq!(rendered_lines(&h.read_projection(record)), normalized_paths(membership));
    }
}

#[test]
fn empty_playlist_publishes_zero_bytes_and_valid_ownership_hash() {
    let mut h = Harness::new();
    let result = h.sync(true, vec![playlist("empty", "Empty", &[])]).unwrap();
    let record = result.ownership.playlists["empty"].rockbox.as_ref().unwrap();
    let bytes = h.read_projection(record);
    assert!(bytes.is_empty());
    assert_eq!(record.content_hash, blake3::hash(&bytes).to_hex().to_string());
}

#[test]
fn same_display_name_and_foreign_filename_collisions_never_overwrite() {
    let mut h = Harness::new();
    let collision = candidate_filename("Mix", "mix-a", 0);
    h.write_foreign(&collision, b"foreign\n");
    let before = h.foreign_hash(&collision);
    let result = h.sync(true, vec![playlist("mix-a", "Mix", &[0]), playlist("mix-b", "Mix", &[1])]).unwrap();
    let a = result.ownership.playlists["mix-a"].rockbox.as_ref().unwrap();
    let b = result.ownership.playlists["mix-b"].rockbox.as_ref().unwrap();
    assert_ne!(a.relative_filename, b.relative_filename);
    assert_ne!(a.relative_filename, collision);
    assert_eq!(h.foreign_hash(&collision), before);
}

#[test]
fn rename_publishes_new_before_removing_old_and_settles_new_record() {
    let mut h = Harness::new();
    let first = h.sync(true, vec![playlist("stable", "Old Name", &[0])]).unwrap();
    let old = first.ownership.playlists["stable"].rockbox.clone().unwrap();
    let second = h.sync(true, vec![playlist("stable", "New Name", &[0])]).unwrap();
    let new = second.ownership.playlists["stable"].rockbox.clone().unwrap();
    assert_ne!(old.relative_filename, new.relative_filename);
    assert!(!h.projection_exists(&old));
    assert!(h.projection_exists(&new));
    assert_eq!(h.ownership().playlists["stable"].rockbox, Some(new));
}

#[test]
fn unsubscribe_removes_apple_and_exact_rockbox_but_preserves_foreign() {
    let mut h = Harness::new();
    h.write_foreign("Handmade.m3u8", b"/foreign/path.m4a\n");
    let foreign_before = h.foreign_hash("Handmade.m3u8");
    let first = h.sync(true, vec![playlist("gone", "Gone", &[0])]).unwrap();
    let old = first.ownership.playlists["gone"].rockbox.clone().unwrap();
    let second = h.sync(true, vec![]).unwrap();
    assert!(second.verified.iter().all(|p| p.slug != "gone"));
    assert!(!second.ownership.playlists.contains_key("gone"));
    assert!(!h.projection_exists(&old));
    assert_eq!(h.foreign_hash("Handmade.m3u8"), foreign_before);
}

#[test]
fn toggle_off_waits_for_ownership_checkpoint_then_removes_recorded_only() {
    let mut h = Harness::new();
    h.write_foreign("Handmade.m3u8", b"foreign");
    let foreign_before = h.foreign_hash("Handmade.m3u8");
    let first = h.sync(true, vec![playlist("keep", "Keep", &[0])]).unwrap();
    let record = first.ownership.playlists["keep"].rockbox.clone().unwrap();
    h.fail_once(FailurePoint::BeforeOwnershipPublish);
    assert!(h.sync(false, vec![playlist("keep", "Keep", &[0])]).is_err());
    assert!(h.projection_exists(&record));
    let recovered = h.recover().unwrap();
    assert!(recovered.completed);
    assert!(!h.projection_exists(&record));
    assert_eq!(h.foreign_hash("Handmade.m3u8"), foreign_before);
}

#[test]
fn failed_delete_recovery_retries_exact_old_path_without_new_apple_id() {
    let mut h = Harness::new();
    let first = h.sync(true, vec![playlist("stable", "Old", &[0])]).unwrap();
    let apple_id = first.ownership.playlists["stable"].apple_playlist_id;
    let old = first.ownership.playlists["stable"].rockbox.clone().unwrap();
    h.fail_once(FailurePoint::ProjectionDelete);
    assert!(h.sync(true, vec![playlist("stable", "New", &[0])]).is_err());
    assert!(h.journal().unwrap().pending_rockbox_ops["stable"].previous.is_some());
    let apple_writes_before = h.apple_write_count.load(Ordering::SeqCst);
    let recovered = h.recover().unwrap();
    assert_eq!(recovered.ownership.playlists["stable"].apple_playlist_id, apple_id);
    assert_eq!(h.apple_write_count.load(Ordering::SeqCst), apple_writes_before);
    assert!(!h.projection_exists(&old));
    assert!(h.journal().is_none());
}

#[test]
fn corrupt_record_fails_closed_without_foreign_mutation() {
    for bad in ["../x.m3u8", "/x.m3u8", "a/b.m3u8", "a\\b.m3u8"] {
        let mut h = Harness::new();
        h.write_foreign("Handmade.m3u8", b"foreign");
        let before = h.foreign_hash("Handmade.m3u8");
        let invalid = serde_json::json!({
            "schema_version": 1,
            "device_serial": h.serial.clone(),
            "playlists": { "bad": {
                "apple_playlist_id": 7,
                "expected_kind": "normal",
                "rockbox": { "relative_filename": bad, "content_hash": "bad" }
            }}
        });
        h.write_raw_device_ownership(&serde_json::to_vec(&invalid).unwrap());
        assert!(h.sync(false, vec![]).is_err(), "accepted {bad:?}");
        assert_eq!(h.foreign_hash("Handmade.m3u8"), before);
    }
}

#[test]
fn unplug_at_write_rename_and_delete_recovers_idempotently() {
    for point in [FailurePoint::ProjectionWrite, FailurePoint::ProjectionRename,
                  FailurePoint::ProjectionDelete] {
        let mut h = Harness::new();
        h.sync(true, vec![playlist("stable", "Old", &[0])]).unwrap();
        h.fail_once(point);
        assert!(h.sync(true, vec![playlist("stable", "New", &[0])]).is_err());
        assert!(h.journal().is_some());
        let recovered = h.recover().unwrap();
        let record = recovered.ownership.playlists["stable"].rockbox.as_ref().unwrap();
        assert_eq!(record.content_hash,
                   blake3::hash(&h.read_projection(record)).to_hex().to_string());
        assert!(h.journal().is_none());
    }
}

#[cfg(unix)]
#[test]
fn symlink_swap_after_staging_cannot_escape_managed_root() {
    let mut h = Harness::new();
    h.fail_once(FailurePoint::BeforeOwnershipPublish);
    assert!(h.sync(true, vec![playlist("mix", "Mix", &[0])]).is_err());
    let outside = h.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    h.replace_managed_root_with_symlink(&outside);
    assert!(h.recover().is_err());
    assert!(h.journal().is_some());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}
```

- [ ] **Step 2: Run the new target and verify RED.**

Run: `cargo test -p classick --test rockbox_projection_integration -- --test-threads=1 --nocapture`

Expected: tests fail at the absent production checkpoint/projection seam, not because fake mount `F00` or the fixture is missing.

- [ ] **Step 3: Wire only gaps exposed by the integration tests.** If a production file needs a change, add it to this task's named-files list before editing and keep it below 500 lines. Do not weaken recorded-path validation or make projection warn-only to turn failures GREEN.

- [ ] **Step 4: Run focused and full tests GREEN on both OSes.**

Run on macOS/Linux shell:

```bash
cargo test -p classick --test rockbox_projection_integration -- --test-threads=1
cargo test -p classick --test device_playlists_integration -- --test-threads=1
cargo test -p classick --test playlists_e2e -- --test-threads=1
cargo test -p classick
```

Run the same commands in PowerShell with identical arguments. Expected: every test passes on Windows and macOS; generated file bytes/hashes/order are identical for the same fixtures; foreign hashes remain unchanged.

- [ ] **Step 5: Commit tests and any named production gaps.**

```bash
git add crates/classick/tests/rockbox_projection_integration.rs crates/classick/tests/device_playlists_integration.rs crates/classick/tests/playlists_e2e.rs
git commit -m "test(playlist): prove Apple and Rockbox projection equivalence"
```

---

### Task 6: Cross-platform Build Gate and Live Rockbox Playback Gate

**Files:**
- Modify: `LEARNINGS.md`

**Interfaces:** No new API. This is the release-blocking verification of the exact core behavior shipped by Tasks 1-5.

- [ ] **Step 1: Run formatting, diagnostics, and full automated gates on macOS.**

```bash
cargo fmt --all -- --check
cargo clippy -p classick --all-targets -- -D warnings
cargo test -p classick
cargo build -p classick --release
ui/macos/bundle.sh
```

Expected: every command exits zero; no LSP/rust-analyzer diagnostics remain; `ui/macos/Classick.app` bundles the release core.

- [ ] **Step 2: Run the equivalent Windows core gate in a normal MSVC PowerShell with MSYS2 available.**

```powershell
cargo fmt --all -- --check
cargo clippy -p classick --all-targets -- -D warnings
cargo test -p classick
cargo build -p classick --release
```

Expected: every command exits zero; `target/release/classick.exe` and vendored DLL closure exist; projection integration tests pass with the same bytes/hashes as macOS.

- [ ] **Step 3: Capture the live-device backup and baseline before any write.** Replace `<IPOD>` with the mounted root (`/Volumes/<name>` on macOS or `G:` on Windows) and `<BACKUP>` with a timestamped local directory outside the device.

macOS:

```bash
mkdir -p "<BACKUP>"
cp -p "<IPOD>/iPod_Control/iTunes/iTunesDB" "<BACKUP>/iTunesDB.before"
shasum -a 256 "<BACKUP>/iTunesDB.before"
find "<IPOD>/Playlists" -maxdepth 2 -type f -print -exec shasum -a 256 {} \; > "<BACKUP>/playlists.before.txt"
```

Windows PowerShell:

```powershell
New-Item -ItemType Directory -Force '<BACKUP>' | Out-Null
Copy-Item '<IPOD>\iPod_Control\iTunes\iTunesDB' '<BACKUP>\iTunesDB.before'
Get-FileHash '<BACKUP>\iTunesDB.before' -Algorithm SHA256
Get-ChildItem '<IPOD>\Playlists' -Recurse -File | Get-FileHash -Algorithm SHA256 | Format-Table -AutoSize | Out-File '<BACKUP>\playlists.before.txt'
```

Expected: backup/hash commands succeed. If the device disconnects, the DB is missing, or backup/hash capture fails, stop; do not sync.

- [ ] **Step 4: Run one coordinated Classick sync with Rockbox compatibility enabled.** Use the existing UI setting for the explicitly selected serial, then Sync Now. Keep the source share read-only and the iPod connected through “Finishing sync… / Keep the iPod connected.”

Expected: terminal state is completed only after projection; `/Playlists/Classick` contains one recorded `.m3u8` for the chosen manual playlist and one for the chosen smart playlist; neither file has BOM/host paths/backslashes; ownership hashes match exact file bytes. Before booting either firmware, run Plan 6A's read-only playlist audit and prove Classick's write created no new firmware-system `Videos` record.

- [ ] **Step 5: Verify Apple and Rockbox playback equivalence physically.** Eject through Classick. Boot Apple firmware, open both playlists, record displayed order, and play the first/middle/last track. Reconnect/eject without mutation, boot Rockbox, load both files through its file/playlist catalogue, record order, and play the same first/middle/last tracks.

Expected: manual and smart order exactly matches between firmwares; all sampled tracks play; Rockbox requires no tag-database rebuild. Any `Videos` change observed only after Apple-firmware boot is recorded against Plan 6A's physical causality sequence and is not attributed to Classick by this gate.

- [ ] **Step 6: Verify rename, unsubscribe, toggle-off, and foreign preservation physically.** Before mutation, place one hand-authored `Handmade.m3u8` in `/Playlists/Classick` and record its SHA-256. Rename one Classick playlist, sync, and verify the new file works and old recorded file is gone. Unsubscribe it, sync, and verify its Apple and recorded Rockbox representations are gone. Disable Rockbox compatibility, sync, and verify remaining recorded projections are gone while Apple playlists remain. Re-hash `Handmade.m3u8` after every run.

Expected: the foreign file hash never changes; no unrecorded file is deleted; every mutation reaches completed only after required finalization; both firmwares still boot and play tracks.

- [ ] **Step 7: Add one concise, non-duplicate learning and rerun the final test.**

Add this bullet under a dated playlist-delivery heading in `LEARNINGS.md`:

```markdown
- Rockbox playlist files are managed only through the device-authoritative filename/hash record plus a surviving checkpoint journal; projection uses the same verified ordered device paths as the Apple playlist, and unrecorded files under `/Playlists/Classick` are always foreign.
```

Run: `cargo test -p classick --test rockbox_projection_integration -- --test-threads=1`

Expected: PASS after the documentation-only change.

- [ ] **Step 8: Commit the learning.**

```bash
git add LEARNINGS.md
git commit -m "docs(playlist): record Rockbox ownership invariant"
```

## Completion Checklist

- [ ] Plan 3 checkpoint tests and Plan 6A playlist-integrity tests are GREEN before 6B execution.
- [ ] Exact 6A DTO/store/membership names in this plan match implemented code.
- [ ] UTF-8/no-BOM, absolute slash path, same-order, empty-file, hash, and invalid-path tests pass.
- [ ] FAT-safe stable filename, same-name, deterministic collision, and foreign collision tests pass.
- [ ] Every write is sibling-temp + fsync + atomic rename; every delete is exact-recorded and directory-contained.
- [ ] Rename, unsubscribe, toggle-off, failed delete retry, unplug recovery, and symlink/traversal tests pass.
- [ ] Recovery resumes from `PlaylistOwnershipPublished` without rerunning reconcile or changing Apple playlist IDs.
- [ ] Apple/Rockbox logical membership equivalence passes in integration tests and on the physical iPod.
- [ ] Full Rust tests/build pass on Windows and macOS; macOS bundle succeeds.
- [ ] Live Rockbox catalogue playback passes without a tag database rewrite; foreign Apple/Rockbox playlists remain unchanged.
