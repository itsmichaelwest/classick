//! Integration tests for Task 6: Classick-managed iTunesDB playlist
//! reconcile (`ipod::device_playlists::reconcile` / `reconcile_in`).
//!
//! Builds a real iTunesDB with committed tracks via the existing
//! `retry_deferred` harness (same fake-mount + real-transcode pattern as
//! `wipe_all_tracks_integration.rs` / `fit_retry_integration.rs` — a real
//! transcode via the system `afconvert` on macOS, never ffmpeg on macOS),
//! then drives `reconcile_in` directly against a per-test device-state root
//! (the `_in` test/override variant — see `device_state.rs`'s existing
//! `_in` convention) and reparses the DB from disk after each `db.write()`
//! to confirm the changes actually landed, not just in-memory state.

use classick::apply_loop::{retry_deferred, ArtworkCounts};
use classick::cli::EncoderChoice;
use classick::config::Config;
use classick::device_state;
use classick::ffi;
use classick::fit::DeferredAlbum;
use classick::ipod::db::{ensure_managed_playlist, list_playlists, OwnedDb};
use classick::ipod::device_playlists::{self, ReconcileStats};
use classick::manifest::Manifest;
use classick::manifest_store::ManifestStore;
use classick::progress::Progress;
use classick::source::SourceEntry;
use classick::source_location::{SourceIdentity, SourceLocation};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

const FIXTURE_FLAC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac");

/// Per-test scratch dir under `target/test-tmp/` so tests don't collide.
fn scratch_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("device-playlists-{label}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    base
}

/// Fake iPod mount with the directory structure libgpod expects —
/// `itdb_cp_track_to_ipod` round-robins new files into `F00..F49` and
/// errors ("No 'F..' directories found") rather than creating one itself,
/// so the fake mount needs at least one (LEARNINGS.md).
fn fake_mount() -> PathBuf {
    let base = scratch_dir("mount");
    std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
    let music = base.join("iPod_Control").join("Music");
    std::fs::create_dir_all(music.join("F00")).unwrap();
    base
}

/// Write a real, valid (empty) iTunesDB at `<mount>/iPod_Control/iTunes/iTunesDB`
/// by driving libgpod directly — same approach as the other integration tests.
/// The master playlist is titled "iPod" so tests can assert it by name.
fn write_valid_itunesdb(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
        assert!(!db.is_null(), "itdb_new returned null");

        let mount_c = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(db, mount_c.as_ptr());

        let title = CString::new("iPod").unwrap();
        let mpl = ffi::itdb_playlist_new(title.as_ptr(), 0);
        assert!(!mpl.is_null(), "itdb_playlist_new returned null");
        ffi::itdb_playlist_set_mpl(mpl);
        ffi::itdb_playlist_add(db, mpl, -1);

        let mut err: *mut ffi::GError = ptr::null_mut();
        let ok = ffi::itdb_write(db, &mut err);
        ffi::itdb_free(db);
        assert_ne!(ok, 0, "itdb_write failed generating test fixture");
    }
}

/// One small "album": `track_count` copies of the committed FLAC fixture
/// under a shared parent directory.
fn make_album(source_root: &Path, track_count: usize) -> Vec<SourceEntry> {
    let album_dir = source_root.join("Artist").join("Album");
    std::fs::create_dir_all(&album_dir).unwrap();
    (0..track_count)
        .map(|i| {
            let dst = album_dir.join(format!("track{i}.flac"));
            std::fs::copy(FIXTURE_FLAC, &dst).unwrap();
            let meta = std::fs::metadata(&dst).unwrap();
            SourceEntry {
                path: dst,
                mtime: meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                size: meta.len(),
            }
        })
        .collect()
}

