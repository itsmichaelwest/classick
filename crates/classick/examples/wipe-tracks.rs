//! Removes ALL tracks from the iPod via libgpod. Dev utility for Phase 2
//! iteration — running Phase 1 repeatedly produces duplicate tracks.
//!
//! Usage: cargo run --example wipe-tracks -- [mount]
//! Default mount: G:\

use anyhow::Result;
use classick::ffi;
use classick::ipod::db::OwnedDb;
use classick::ipod::device;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::ptr;

fn main() -> Result<()> {
    // Required so libgpod's artwork write path can find pixbuf loaders.
    // Not strictly needed for a wipe, but setting it is harmless and avoids
    // a GLib warning if the DB write internally pokes at artwork formats.
    unsafe {
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE"));
    }

    let mount = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "G:\\".to_string());
    let mount_path = PathBuf::from(&mount);

    println!("Opening iPod DB at {}", mount);
    let db = OwnedDb::open(&mount_path)?;

    let initial_count = db.track_count();
    println!("Found {} track(s) on iPod.", initial_count);

    if initial_count == 0 {
        println!("Nothing to do.");
        return Ok(());
    }

    println!("Wiring FirewireGuid for write signing...");
    let guid = device::read_firewire_guid(&mount_path)?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &guid)?;
    }

    println!(
        "About to DELETE ALL {} track(s) from {}.",
        initial_count, mount
    );
    println!("Sleeping 5 seconds; Ctrl+C now to abort...");
    for n in (1..=5).rev() {
        println!("  {n}...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Collect all track pointers into a Vec before iterating to avoid
    // mutation-during-iteration on the GList.
    let mut tracks: Vec<*mut ffi::Itdb_Track> = Vec::with_capacity(initial_count);
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            tracks.push((*node).data as *mut ffi::Itdb_Track);
            node = (*node).next;
        }
    }

    println!("Removing {} track(s) + their on-disk files...", tracks.len());
    let mut files_deleted = 0usize;
    let mut files_missing = 0usize;

    unsafe {
        for track in &tracks {
            let track = *track;

            // Get the on-iPod filename. Returns a g_strdup'd path we must g_free.
            let fname_c = ffi::itdb_filename_on_ipod(track);
            if !fname_c.is_null() {
                let path_str = CStr::from_ptr(fname_c).to_string_lossy().into_owned();
                let path = Path::new(&path_str);
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        println!("  deleted: {}", path.display());
                        files_deleted += 1;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        eprintln!("  warn: file not found (already deleted?): {}", path.display());
                        files_missing += 1;
                    }
                    Err(e) => {
                        eprintln!("  warn: failed to delete {}: {e}", path.display());
                    }
                }
                ffi::g_free(fname_c as *mut std::os::raw::c_void);
            } else {
                eprintln!("  warn: itdb_filename_on_ipod returned NULL for a track (no file to delete)");
            }

            // Remove from all playlists. Passing NULL for the playlist means
            // "remove this track from every playlist that contains it".
            ffi::itdb_playlist_remove_track(ptr::null_mut(), track);

            // Remove from the DB tracks list AND free the track struct.
            ffi::itdb_track_remove(track);
        }
    }

    println!(
        "Files deleted: {}, missing/already gone: {}.",
        files_deleted, files_missing
    );

    // libgpod's write path tries to rename "Play Counts" → "Play Counts.bak".
    // On Windows, rename fails if the destination already exists (unlike POSIX).
    // Delete the stale .bak first so the rename can succeed.
    let play_counts_bak = mount_path
        .join("iPod_Control")
        .join("iTunes")
        .join("Play Counts.bak");
    if play_counts_bak.exists() {
        println!("Removing stale Play Counts.bak to allow write...");
        std::fs::remove_file(&play_counts_bak)?;
    }

    println!("Writing DB...");
    db.write()?;

    println!(
        "Wiped {} tracks. New track count: {}.",
        initial_count,
        db.track_count()
    );
    println!("Eject the iPod and verify it boots normally before re-syncing.");

    Ok(())
}
