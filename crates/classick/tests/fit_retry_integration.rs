//! Integration tests for Task 8's end-of-run deferred-album retry
//! (`apply_loop::retry_deferred`). Exercises the real commit path — libgpod
//! FFI + a real transcode (system `afconvert` on macOS, precedented in
//! `transcode.rs`'s own tests — never ffmpeg on macOS) of the committed
//! `tests/fixtures/tagged.flac` fixture — against a fake mount + hand-rolled
//! iTunesDB, following the pattern in `auto_restore_integration.rs`.
//!
//! `retry_deferred` takes its size budget as a plain `Option<u64>` parameter
//! rather than querying `free_space` internally (see its doc comment), so it
//! can be driven here with an arbitrary budget without needing a real
//! mounted device whose free space we could control.

use classick::apply_loop::{retry_deferred, ArtworkCounts};
use classick::cli::EncoderChoice;
use classick::config::Config;
use classick::ffi;
use classick::fit::DeferredAlbum;
use classick::ipod::db::OwnedDb;
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
        .join(format!("fit-retry-{label}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    base
}

/// Fake iPod mount with the directory structure libgpod expects. A real
/// device arrives from Apple's factory Restore with `iPod_Control/Music`
/// pre-populated with `F00`..`F49` hashed subdirectories that
/// `itdb_cp_track_to_ipod` round-robins new files into (it errors — "No
/// 'F..' directories found" — rather than creating one itself), so the fake
/// mount needs at least one.
fn fake_mount() -> PathBuf {
    let base = scratch_dir("mount");
    std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
    let music = base.join("iPod_Control").join("Music");
    std::fs::create_dir_all(music.join("F00")).unwrap();
    base
}

/// Write a real, valid (empty) iTunesDB at `<mount>/iPod_Control/iTunes/iTunesDB`
/// by driving libgpod directly — same approach as `auto_restore_integration.rs`.
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
/// under a shared parent directory, so `fit::album_key`'s directory-based
/// fallback (these tests pass an `album_tag_of` that always returns `None`,
/// same as a run with no library index) groups them together.
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
        backfill_rockbox: false,
        scan_library: false,
        restore_db_backup: false,
        replace_library: false,
    }
}

#[test]
fn retry_deferred_commits_album_when_budget_is_sufficient() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();
    assert_eq!(db.track_count(), 0);

    let source_root = scratch_dir("src-fit");
    let tracks = make_album(&source_root, 2);
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
        &db,
        &mut manifest,
        &tracks,
        deferred,
        Some(total_bytes * 10), // generous budget: well over what's needed
        |_: &Path| None,
        &progress,
        &decision_rx,
        &mut bytes_written,
        &mut artwork_counts,
    )
    .expect("retry_deferred should succeed");

    assert!(result.is_empty(), "album should no longer be deferred: {result:?}");
    assert_eq!(manifest.tracks.len(), 2, "both tracks should land in the manifest");
    assert_eq!(db.track_count(), 2, "both tracks should land in the in-memory DB");
    assert!(bytes_written > 0, "bytes_written tally should reflect the committed audio");

    // Reparse from disk to confirm the commit is real, not just in-memory.
    db.write().unwrap();
    drop(db);
    let reopened = OwnedDb::open(&mount).unwrap();
    assert_eq!(reopened.track_count(), 2, "tracks should persist across a reparse");
}

#[test]
fn retry_deferred_leaves_album_deferred_when_budget_is_insufficient() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    let db = OwnedDb::open(&mount).unwrap();

    let source_root = scratch_dir("src-nofit");
    let tracks = make_album(&source_root, 2);
    let total_bytes: u64 = tracks.iter().map(|t| t.size).sum();
    let key = album_key_for(&tracks);

    let deferred = vec![DeferredAlbum { key: key.clone(), tracks: tracks.len(), bytes: total_bytes }];

    let config = test_config();
    let refalac_version: Option<String> = None;
    let mut manifest = Manifest::empty();
    let (progress, decision_rx) = Progress::start(false, false).unwrap();
    let mut bytes_written: u64 = 0;
    let mut artwork_counts = ArtworkCounts::default();

    let result = retry_deferred(
        &config,
        &refalac_version,
        &db,
        &mut manifest,
        &tracks,
        deferred,
        Some(0), // no budget at all
        |_: &Path| None,
        &progress,
        &decision_rx,
        &mut bytes_written,
        &mut artwork_counts,
    )
    .expect("retry_deferred should succeed (a deferral is not an error)");

    assert_eq!(result.len(), 1, "album should still be reported as deferred");
    assert_eq!(result[0].key, key);
    assert_eq!(result[0].tracks, 2);
    assert_eq!(result[0].bytes, total_bytes);
    assert!(manifest.tracks.is_empty(), "nothing should have been committed to the manifest");
    assert_eq!(db.track_count(), 0, "nothing should have been added to the DB");
    assert_eq!(bytes_written, 0, "bytes_written should be untouched");
}