fn album_key_for(tracks: &[SourceEntry]) -> String {
    tracks[0]
        .path
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn test_config() -> Config {
    Config {
        source: PathBuf::from("/nonexistent-source"),
        ipod: None,
        ffmpeg: PathBuf::from("ffmpeg"),
        dry_run: false,
        apply: true,
        no_delete: false,
        verbose: false,
        rebuild_manifest: false,
        use_tui: false,
        manifest_path: PathBuf::from("/nonexistent-manifest.json"),
        save_config: false,
        encoder: EncoderChoice::Ffmpeg,
        refalac_path: PathBuf::from("refalac64"),
        passthrough_wav: false,
        force_reencode: false,
        rockbox_compat: false,
        rockbox_compat_cli_flag: false,
        backfill_rockbox: false,
        scan_library: false,
        restore_db_backup: false,
        replace_library: false,
        verify_artwork: false,
    }
}

/// Seed a real DB with `track_count` committed tracks using the existing
/// `retry_deferred` commit path (real libgpod add + real transcode), the
/// same harness `wipe_all_tracks_integration.rs` uses to get tracks
/// genuinely on-disk rather than hand-faked. Returns the resulting
/// `Manifest` so callers can read back each track's assigned `ipod_dbid`.
fn seed_tracks_with_manifest(db: &OwnedDb, source_root: &Path, track_count: usize) -> Manifest {
    let tracks = make_album(source_root, track_count);
    let total_bytes: u64 = tracks.iter().map(|t| t.size).sum();
    let key = album_key_for(&tracks);
    let deferred = vec![DeferredAlbum {
        key,
        tracks: tracks.len(),
        bytes: total_bytes,
    }];

    let config = test_config();
    let refalac_version: Option<String> = None;
    let mut manifest = Manifest::empty();
    let (progress, decision_rx) = Progress::start(false, false).unwrap();
    let mut bytes_written: u64 = 0;
    let mut artwork_counts = ArtworkCounts::default();

    let result = retry_deferred(
        &config,
        &refalac_version,
        db,
        &mut manifest,
        &tracks,
        deferred,
        Some(total_bytes * 10), // generous budget
        |_: &Path| None,
        &progress,
        &decision_rx,
        &mut bytes_written,
        &mut artwork_counts,
    )
    .expect("seed commit should succeed");
    assert!(
        result.is_empty(),
        "seed album should not be deferred: {result:?}"
    );
    assert_eq!(
        db.track_count(),
        track_count,
        "sanity: all seed tracks committed"
    );
    manifest
}

/// Sorted member dbids of the on-device playlist named `name`. Panics if no
/// such playlist exists — test assertion helper, not production code, so a
/// panic-on-missing is the right failure mode (surfaces immediately as a
/// clear test failure rather than a silent empty Vec).
fn playlist_member_dbids(db: &OwnedDb, name: &str) -> Vec<u64> {
    unsafe {
        let name_c = CString::new(name).unwrap();
        let pl = ffi::itdb_playlist_by_name(db.as_ptr(), name_c.as_ptr() as *mut _);
        assert!(!pl.is_null(), "playlist {name:?} should exist");
        let mut out = Vec::new();
        let mut node = (*pl).members;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            out.push((*t).dbid as u64);
            node = (*node).next;
        }
        out.sort();
        out
    }
}

fn sorted_playlist_names(db: &OwnedDb) -> Vec<String> {
    let mut names: Vec<String> = list_playlists(db)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names
}

/// `(id, is_mpl, sorted member dbids)` for every playlist named exactly
/// `name` — unlike `playlist_member_dbids`, this doesn't assume the name is
/// unique, so it's the right helper once a test has two same-named
/// playlists on purpose (a foreign one Classick must never touch, plus the
/// Classick-managed one it creates alongside it).
fn playlists_named(db: &OwnedDb, name: &str) -> Vec<(u64, bool, Vec<u64>)> {
    unsafe {
        let mut out = Vec::new();
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let pl = (*node).data as *mut ffi::Itdb_Playlist;
            if !pl.is_null() && !(*pl).name.is_null() {
                let pname = CStr::from_ptr((*pl).name).to_string_lossy();
                if pname == name {
                    let mut members = Vec::new();
                    let mut mnode = (*pl).members;
                    while !mnode.is_null() {
                        let t = (*mnode).data as *mut ffi::Itdb_Track;
                        members.push((*t).dbid as u64);
                        mnode = (*mnode).next;
                    }
                    members.sort();
                    out.push(((*pl).id as u64, ffi::itdb_playlist_is_mpl(pl) != 0, members));
                }
            }
            node = (*node).next;
        }
        out
    }
}

