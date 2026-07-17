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
use classick::ipod::db::{ensure_playlist, list_playlists, OwnedDb};
use classick::ipod::device_playlists::{self, ReconcileStats};
use classick::manifest::Manifest;
use classick::progress::Progress;
use classick::source::SourceEntry;
use std::ffi::CString;
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
    tracks[0].path.parent().unwrap().to_string_lossy().into_owned()
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
    let deferred = vec![DeferredAlbum { key, tracks: tracks.len(), bytes: total_bytes }];

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
    assert!(result.is_empty(), "seed album should not be deferred: {result:?}");
    assert_eq!(db.track_count(), track_count, "sanity: all seed tracks committed");
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
    let mut names: Vec<String> = list_playlists(db).into_iter().map(|(name, _)| name).collect();
    names.sort();
    names
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

    // --- Step 1: reconcile with desired = [("Gym", [dbid0, dbid1])]. ---
    let desired = vec![("Gym".to_string(), vec![dbids[0], dbids[1]])];
    let stats = device_playlists::reconcile_in(&db, &desired, &state_root, serial)
        .expect("reconcile should succeed");
    assert_eq!(stats, ReconcileStats { created: 1, updated: 0, removed: 0 });

    db.write().expect("db.write after first reconcile");
    drop(db);

    // Reparse from disk: "Gym" exists with exactly 2 members, MPL untouched.
    let reopened = OwnedDb::open(&mount).unwrap();
    assert_eq!(sorted_playlist_names(&reopened), vec!["Gym".to_string(), "iPod".to_string()]);
    assert_eq!(playlist_member_dbids(&reopened, "Gym"), vec![dbids[0], dbids[1]]);
    let mpl_is_still_mpl = list_playlists(&reopened)
        .into_iter()
        .any(|(name, is_mpl)| name == "iPod" && is_mpl);
    assert!(mpl_is_still_mpl, "master playlist should be untouched (still flagged is_mpl)");
    assert_eq!(reopened.track_count(), 3, "reconcile must never touch track count");

    // --- Step 2: reconcile again, this time also managing "Foreign" (so it
    // gets created for real via `ensure_playlist` and lands in the managed
    // record), then simulate it turning out NOT to be ours by rewriting the
    // managed-record file directly to drop it — the strongest test of "no
    // name heuristics": even a name reconcile itself just created is
    // untouchable once the record says otherwise. ---
    let desired_with_foreign = vec![
        ("Gym".to_string(), vec![dbids[0], dbids[1]]),
        ("Foreign".to_string(), vec![dbids[2]]),
    ];
    let stats2 = device_playlists::reconcile_in(&reopened, &desired_with_foreign, &state_root, serial)
        .expect("second reconcile should succeed");
    assert_eq!(stats2, ReconcileStats { created: 1, updated: 1, removed: 0 });
    reopened.write().expect("db.write after second reconcile");

    let managed_path = device_state::managed_playlists_path_in(&state_root, serial).unwrap();
    let recorded = std::fs::read_to_string(&managed_path).unwrap();
    assert!(recorded.contains("Foreign"), "sanity: Foreign really was recorded as managed");
    // Rewrite the record as if "Foreign" was never ours.
    std::fs::write(&managed_path, r#"{"names":["Gym"]}"#).unwrap();
    drop(reopened);

    // --- Step 3: reconcile with desired = [] (drop "Gym"). "Foreign" must
    // survive — it's on-device (reconcile #2 really created it) but the
    // record (the ONLY source of managed identity) no longer lists it. ---
    let reopened2 = OwnedDb::open(&mount).unwrap();
    let stats3 = device_playlists::reconcile_in(&reopened2, &[], &state_root, serial)
        .expect("third reconcile should succeed");
    assert_eq!(stats3, ReconcileStats { created: 0, updated: 0, removed: 1 }, "only Gym should be removed");

    reopened2.write().expect("db.write after third reconcile");
    drop(reopened2);

    let final_db = OwnedDb::open(&mount).unwrap();
    let names = sorted_playlist_names(&final_db);
    assert_eq!(
        names,
        vec!["Foreign".to_string(), "iPod".to_string()],
        "Gym gone, Foreign + MPL untouched"
    );
    assert_eq!(final_db.track_count(), 3, "reconcile must never touch track count");
}

#[test]
fn ensure_playlist_never_overwrites_the_master_playlist() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    // "iPod" is the MPL's title (see `write_valid_itunesdb`). Asking
    // `ensure_playlist` to manage a playlist under that exact name must
    // refuse rather than silently repurpose the Songs view.
    let result = ensure_playlist(&db, "iPod", &[]);
    assert!(result.is_err(), "ensure_playlist must refuse to touch the master playlist");
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
    let desired = vec![("Solo".to_string(), vec![dbid])];

    let first = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(first, ReconcileStats { created: 1, updated: 0, removed: 0 });

    // Re-running with the SAME desired set should update (idempotent
    // membership rewrite), never re-create or remove.
    let second = device_playlists::reconcile_in(&db, &desired, &state_root, serial).unwrap();
    assert_eq!(second, ReconcileStats { created: 0, updated: 1, removed: 0 });

    assert_eq!(playlist_member_dbids(&db, "Solo"), vec![dbid]);
}
