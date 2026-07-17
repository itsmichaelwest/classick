//! Task 8: end-to-end core smoke for the union/reconcile seam.
//!
//! Drives the real seams a sync run touches — `sync_set::compute` (union of
//! scope ∩ walk with subscribed-playlist tracks), the real commit path
//! (`apply_loop::retry_deferred`: real libgpod FFI + a real transcode of the
//! committed `tests/fixtures/tagged.flac` fixture via the system `afconvert`
//! on macOS, never ffmpeg — same fake-mount + hand-rolled iTunesDB pattern as
//! `fit_retry_integration.rs`), `ipod::device_playlists::reconcile_in` (the
//! `_in` test/override variant, per `device_playlists_integration.rs`), and
//! `manifest::diff` — without going through the daemon/IPC layer at all.
//!
//! Scenario: a manual playlist references a track that sits OUTSIDE an
//! Include-scope selection. `sync_set::compute` must still pull it in (the
//! union guarantee); it must land on the device and show up as a reconciled
//! playlist member after a reparse from disk. Unsubscribing then drops the
//! device playlist AND (separately, via `manifest::diff`) plans a Remove for
//! the now-out-of-scope track, pinning the union<->diff interplay.
//!
//! Mirror-write + adopt round-trip (Task 6 §1) is already covered by
//! `device_playlists_integration.rs`'s `adopt_from_ipod_*` tests — not
//! duplicated here.