/// Create a playlist directly via low-level FFI — no `ensure_managed_playlist`,
/// no managed-record bookkeeping. Simulates a foreign (non-Classick)
/// playlist that happens to share a name with one Classick is about to
/// manage. Returns its itdb id.
fn create_foreign_playlist(db: &OwnedDb, name: &str) -> u64 {
    unsafe {
        let name_c = CString::new(name).unwrap();
        let pl = ffi::itdb_playlist_new(name_c.as_ptr(), 0);
        assert!(!pl.is_null(), "itdb_playlist_new returned null");
        ffi::itdb_playlist_add(db.as_ptr(), pl, -1);
        (*pl).id as u64
    }
}

#[test]
fn reconcile_creates_updates_and_removes_without_touching_foreign_or_mpl() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 3);
    let mut dbids: Vec<u64> = manifest.tracks.iter().map(|e| e.ipod_dbid).collect();
    dbids.sort();
    assert_eq!(dbids.len(), 3, "sanity: three distinct tracks seeded");

    let state_root = scratch_dir("state");
    let serial = "0xTESTSERIAL01";

    // --- Step 1: reconcile with desired = [("gym", "Gym", [dbid0, dbid1])]. ---
    let desired = vec![(
        "gym".to_string(),
        "Gym".to_string(),
        vec![dbids[0], dbids[1]],
    )];
    let stats = device_playlists::reconcile_in(&db, &desired, &state_root, serial)
        .expect("reconcile should succeed");
    assert_eq!(
        stats,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );

    db.write().expect("db.write after first reconcile");
    drop(db);

    // Reparse from disk: "Gym" exists with exactly 2 members, MPL untouched.
    let reopened = OwnedDb::open(&mount).unwrap();
    assert_eq!(
        sorted_playlist_names(&reopened),
        vec!["Gym".to_string(), "iPod".to_string()]
    );
    assert_eq!(
        playlist_member_dbids(&reopened, "Gym"),
        vec![dbids[0], dbids[1]]
    );
    let mpl_is_still_mpl = list_playlists(&reopened)
        .into_iter()
        .any(|(name, is_mpl)| name == "iPod" && is_mpl);
    assert!(
        mpl_is_still_mpl,
        "master playlist should be untouched (still flagged is_mpl)"
    );
    assert_eq!(
        reopened.track_count(),
        3,
        "reconcile must never touch track count"
    );

    // --- Step 2: reconcile again, this time also managing "Foreign" (so it
    // gets created for real via `ensure_playlist` and lands in the managed
    // record), then simulate it turning out NOT to be ours by rewriting the
    // managed-record file directly to drop it — the strongest test of "no
    // name heuristics": even a name reconcile itself just created is
    // untouchable once the record says otherwise. ---
    let desired_with_foreign = vec![
        (
            "gym".to_string(),
            "Gym".to_string(),
            vec![dbids[0], dbids[1]],
        ),
        ("foreign".to_string(), "Foreign".to_string(), vec![dbids[2]]),
    ];
    let stats2 =
        device_playlists::reconcile_in(&reopened, &desired_with_foreign, &state_root, serial)
            .expect("second reconcile should succeed");
    assert_eq!(
        stats2,
        ReconcileStats {
            created: 1,
            updated: 1,
            removed: 0
        }
    );
    reopened.write().expect("db.write after second reconcile");

    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    let recorded = std::fs::read_to_string(&managed_path).unwrap();
    assert!(
        recorded.contains("Foreign"),
        "sanity: Foreign really was recorded as managed"
    );
    // Rewrite the record as if "Foreign" was never ours.
    std::fs::write(&managed_path, r#"{"names":["Gym"]}"#).unwrap();
    drop(reopened);

    // --- Step 3: reconcile with desired = [] (drop "Gym"). "Foreign" must
    // survive — it's on-device (reconcile #2 really created it) but the
    // record (the ONLY source of managed identity) no longer lists it. ---
    let reopened2 = OwnedDb::open(&mount).unwrap();
    let stats3 = device_playlists::reconcile_in(&reopened2, &[], &state_root, serial)
        .expect("third reconcile should succeed");
    assert_eq!(
        stats3,
        ReconcileStats {
            created: 0,
            updated: 0,
            removed: 1
        },
        "only Gym should be removed"
    );

    reopened2.write().expect("db.write after third reconcile");
    drop(reopened2);

    let final_db = OwnedDb::open(&mount).unwrap();
    let names = sorted_playlist_names(&final_db);
    assert_eq!(
        names,
        vec!["Foreign".to_string(), "iPod".to_string()],
        "Gym gone, Foreign + MPL untouched"
    );
    assert_eq!(
        final_db.track_count(),
        3,
        "reconcile must never touch track count"
    );
}

