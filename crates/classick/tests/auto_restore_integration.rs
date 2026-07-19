//! Integration tests for Task 4: `restore_itunesdb_from_backup` +
//! `open_with_auto_restore`. Cross-platform (macOS-first — this repo has
//! no CI, iPod work is validated on a real Mac).
//!
//! There is no committed iTunesDB fixture. Instead, each test generates a
//! real, valid iTunesDB from scratch by driving libgpod directly
//! (`itdb_new` + `itdb_set_mountpoint` + a master playlist + `itdb_write`)
//! against a fake mount dir. This was validated by hand (see task report)
//! to round-trip through `itdb_parse_file` cleanly, so it's a faithful
//! stand-in for a real device's DB without shipping a binary blob that's
//! tied to a specific libgpod build.

use classick::ffi;
use classick::ipod::db::{
    open_with_auto_restore, restore_itunesdb_from_backup, ITUNESDB_CORRUPT_ASIDE_NAME,
};
use classick::ipod::layout::itunes_db_path;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

/// Per-test fake mount under `target/test-tmp/` so tests don't collide.
fn fake_mount() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-tmp")
        .join(format!("auto-restore-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
    std::fs::create_dir_all(base.join("iPod_Control").join("Music")).unwrap();
    base
}

/// Write a real, valid iTunesDB at `<mount>/iPod_Control/iTunes/iTunesDB`
/// by driving libgpod directly. Panics on any libgpod failure — this is
/// test setup, not the code under test.
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

fn backup_path(mount: &Path) -> PathBuf {
    mount
        .join("iPod_Control")
        .join("iTunes")
        .join(classick::ipod::db::ITUNESDB_BACKUP_NAME)
}

fn corrupt_aside_path(mount: &Path) -> PathBuf {
    mount
        .join("iPod_Control")
        .join("iTunes")
        .join(ITUNESDB_CORRUPT_ASIDE_NAME)
}

const CORRUPT_BYTES: &[u8] = b"not-an-itunesdb-just-garbage-bytes-0123456789abcdef!!";

#[test]
fn open_with_auto_restore_restores_from_backup_on_corrupt_live_db() {
    let mount = fake_mount();

    // Valid backup, corrupt live DB.
    write_valid_itunesdb(&mount);
    let live = itunes_db_path(&mount);
    let good_bytes = std::fs::read(&live).unwrap();
    std::fs::copy(&live, backup_path(&mount)).unwrap();
    std::fs::write(&live, CORRUPT_BYTES).unwrap();

    let restored = std::sync::atomic::AtomicBool::new(false);
    let db = open_with_auto_restore(&mount, || {
        restored.store(true, Ordering::SeqCst);
    })
    .expect("open_with_auto_restore should recover via the backup");
    drop(db);

    assert!(
        restored.load(Ordering::SeqCst),
        "on_restore callback should fire"
    );

    // Corrupt live DB was set aside, not deleted.
    let aside = corrupt_aside_path(&mount);
    assert!(aside.exists(), "iTunesDB.corrupt should exist");
    assert_eq!(std::fs::read(&aside).unwrap(), CORRUPT_BYTES);

    // Live DB now parses and matches what the backup held.
    let live_bytes = std::fs::read(&live).unwrap();
    assert_eq!(live_bytes, good_bytes);
    unsafe {
        let path_c = CString::new(live.to_str().unwrap()).unwrap();
        let mut err: *mut ffi::GError = ptr::null_mut();
        let parsed = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
        assert!(!parsed.is_null(), "restored live DB should parse");
        ffi::itdb_free(parsed);
    }
}

#[test]
fn open_with_auto_restore_fails_when_backup_also_corrupt() {
    let mount = fake_mount();

    // Corrupt live DB AND corrupt backup.
    write_valid_itunesdb(&mount);
    let live = itunes_db_path(&mount);
    std::fs::write(&live, CORRUPT_BYTES).unwrap();
    std::fs::write(backup_path(&mount), b"also-garbage-not-a-real-itunesdb").unwrap();

    let restored = std::sync::atomic::AtomicBool::new(false);
    let result = open_with_auto_restore(&mount, || {
        restored.store(true, Ordering::SeqCst);
    });

    let err = match result {
        Ok(_) => panic!("should fail when the backup is also unparseable"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("--rebuild-manifest") && msg.contains("--restore-db-backup"),
        "error should name both recovery remedies, got: {msg}"
    );
    assert!(
        !restored.load(Ordering::SeqCst),
        "on_restore must not fire on failure"
    );

    // Live DB (still corrupt) untouched, and no .corrupt aside was created —
    // we must not clobber the only copy of the corrupt DB with a failed
    // restore attempt, and must not destructively rename it before we know
    // the backup is usable.
    assert_eq!(std::fs::read(&live).unwrap(), CORRUPT_BYTES);
    assert!(
        !corrupt_aside_path(&mount).exists(),
        "no .corrupt aside on failed restore"
    );
}

#[test]
fn restore_itunesdb_from_backup_errors_when_backup_missing() {
    let mount = fake_mount();
    write_valid_itunesdb(&mount);
    // No backup file at all.

    let result = restore_itunesdb_from_backup(&mount);
    assert!(
        result.is_err(),
        "should error when there is no backup to restore from"
    );
    assert!(!corrupt_aside_path(&mount).exists());
}
