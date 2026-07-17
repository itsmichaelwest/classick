//! Spike: determine when libgpod assigns `Itdb_Playlist.id` — at
//! `itdb_playlist_new`/`itdb_playlist_add` time, or only at `itdb_write`
//! time. Answers a question `ipod::db::ensure_managed_playlist`'s doc
//! comment depends on. libgpod ships in this repo as vendored headers +
//! prebuilt binaries (no C source to read), so this was run empirically
//! rather than looked up.
//!
//! Finding (`cargo run -p classick --example playlist-id-spike`):
//! - Right after `itdb_playlist_new`: id == 0.
//! - Right after `itdb_playlist_add`: id is a real random u64, and it's
//!   stable across `itdb_write` + a fresh `itdb_parse` from disk.
//! So the id can be read immediately after `itdb_playlist_add` — no
//! write-then-re-walk pass is needed to learn a freshly created playlist's
//! id.
use classick::ffi;
use std::ffi::CString;
use std::ptr;

fn main() {
    let dir = std::env::temp_dir().join("playlist-id-spike");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("iPod_Control").join("iTunes")).unwrap();
    std::fs::create_dir_all(dir.join("iPod_Control").join("Music").join("F00")).unwrap();

    unsafe {
        let db = ffi::itdb_new();
        let mount_c = CString::new(dir.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(db, mount_c.as_ptr());

        let mpl_title = CString::new("iPod").unwrap();
        let mpl = ffi::itdb_playlist_new(mpl_title.as_ptr(), 0);
        ffi::itdb_playlist_set_mpl(mpl);
        ffi::itdb_playlist_add(db, mpl, -1);

        let title = CString::new("Gym").unwrap();
        let pl = ffi::itdb_playlist_new(title.as_ptr(), 0);
        println!("id immediately after itdb_playlist_new (pre-add): {}", (*pl).id);
        ffi::itdb_playlist_add(db, pl, -1);
        println!("id immediately after itdb_playlist_add: {}", (*pl).id);

        let mut err: *mut ffi::GError = ptr::null_mut();
        let ok = ffi::itdb_write(db, &mut err);
        println!("write ok={}", ok != 0);
        println!("id after itdb_write (same in-memory pl ptr): {}", (*pl).id);

        ffi::itdb_free(db);

        // Reparse from disk and find "Gym" by name, compare id.
        let db2 = ffi::itdb_parse(mount_c.as_ptr(), &mut err);
        assert!(!db2.is_null());
        let name_c = CString::new("Gym").unwrap();
        let pl2 = ffi::itdb_playlist_by_name(db2, name_c.as_ptr() as *mut _);
        assert!(!pl2.is_null());
        println!("id after reparse from disk: {}", (*pl2).id);
        ffi::itdb_free(db2);
    }
}