/// Fix 2 regression: `PlaylistStore::unique_slug` lets two distinct
/// playlists share a display name (`gym` and `gym-2`, both titled "Gym").
/// Before Fix 2, `reconcile` keyed the managed record by NAME alone, which
/// `dedup_by(name)` collapsed into a single managed entry — permanently
/// orphaning one on-device playlist and clobbering membership on every
/// subsequent reconcile (last write wins). Keying by slug must create and
/// track both independently, and re-running with the same desired set must
/// be a genuine steady-state no-op — an update for each, never a fresh
/// create, and neither playlist lost or orphaned.
#[test]
fn reconcile_keeps_distinct_slugs_sharing_a_display_name_independent() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-dupname");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 2);
    let dbid0 = manifest.tracks[0].ipod_dbid;
    let dbid1 = manifest.tracks[1].ipod_dbid;

    let state_root = scratch_dir("state-dupname");
    let serial = "0xDUPNAME";

    let desired = vec![
        ("gym".to_string(), "Gym".to_string(), vec![dbid0]),
        ("gym-2".to_string(), "Gym".to_string(), vec![dbid1]),
    ];

    let first = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(
        first,
        ReconcileStats {
            created: 2,
            updated: 0,
            removed: 0
        },
        "both distinctly-slugged playlists must be created"
    );
    db.write().expect("db.write after first reconcile");

    let gyms = playlists_named(&db, "Gym");
    assert_eq!(
        gyms.len(),
        2,
        "two distinct on-device playlists, both named \"Gym\""
    );
    let mut members: Vec<Vec<u64>> = gyms.iter().map(|(_, _, m)| m.clone()).collect();
    members.sort();
    let mut expected = vec![vec![dbid0], vec![dbid1]];
    expected.sort();
    assert_eq!(
        members, expected,
        "each keeps its own distinct membership, never merged"
    );

    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&managed_path).unwrap()).unwrap();
    let slugs: std::collections::BTreeSet<String> = recorded["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["slug"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        slugs,
        std::collections::BTreeSet::from(["gym".to_string(), "gym-2".to_string()]),
        "both slugs recorded as distinct managed entries, not deduped away"
    );

    // Steady-state: same desired set again must update both in place —
    // no clobber (each keeps its own membership), no orphan (neither is
    // dropped or recreated).
    let second = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(
        second,
        ReconcileStats {
            created: 0,
            updated: 2,
            removed: 0
        },
        "steady-state re-run: both update in place, none created or removed"
    );

    let gyms_after = playlists_named(&db, "Gym");
    assert_eq!(
        gyms_after.len(),
        2,
        "no orphan created, no playlist lost on the steady-state run"
    );
    let mut members_after: Vec<Vec<u64>> = gyms_after.iter().map(|(_, _, m)| m.clone()).collect();
    members_after.sort();
    assert_eq!(
        members_after, expected,
        "membership unchanged by the steady-state run"
    );
}

#[test]
fn ensure_managed_playlist_never_adopts_the_master_playlist_by_name() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    // "iPod" is the MPL's title (see `write_valid_itunesdb`). With no
    // recorded id, `ensure_managed_playlist` must never resolve to the MPL
    // by name — instead of erroring OR repurposing the Songs view, it
    // creates a second, distinct playlist under the same name.
    let new_id = ensure_managed_playlist(&db, "iPod", &[], None)
        .expect("a name collision with the MPL must not be an error");

    let mpl_is_still_mpl = list_playlists(&db)
        .into_iter()
        .any(|(name, is_mpl)| name == "iPod" && is_mpl);
    assert!(
        mpl_is_still_mpl,
        "master playlist should remain flagged is_mpl"
    );

    let ipod_named = playlists_named(&db, "iPod");
    assert_eq!(
        ipod_named.len(),
        2,
        "expected the MPL plus the newly created playlist"
    );
    let mpl_entry = ipod_named
        .iter()
        .find(|(_, is_mpl, _)| *is_mpl)
        .expect("MPL entry present");
    assert_ne!(
        mpl_entry.0, new_id,
        "the new playlist must be a distinct id from the MPL"
    );
    assert!(
        ipod_named
            .iter()
            .any(|(id, is_mpl, _)| *id == new_id && !is_mpl),
        "the new non-MPL playlist should be present with the returned id"
    );
}

