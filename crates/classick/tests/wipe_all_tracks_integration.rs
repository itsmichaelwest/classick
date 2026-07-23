//! Integration test for Task 11's `wipe_all_tracks`
//! (`crates/classick/src/ipod/db.rs`), the destructive core of
//! `--replace-library`.
//!
//! Builds a real iTunesDB with 2 committed tracks via the existing
//! `retry_deferred` harness (same fake-mount + real-transcode pattern as
//! `fit_retry_integration.rs` / `auto_restore_integration.rs` — a real
//! transcode via the system `afconvert` on macOS, never ffmpeg on macOS),
//! then wipes it and verifies: the reported removed count, the in-memory
//! post-wipe track count, that the on-disk audio files are gone, and that a
//! fresh reparse from disk also shows zero tracks.

use classick::apply_loop::{build_replace_journal, retry_deferred, ArtworkCounts};
use classick::cli::EncoderChoice;
use classick::config::Config;
use classick::device_coordination::DeviceMutationSession;
use classick::ffi;
use classick::fit::DeferredAlbum;
use classick::ipod::db::{wipe_all_tracks, OwnedDb};
use classick::manifest::Manifest;
use classick::manifest_store::ManifestStore;
use classick::progress::Progress;
use classick::source::SourceEntry;
use classick::source_location::{SourceIdentity, SourceLocation};
use classick::sync_transaction::{CheckpointCoordinator, PlaylistFailurePoint, PublishOptions};
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
        .join(format!("wipe-all-{label}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    base
}

/// Fake iPod mount with the directory structure libgpod expects —
/// `itdb_cp_track_to_ipod` round-robins new files into `F00..F49` and
/// errors ("No 'F..' directories found") rather than creating one itself,
/// so the fake mount needs at least one.
fn fake_mount() -> PathBuf {
    let base = scratch_dir("mount");
    std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
    let music = base.join("iPod_Control").join("Music");
    std::fs::create_dir_all(music.join("F00")).unwrap();
    base
}

/// Write a real, valid (empty) iTunesDB at `<mount>/iPod_Control/iTunes/iTunesDB`
/// by driving libgpod directly — same approach as the other integration tests.
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
        transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
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
/// same harness `fit_retry_integration.rs` uses to get tracks genuinely
/// on-disk rather than hand-faked.
fn seed_tracks(db: &OwnedDb, source_root: &Path, track_count: usize) {
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
}

#[test]
fn wipe_all_tracks_removes_every_track_and_its_file() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src");
    seed_tracks(&db, &source_root, 2);

    let music_f00 = mount.join("iPod_Control").join("Music").join("F00");
    let files_before = std::fs::read_dir(&music_f00).unwrap().count();
    assert_eq!(
        files_before, 2,
        "sanity: both audio files landed on disk before wipe"
    );

    let removed = wipe_all_tracks(&db).expect("wipe_all_tracks should succeed");
    assert_eq!(
        removed, 2,
        "wipe_all_tracks should report the pre-wipe track count"
    );
    assert_eq!(
        db.track_count(),
        0,
        "in-memory DB should have zero tracks after wipe"
    );

    let files_after = std::fs::read_dir(&music_f00).unwrap().count();
    assert_eq!(files_after, 0, "audio files should be deleted from disk");

    db.write().unwrap();
    drop(db);
    let reopened = OwnedDb::open(&mount).unwrap();
    assert_eq!(
        reopened.track_count(),
        0,
        "reparsed DB should show zero tracks after wipe + write"
    );
}

#[test]
fn wipe_all_tracks_on_empty_db_is_a_noop() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();
    assert_eq!(db.track_count(), 0);

    let removed = wipe_all_tracks(&db).expect("wipe_all_tracks on an empty DB should succeed");
    assert_eq!(removed, 0);
    assert_eq!(db.track_count(), 0);
}