use classick::apply_loop::{retry_deferred, ArtworkCounts};
use classick::cli::EncoderChoice;
use classick::config::Config;
use classick::device_config::Subscriptions;
use classick::ffi;
use classick::fit::DeferredAlbum;
use classick::ipod::db::OwnedDb;
use classick::ipod::device_playlists::{self, ReconcileStats};
use classick::library_index::{IndexedTrack, LibraryIndex};
use classick::manifest::{Action, Manifest};
use classick::playlist::{ManualPlaylist, Playlist, PlaylistStore};
use classick::progress::Progress;
use classick::selection::{Selection, SelectionMode, SelectionRule};
use classick::source::SourceEntry;
use classick::sync_set;
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
        .join(format!("playlists-e2e-{label}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    base
}

/// Fake iPod mount with the directory structure libgpod expects —
/// `itdb_cp_track_to_ipod` round-robins new files into `F00..F49` and errors
/// ("No 'F..' directories found") rather than creating one itself, so the
/// fake mount needs at least one (LEARNINGS.md).
fn fake_mount() -> PathBuf {
    let base = scratch_dir("mount");
    std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
    let music = base.join("iPod_Control").join("Music");
    std::fs::create_dir_all(music.join("F00")).unwrap();
    base
}

/// Write a real, valid (empty) iTunesDB at `<mount>/iPod_Control/iTunes/iTunesDB`
/// by driving libgpod directly — same approach as the other integration
/// tests. The master playlist is titled "iPod".
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

/// Copy the committed FLAC fixture to `<source_root>/<rel_dir>/track.flac`
/// and stat it back into a `SourceEntry`, same pattern as `fit_retry_integration.rs`'s
/// `make_album` but one track per (distinct) directory, so each track is its
/// own `fit::album_key` group under the directory-based fallback.
fn make_track(source_root: &Path, rel_dir: &str) -> SourceEntry {
    let dir = source_root.join(rel_dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("track.flac");
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
}

fn indexed(mtime: i64, size: u64, artist: &str) -> IndexedTrack {
    IndexedTrack {
        mtime,
        size,
        artist: artist.to_string(),
        album_artist: String::new(),
        album: "Album".to_string(),
        genre: "G".to_string(),
        title: String::new(),
        duration_ms: 0,
        year: None,
    }
}

/// Commit `tracks` to `db`/`manifest` via the real `retry_deferred` path —
/// real libgpod add + real transcode — each track as its own one-track album
/// (matching `make_track`'s one-track-per-directory layout). Mirrors
/// `device_playlists_integration.rs`'s `seed_tracks_with_manifest`, but takes
/// an explicit track list instead of manufacturing one internally.
fn commit_tracks(db: &OwnedDb, manifest: &mut Manifest, tracks: &[SourceEntry]) {
    let deferred: Vec<DeferredAlbum> = tracks
        .iter()
        .map(|t| DeferredAlbum {
            key: t.path.parent().unwrap().to_string_lossy().into_owned(),
            tracks: 1,
            bytes: t.size,
        })
        .collect();

    let config = test_config();
    let refalac_version: Option<String> = None;
    let (progress, decision_rx) = Progress::start(false, false).unwrap();
    let mut bytes_written: u64 = 0;
    let mut artwork_counts = ArtworkCounts::default();
    let total_bytes: u64 = tracks.iter().map(|t| t.size).sum();

    let result = retry_deferred(
        &config,
        &refalac_version,
        db,
        manifest,
        tracks,
        deferred,
        Some(total_bytes * 10), // generous budget: well over what's needed
        |_: &Path| None,
        &progress,
        &decision_rx,
        &mut bytes_written,
        &mut artwork_counts,
    )
    .expect("commit_tracks: retry_deferred should succeed");
    assert!(
        result.is_empty(),
        "no album should still be deferred: {result:?}"
    );
}

/// dbid of the manifest entry whose `source_path` is `path`. Panics if
/// absent — test helper, panic-on-missing is the right failure mode.
fn dbid_for(manifest: &Manifest, path: &Path) -> u64 {
    manifest
        .tracks
        .iter()
        .find(|e| e.source_path == path)
        .unwrap_or_else(|| panic!("no manifest entry for {}", path.display()))
        .ipod_dbid
}

/// Sorted member dbids of the on-device playlist named `name`. Panics if no
/// such playlist exists.
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
    let mut names: Vec<String> = classick::ipod::db::list_playlists(db)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names
}

/// Translate a `sync_set::EffectiveSet::playlist_tracks` entry into the
/// `(name, dbids)` shape `reconcile_in` wants, resolving each resolved
/// absolute path through the manifest.
fn desired_playlists(
    manifest: &Manifest,
    playlist_tracks: &[(String, Vec<PathBuf>)],
) -> Vec<(String, Vec<u64>)> {
    playlist_tracks
        .iter()
        .map(|(name, paths)| {
            (
                name.clone(),
                paths.iter().map(|p| dbid_for(manifest, p)).collect(),
            )
        })
        .collect()
}

/// Full setup shared by both scenarios: a fake mount with a real (empty)
/// iTunesDB, two real on-disk tracks (`Keep/track.flac` in scope, `Skip/
/// track.flac` out of scope), a `LibraryIndex` with matching (mtime, size)
/// so `selection::filter`'s freshness check actually exercises scope
/// filtering rather than failing open, an Include selection matching only
/// "Keep Artist", and a manual playlist "mix" referencing the out-of-scope
/// track by its source-relative path.
struct Fixture {
    mount: PathBuf,
    db: OwnedDb,
    manifest: Manifest,
    source_root: PathBuf,
    keep: SourceEntry,
    skip: SourceEntry,
    index: LibraryIndex,
    selection: Selection,
    store: PlaylistStore,
    state_root: PathBuf,
    serial: String,
}

fn setup() -> Fixture {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();
    let mut manifest = Manifest::empty();

    let source_root = scratch_dir("src");
    let keep = make_track(&source_root, "Keep");
    let skip = make_track(&source_root, "Skip");

    commit_tracks(&db, &mut manifest, std::slice::from_ref(&keep));
    commit_tracks(&db, &mut manifest, std::slice::from_ref(&skip));
    assert_eq!(
        db.track_count(),
        2,
        "sanity: both tracks landed on the device"
    );

    let mut index = LibraryIndex::empty(source_root.clone());
    index.files.insert(
        keep.path.clone(),
        indexed(keep.mtime, keep.size, "Keep Artist"),
    );
    index.files.insert(
        skip.path.clone(),
        indexed(skip.mtime, skip.size, "Skip Artist"),
    );

    let selection = Selection {
        version: 1,
        mode: SelectionMode::Include,
        rules: vec![SelectionRule::Artist {
            name: "Keep Artist".into(),
        }],
    };

    let playlists_root = scratch_dir("playlists");
    let store = PlaylistStore::open(playlists_root).unwrap();
    let manual = ManualPlaylist {
        slug: "mix".to_string(),
        name: "Mix".to_string(),
        tracks: vec![PathBuf::from("Skip/track.flac")],
        skipped_unsafe: 0,
    };
    store.save(&Playlist::Manual(manual)).unwrap();

    let state_root = scratch_dir("state");
    let serial = "0xE2ESERIAL01".to_string();

    Fixture {
        mount,
        db,
        manifest,
        source_root,
        keep,
        skip,
        index,
        selection,
        store,
        state_root,
        serial,
    }
}

/// Scenario 1: a manual playlist referencing a track OUTSIDE the Include
/// scope is still unioned in by `sync_set::compute`; committing + reconciling
/// lands it on the device as a real playlist member.
#[test]
fn playlist_track_outside_scope_syncs_and_reconciles_as_playlist_member() {
    let f = setup();

    let walk = vec![f.keep.clone(), f.skip.clone()];
    let subs = Subscriptions {
        version: 1,
        playlists: vec!["mix".to_string()],
    };

    let effective = sync_set::compute(
        walk,
        &f.selection,
        &subs,
        &f.store,
        &f.index,
        &f.source_root,
    );

    assert!(
        effective.playlist_errors.is_empty(),
        "playlist should resolve cleanly: {:?}",
        effective.playlist_errors
    );
    assert_eq!(effective.missing_playlist_tracks, 0);
    let effective_paths: Vec<PathBuf> = effective.sources.iter().map(|e| e.path.clone()).collect();
    assert!(
        effective_paths.contains(&f.keep.path),
        "in-scope track stays"
    );
    assert!(
        effective_paths.contains(&f.skip.path),
        "playlist track outside Include scope must still be unioned in"
    );
    assert_eq!(
        effective.playlist_tracks,
        vec![("mix".to_string(), vec![f.skip.path.clone()])]
    );

    // The tracks are already committed (setup's `commit_tracks`) — the union
    // just confirmed the out-of-scope track WOULD be kept by a real run, so
    // now reconcile the device-side playlist against the manifest dbids the
    // real commit produced.
    let desired = desired_playlists(&f.manifest, &effective.playlist_tracks);
    let stats = device_playlists::reconcile_in(&f.db, &desired, &f.state_root, &f.serial)
        .expect("reconcile should succeed");
    assert_eq!(
        stats,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );

    f.db.write().expect("db.write after reconcile");
    drop(f.db);

    // Reparse from disk — confirm the reconcile landed for real, not just
    // in the in-memory Itdb_iTunesDB. `sync_set::compute` keys
    // `playlist_tracks` by SLUG (not the playlist's display name), and
    // that's what flows straight into `reconcile_in` here, so the on-device
    // playlist is named "mix", not "Mix".
    let reopened = OwnedDb::open(&f.mount).unwrap();
    assert_eq!(
        sorted_playlist_names(&reopened),
        vec!["iPod".to_string(), "mix".to_string()],
        "managed playlist created alongside the untouched MPL"
    );
    let skip_dbid = dbid_for(&f.manifest, &f.skip.path);
    assert_eq!(
        playlist_member_dbids(&reopened, "mix"),
        vec![skip_dbid],
        "the out-of-scope track is the sole member of the reconciled playlist"
    );
    let mpl_untouched = classick::ipod::db::list_playlists(&reopened)
        .into_iter()
        .any(|(name, is_mpl)| name == "iPod" && is_mpl);
    assert!(mpl_untouched, "master playlist must remain flagged is_mpl");
    assert_eq!(
        reopened.track_count(),
        2,
        "reconcile must never touch track count"
    );
}

/// Scenario 2: unsubscribing drops the device-side playlist via reconcile,
/// and separately, `manifest::diff` against the now-narrower effective set
/// plans a Remove for the track that's no longer in scope AND no longer
/// pulled in by any playlist — pinning the union<->diff interplay. This test
/// only asserts the diff's plan; it doesn't execute the removal.
#[test]
fn unsubscribe_drops_device_playlist_and_diff_plans_track_removal() {
    let f = setup();

    // --- First, reconcile with the playlist subscribed (same as scenario 1)
    // so there's a real on-device playlist to unsubscribe FROM. ---
    let walk = vec![f.keep.clone(), f.skip.clone()];
    let subscribed = Subscriptions {
        version: 1,
        playlists: vec!["mix".to_string()],
    };
    let effective_subscribed = sync_set::compute(
        walk.clone(),
        &f.selection,
        &subscribed,
        &f.store,
        &f.index,
        &f.source_root,
    );
    let desired_subscribed = desired_playlists(&f.manifest, &effective_subscribed.playlist_tracks);
    let stats =
        device_playlists::reconcile_in(&f.db, &desired_subscribed, &f.state_root, &f.serial)
            .unwrap();
    assert_eq!(
        stats,
        ReconcileStats {
            created: 1,
            updated: 0,
            removed: 0
        }
    );
    f.db.write().expect("db.write after first reconcile");

    // --- Unsubscribe: recompute with an empty Subscriptions. The union
    // collapses back to just the Include-scope selection, so the out-of-
    // scope "Skip" track drops out of `effective.sources` entirely. ---
    let unsubscribed = Subscriptions::default();
    let effective_unsubscribed = sync_set::compute(
        walk,
        &f.selection,
        &unsubscribed,
        &f.store,
        &f.index,
        &f.source_root,
    );

    let paths: Vec<PathBuf> = effective_unsubscribed
        .sources
        .iter()
        .map(|e| e.path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![f.keep.path.clone()],
        "unsubscribing drops the out-of-scope track from the union"
    );
    assert!(
        effective_unsubscribed.playlist_tracks.is_empty(),
        "no playlists subscribed anymore"
    );

    // Reconcile with the (now empty) desired playlist set: "mix" must be
    // removed from the device.
    let desired_unsubscribed =
        desired_playlists(&f.manifest, &effective_unsubscribed.playlist_tracks);
    assert!(desired_unsubscribed.is_empty());
    let stats2 =
        device_playlists::reconcile_in(&f.db, &desired_unsubscribed, &f.state_root, &f.serial)
            .unwrap();
    assert_eq!(
        stats2,
        ReconcileStats {
            created: 0,
            updated: 0,
            removed: 1
        },
        "mix should be removed"
    );

    f.db.write().expect("db.write after second reconcile");
    drop(f.db);

    let reopened = OwnedDb::open(&f.mount).unwrap();
    assert_eq!(
        sorted_playlist_names(&reopened),
        vec!["iPod".to_string()],
        "mix gone from the reparsed device, MPL untouched"
    );
    assert_eq!(
        reopened.track_count(),
        2,
        "reconcile alone never removes tracks"
    );

    // --- The track's fate is a SEPARATE question, answered by
    // `manifest::diff` against the narrower effective set: "Skip" is no
    // longer in `effective_unsubscribed.sources` but IS still a manifest
    // entry (from setup's commit), so diff must plan a Remove for it. "Keep"
    // is unchanged (same mtime/size as when committed) and stays Unchanged,
    // never Remove. ---
    let actions = classick::manifest::diff(
        &f.manifest,
        &effective_unsubscribed.sources,
        |_p: &Path| panic!("fast path should cover both entries; fingerprint should not be needed"),
        |_p: &Path| {
            panic!("fast path should cover both entries; audio_fingerprint should not be needed")
        },
        "ffmpeg",
        false,
    )
    .expect("diff should succeed");

    let removed_skip = actions
        .iter()
        .any(|a| matches!(a, Action::Remove(e) if e.source_path == f.skip.path));
    assert!(
        removed_skip,
        "the now-out-of-scope, no-longer-subscribed track must plan a Remove: {actions:?}"
    );
    let keep_removed = actions
        .iter()
        .any(|a| matches!(a, Action::Remove(e) if e.source_path == f.keep.path));
    assert!(
        !keep_removed,
        "the still-in-scope track must never plan a Remove: {actions:?}"
    );
    let keep_unchanged = actions
        .iter()
        .any(|a| matches!(a, Action::Unchanged(e) if e.source_path == f.keep.path));
    assert!(
        keep_unchanged,
        "the still-in-scope, untouched track should be Unchanged: {actions:?}"
    );
}