#[test]
fn reconcile_managed_record_is_stable_across_a_no_op_run() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-noop");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 1);
    let dbid = manifest.tracks[0].ipod_dbid;

    let state_root = scratch_dir("state-noop");
    let serial = "0xNOOP";
    let desired = vec![("solo".to_string(), "Solo".to_string(), vec![dbid])];

    let first = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(
        first,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );

    // Re-running with the SAME desired set should update (idempotent
    // membership rewrite), never re-create or remove.
    let second = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(
        second,
        ReconcileStats {
            created: 0,
            updated: 1,
            removed: 0
        }
    );

    assert_eq!(playlist_member_dbids(&db, "Solo"), vec![dbid]);
}

/// The core regression test for the by-id fix: a FOREIGN playlist (created
/// directly via FFI, never recorded by Classick) shares its name with a
/// playlist Classick is about to manage. Before this fix, `ensure_playlist`
/// resolved by `itdb_playlist_by_name` and would have adopted — and
/// cleared/rewritten the membership of — the foreign playlist. After the
/// fix, `reconcile` never even looks the name up: with no recorded id for
/// "Gym", `ensure_managed_playlist` unconditionally creates a NEW playlist,
/// leaving the foreign one byte-for-byte as it was.
#[test]
fn reconcile_never_adopts_a_foreign_playlist_with_a_colliding_name() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-foreign");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 1);
    let dbid = manifest.tracks[0].ipod_dbid;

    // Foreign "Gym", zero members, never recorded — exactly the
    // "user's own playlist happens to share a name" scenario.
    let foreign_id = create_foreign_playlist(&db, "Gym");

    let state_root = scratch_dir("state-foreign");
    let serial = "0xFOREIGN";
    let desired = vec![("gym".to_string(), "Gym".to_string(), vec![dbid])];
    let stats = device_playlists::reconcile_in(&db, &desired, &state_root, serial)
        .expect("reconcile should succeed");
    assert_eq!(
        stats,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );

    db.write().expect("db.write after reconcile");
    drop(db);

    let reopened = OwnedDb::open(&mount).unwrap();
    let gyms = playlists_named(&reopened, "Gym");
    assert_eq!(
        gyms.len(),
        2,
        "the foreign Gym and Classick's new Gym should both exist"
    );

    let foreign = gyms
        .iter()
        .find(|(id, _, _)| *id == foreign_id)
        .expect("the original foreign playlist must still exist, under its original id");
    assert!(
        foreign.2.is_empty(),
        "foreign playlist's original (empty) membership must be completely untouched"
    );

    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&managed_path).unwrap()).unwrap();
    let recorded_id = recorded["names"][0]["id"]
        .as_u64()
        .expect("recorded id present");
    assert_ne!(
        recorded_id, foreign_id,
        "the recorded id must be the NEW playlist, not the foreign one"
    );

    let managed = gyms
        .iter()
        .find(|(id, _, _)| *id == recorded_id)
        .expect("the recorded id should resolve to a \"Gym\" playlist on-device");
    assert_eq!(
        managed.2,
        vec![dbid],
        "the new managed Gym should carry the desired membership"
    );
}

