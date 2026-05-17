mod ffi;

use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// Hardcoded for the Phase 0 spike. Phase 1 will accept this via CLI.
const IPOD_MOUNT: &str = "G:\\";

fn main() -> Result<()> {
    let mount_path = Path::new(IPOD_MOUNT);
    let db_path = mount_path.join("iPod_Control").join("iTunes").join("iTunesDB");
    if !db_path.exists() {
        return Err(anyhow!(
            "iTunesDB not found at {} — is the iPod mounted at {}?",
            db_path.display(),
            IPOD_MOUNT
        ));
    }

    println!("Opening iTunesDB at: {}", db_path.display());

    // SAFETY: itdb_parse_file allocates an Itdb_iTunesDB on success or returns
    // NULL and sets *error on failure. We must call itdb_free on success.
    let db = unsafe {
        let path_c = CString::new(db_path.to_str().unwrap())?;
        let mut err: *mut ffi::GError = ptr::null_mut();
        let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
        if db.is_null() {
            let msg = if err.is_null() {
                "itdb_parse_file returned NULL with no error".to_string()
            } else {
                let m = CStr::from_ptr((*err).message).to_string_lossy().into_owned();
                ffi::g_error_free(err);
                m
            };
            return Err(anyhow!("itdb_parse_file failed: {}", msg));
        }
        db
    };

    // Walk the track list (GList *)
    let mut count: usize = 0;
    let mut node = unsafe { (*db).tracks };
    let mut printed = 0usize;
    while !node.is_null() {
        let track = unsafe { (*node).data as *mut ffi::Itdb_Track };
        if printed < 5 && !track.is_null() {
            let title = unsafe { cstr_or_empty((*track).title) };
            let artist = unsafe { cstr_or_empty((*track).artist) };
            let album = unsafe { cstr_or_empty((*track).album) };
            println!("  [{}] {} — {} — {}", printed + 1, artist, album, title);
            printed += 1;
        }
        count += 1;
        node = unsafe { (*node).next };
    }

    println!("Total tracks: {}", count);

    // Free
    unsafe { ffi::itdb_free(db) };

    Ok(())
}

/// Convert a possibly-null C string from libgpod into a Rust String,
/// returning "<none>" if NULL.
unsafe fn cstr_or_empty(p: *mut std::os::raw::c_char) -> String {
    if p.is_null() {
        return "<none>".to_string();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}