#[test]
fn wipe_all_tracks_clears_normal_and_smart_memberships() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();
    let source_root = scratch_dir("playlist-members");
    seed_tracks(&db, &source_root, 2);

    unsafe {
        for (name, smart) in [("Foreign", 0), ("Foreign Smart", 1)] {
            let name = CString::new(name).unwrap();
            let playlist = ffi::itdb_playlist_new(name.as_ptr(), smart);
            assert!(!playlist.is_null());
            ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
            let mut node = (*db.as_ptr()).tracks;
            while !node.is_null() {
                let track = (*node).data as *mut ffi::Itdb_Track;
                assert!(!track.is_null());
                ffi::itdb_playlist_add_track(playlist, track, -1);
                node = (*node).next;
            }
        }
    }

    assert_eq!(wipe_all_tracks(&db).unwrap(), 2);
    db.write().unwrap();
    drop(db);
    let reopened = OwnedDb::open(&mount).unwrap();
    assert_eq!(playlist_member_count(&reopened, "Foreign"), 0);
    assert_eq!(playlist_member_count(&reopened, "Foreign Smart"), 0);
}

fn playlist_member_count(db: &OwnedDb, name: &str) -> usize {
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null()
                && !(*playlist).name.is_null()
                && std::ffi::CStr::from_ptr((*playlist).name).to_string_lossy() == name
            {
                return ffi::itdb_playlist_tracks_number(playlist) as usize;
            }
            node = (*node).next;
        }
    }
    panic!("playlist {name:?} not found")
}

#[test]
fn replace_commit_writes_empty_db_before_empty_authoritative_manifest() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();
    let source_root = scratch_dir("replace-source");
    seed_tracks(&db, &source_root, 2);
    db.write().unwrap();
    let source = SourceLocation {
        resolved_path: source_root,
        identity: SourceIdentity::Local {
            library_id: "replace-library".into(),
        },
    };
    let serial = "000A27002138B0A8";
    let store = ManifestStore::new(
        mount.clone(),
        serial.into(),
        scratch_dir("replace-cache").join("manifest.json"),
        scratch_dir("replace-legacy").join("manifest.json"),
        classick::atomic_file::AtomicFileWriter::new(),
    );
    let handles = db.list_tracks_for_rebuild();
    drop(db);
    let device_id = classick::device::DeviceId::parse(serial).unwrap();
    let mutation_session = DeviceMutationSession::acquire(&mount, device_id).unwrap();
    let cache = classick::artwork_cache::ArtworkCache::new(scratch_dir("replace-artwork"));
    let coordinator = CheckpointCoordinator {
        mount: &mount,
        serial,
        mutation_session: &mutation_session,
        manifest_store: &store,
        artwork_cache: cache,
    };
    let mut journal = build_replace_journal(&mount, serial, 77, handles);
    let mut empty = Manifest::empty();
    empty.ipod_serial = Some(serial.into());
    empty.last_source_root = Some(source.resolved_path.clone());
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator
        .publish_with_options(
            &mut journal,
            &mut empty,
            &progress,
            PublishOptions {
                playlist_failure_point: Some(PlaylistFailurePoint::BeforeDatabaseWrite),
                ..PublishOptions::default()
            },
        )
        .unwrap_err();

    assert_eq!(OwnedDb::open(&mount).unwrap().track_count(), 2);
    assert_eq!(
        std::fs::read_dir(mount.join("iPod_Control/Music/F00"))
            .unwrap()
            .count(),
        2
    );
    assert!(classick::pending_session::PendingSessionStore::new(&mount)
        .path(77)
        .exists());

    coordinator
        .recover_pending_with_options(&mut empty, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(OwnedDb::open(&mount).unwrap().track_count(), 0);
    assert_eq!(
        std::fs::read_dir(mount.join("iPod_Control/Music/F00"))
            .unwrap()
            .count(),
        0
    );
    let loaded = store.load(&source).unwrap();
    assert!(loaded.manifest.tracks.is_empty());
    assert_eq!(loaded.manifest.ipod_serial.as_deref(), Some(serial));
    assert!(!classick::pending_session::has_sync_transaction_material(&mount).unwrap());
}