/// Once a managed playlist's id is known, changing its desired display name
/// (SAME slug — Fix 2's identity key) must rename the SAME on-device
/// playlist in place rather than creating a new one and orphaning the old
/// — same id, new name. No manual record surgery needed: `reconcile_at`
/// finds the previous id by matching `slug`, independent of `name`.
#[test]
fn reconcile_renames_in_place_when_recorded_id_still_resolves() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-rename");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 1);
    let dbid = manifest.tracks[0].ipod_dbid;

    let state_root = scratch_dir("state-rename");
    let serial = "0xRENAME";

    let desired = vec![("gym".to_string(), "Gym".to_string(), vec![dbid])];
    let first = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(
        first,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );
    db.write().expect("db.write after first reconcile");
    drop(db);

    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&managed_path).unwrap()).unwrap();
    let original_id = recorded["names"][0]["id"]
        .as_u64()
        .expect("recorded id present");
    assert_eq!(recorded["names"][0]["slug"].as_str(), Some("gym"));

    // Same slug, new display name — the rename signal.
    let reopened = OwnedDb::open(&mount).unwrap();
    let desired_renamed = vec![("gym".to_string(), "GymRenamed".to_string(), vec![dbid])];
    let second =
        device_playlists::reconcile_in(&reopened, &desired_renamed, &state_root, serial).unwrap();
    assert_eq!(
        second,
        ReconcileStats {
            created: 0,
            updated: 1,
            removed: 0
        },
        "a rename via the same slug must be an update, not a create"
    );

    reopened.write().expect("db.write after second reconcile");
    drop(reopened);

    let final_db = OwnedDb::open(&mount).unwrap();
    assert_eq!(
        sorted_playlist_names(&final_db),
        vec!["GymRenamed".to_string(), "iPod".to_string()],
        "old name gone, new name present, no duplicate"
    );
    let renamed = playlists_named(&final_db, "GymRenamed");
    assert_eq!(renamed.len(), 1);
    assert_eq!(renamed[0].0, original_id, "same itdb id across the rename");
    assert_eq!(renamed[0].2, vec![dbid]);
}

/// A pre-migration `managed_playlists.json` entry (bare name string, no
/// `id`) must still deserialize and still drive removal correctly via the
/// name-based fallback (`remove_playlist_by_name`), even though it's never
/// eligible for the by-id reuse path on the create/update side.
#[test]
fn reconcile_removes_a_legacy_name_only_recorded_playlist() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-legacy");
    let manifest = seed_tracks_with_manifest(&db, &source_root, 1);
    let dbid = manifest.tracks[0].ipod_dbid;

    // Create "Gym" for real (so it's genuinely on-device), independent of
    // any managed-record bookkeeping.
    ensure_managed_playlist(&db, "Gym", &[dbid], None).expect("create Gym");
    db.write().expect("db.write");
    drop(db);

    let state_root = scratch_dir("state-legacy");
    let serial = "0xLEGACY";
    // Hand-write a pre-migration name-only managed record for it, as if
    // this device's managed_playlists.json predates the id field.
    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    std::fs::write(&managed_path, br#"{"names":["Gym"]}"#).unwrap();

    let reopened = OwnedDb::open(&mount).unwrap();
    // desired = [] drops "Gym"; the legacy record has no id, so removal
    // must fall back to `remove_playlist_by_name`.
    let stats = device_playlists::reconcile_in(&reopened, &[], &state_root, serial).unwrap();
    assert_eq!(
        stats,
        ReconcileStats {
            created: 0,
            updated: 0,
            removed: 1
        }
    );

    reopened.write().expect("db.write after reconcile");
    drop(reopened);

    let final_db = OwnedDb::open(&mount).unwrap();
    assert_eq!(
        sorted_playlist_names(&final_db),
        vec!["iPod".to_string()],
        "Gym removed via the legacy name-based fallback"
    );
}

/// Populate `<mount>/iPod_Control/classick/playlists/` (the on-device
/// mirror `adopt_from_ipod` reads from) with one `.m3u8` file plus a
/// `subscriptions.json`, simulating a prior machine/install having synced
/// and mirrored playlists to this iPod.
fn seed_device_mirror(mount: &Path) {
    let mirror_dir = mount
        .join("iPod_Control")
        .join("classick")
        .join("playlists");
    std::fs::create_dir_all(&mirror_dir).unwrap();
    std::fs::write(mirror_dir.join("gym.m3u8"), b"#EXTM3U\n").unwrap();
    std::fs::write(
        mirror_dir.join("subscriptions.json"),
        br#"{"playlists":["gym"]}"#,
    )
    .unwrap();
}

#[test]
fn adopt_from_ipod_adopts_when_both_local_artifacts_are_absent() {
    let mount = fake_mount();
    seed_device_mirror(&mount);

    let host_root = scratch_dir("adopt-empty-host");
    let playlists_root = host_root.join("playlists");
    let subscriptions_path = host_root.join("subscriptions.json");
    assert!(!playlists_root.exists());
    assert!(!subscriptions_path.exists());

    let adopted = device_playlists::adopt_from_ipod(&mount, &playlists_root, &subscriptions_path);
    assert_eq!(adopted, 1, "the one mirrored .m3u8 should be adopted");
    assert!(playlists_root.join("gym.m3u8").exists());
    assert!(
        subscriptions_path.exists(),
        "subscriptions.json should also be adopted"
    );
    assert_eq!(
        std::fs::read(&subscriptions_path).unwrap(),
        std::fs::read(
            mount
                .join("iPod_Control")
                .join("classick")
                .join("playlists")
                .join("subscriptions.json")
        )
        .unwrap()
    );
}

/// The gate fix under test: a pre-existing LOCAL `subscriptions.json` must
/// block adoption entirely, even though `playlists_root` itself is empty —
/// "host emptiness" requires BOTH artifacts absent, and adoption must never
/// overwrite an existing subscriptions.json. Per the documented choice in
/// `adopt_from_ipod`'s doc comment (simplest correct: if EITHER local
/// artifact exists, adopt nothing), the mirrored playlist file must also be
/// left un-adopted, not just the subscriptions file left untouched.
#[test]
fn adopt_from_ipod_skips_when_local_subscriptions_json_already_exists() {
    let mount = fake_mount();
    seed_device_mirror(&mount);

    let host_root = scratch_dir("adopt-existing-subs");
    let playlists_root = host_root.join("playlists"); // left empty/absent on purpose
    let subscriptions_path = host_root.join("subscriptions.json");
    std::fs::create_dir_all(&host_root).unwrap();
    let original_subs = br#"{"playlists":["road-trip"]}"#;
    std::fs::write(&subscriptions_path, original_subs).unwrap();

    let adopted = device_playlists::adopt_from_ipod(&mount, &playlists_root, &subscriptions_path);
    assert_eq!(
        adopted, 0,
        "existing local subscriptions.json must block adoption entirely"
    );
    assert!(
        !playlists_root.join("gym.m3u8").exists(),
        "playlists must not be adopted either, per the EITHER-artifact-exists gate"
    );
    assert_eq!(
        std::fs::read(&subscriptions_path).unwrap(),
        original_subs,
        "the existing local subscriptions.json must be byte-for-byte untouched"
    );
}

#[test]
fn device_authority_remains_serial_correct_for_followup_consumers() {
    let source = SourceLocation {
        resolved_path: scratch_dir("serial-source"),
        identity: SourceIdentity::Local {
            library_id: "serial-library".into(),
        },
    };
    let root = scratch_dir("serial-state");
    let mount_a = scratch_dir("serial-mount-a");
    let mount_b = scratch_dir("serial-mount-b");
    let store_a = ManifestStore::new(
        mount_a,
        "SERIAL-A".into(),
        root.join("devices/SERIAL-A/manifest.json"),
        root.join("manifest.json"),
        classick::atomic_file::AtomicFileWriter::new(),
    );
    let store_b = ManifestStore::new(
        mount_b,
        "SERIAL-B".into(),
        root.join("devices/SERIAL-B/manifest.json"),
        root.join("manifest.json"),
        classick::atomic_file::AtomicFileWriter::new(),
    );
    let mut manifest_a = Manifest::empty();
    manifest_a.ipod_serial = Some("SERIAL-A".into());
    let mut manifest_b = Manifest::empty();
    manifest_b.ipod_serial = Some("SERIAL-B".into());
    store_a.publish(&manifest_a, &source).unwrap();
    store_b.publish(&manifest_b, &source).unwrap();

    assert_eq!(
        store_a
            .load(&source)
            .unwrap()
            .manifest
            .ipod_serial
            .as_deref(),
        Some("SERIAL-A")
    );
    assert_eq!(
        store_b
            .load(&source)
            .unwrap()
            .manifest
            .ipod_serial
            .as_deref(),
        Some("SERIAL-B")
    );
}
