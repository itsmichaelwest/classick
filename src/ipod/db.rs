//! libgpod DB operations wrapped in RAII Rust types.

use crate::ffi;
use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// Owns an `Itdb_iTunesDB *` and frees it on Drop. Holds the database in memory
/// (libgpod's parse loads the whole thing). All write operations are methods.
pub struct OwnedDb(*mut ffi::Itdb_iTunesDB);

/// The metadata fields we copy into `Itdb_Track`. Parsed from ffprobe by main.rs.
#[derive(Debug, Default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub year: Option<i32>,
    pub track_nr: Option<i32>,
    pub tracks: Option<i32>,
    pub disc_nr: Option<i32>,
    pub discs: Option<i32>,
}

impl OwnedDb {
    /// Parse the iTunesDB at `<ipod_mount>\iPod_Control\iTunes\iTunesDB`.
    pub fn open(ipod_mount: &Path) -> Result<Self> {
        let db_path = ipod_mount
            .join("iPod_Control")
            .join("iTunes")
            .join("iTunesDB");
        let path_c = path_to_cstring(&db_path)?;
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
            if db.is_null() {
                return Err(gerror_to_anyhow("itdb_parse_file", err));
            }
            Ok(OwnedDb(db))
        }
    }

    pub fn as_ptr(&self) -> *mut ffi::Itdb_iTunesDB {
        self.0
    }

    /// Number of tracks currently in the DB.
    pub fn track_count(&self) -> usize {
        unsafe { ffi::itdb_tracks_number(self.0) as usize }
    }

    /// Persist DB to the iPod. After this returns Ok, the iPod's stored DB on
    /// disk reflects the in-memory state (track adds, file copies, etc.).
    pub fn write(&self) -> Result<()> {
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_write(self.0, &mut err) == 0 {
                return Err(gerror_to_anyhow("itdb_write", err));
            }
        }
        Ok(())
    }

    /// Copy `source_alac` onto the iPod, attach metadata `tags`, add to the
    /// master playlist. Does NOT call `itdb_write` — call `write()` separately
    /// so the caller controls when the DB is flushed.
    ///
    /// On failure mid-way (file copied but playlist add fails), the file is
    /// left on the iPod orphaned — Phase 2's `--rebuild-manifest` recovers
    /// from this kind of state. Phase 1 just surfaces the error.
    pub fn add_track_with_file(&self, source_alac: &Path, tags: &Tags) -> Result<()> {
        let alac_c = path_to_cstring(source_alac)?;
        unsafe {
            let track = ffi::itdb_track_new();
            if track.is_null() {
                return Err(anyhow!("itdb_track_new returned NULL"));
            }
            apply_tags(track, tags);

            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_cp_track_to_ipod(track, alac_c.as_ptr(), &mut err) == 0 {
                // The track was not added to the DB; we own it and must free.
                ffi::itdb_track_free(track);
                return Err(gerror_to_anyhow("itdb_cp_track_to_ipod", err));
            }
            // cp_track adds the track to db.tracks; we still need to add it to
            // the master playlist for it to show in the iPod's Songs menu.
            let master = ffi::itdb_playlist_mpl(self.0);
            if master.is_null() {
                return Err(anyhow!(
                    "master playlist missing on this iPod (corrupt DB?)"
                ));
            }
            ffi::itdb_playlist_add_track(master, track, -1);
        }
        Ok(())
    }
}

impl Drop for OwnedDb {
    fn drop(&mut self) {
        unsafe {
            ffi::itdb_free(self.0);
        }
    }
}

/// Copy each set field from `tags` into the corresponding `Itdb_Track` slot.
/// Strings are duplicated via `g_strdup` so libgpod owns them and frees with
/// the track. Numeric fields are written directly. Unset Optional fields leave
/// the libgpod default (typically 0 or NULL).
///
/// # Safety
/// `track` must be a freshly-allocated `Itdb_Track *` from `itdb_track_new`.
unsafe fn apply_tags(track: *mut ffi::Itdb_Track, tags: &Tags) {
    set_str(&mut (*track).title, tags.title.as_deref());
    set_str(&mut (*track).artist, tags.artist.as_deref());
    set_str(&mut (*track).album, tags.album.as_deref());
    set_str(&mut (*track).albumartist, tags.album_artist.as_deref());
    set_str(&mut (*track).genre, tags.genre.as_deref());
    set_str(&mut (*track).composer, tags.composer.as_deref());
    if let Some(y) = tags.year {
        (*track).year = y;
    }
    if let Some(n) = tags.track_nr {
        (*track).track_nr = n;
    }
    if let Some(t) = tags.tracks {
        (*track).tracks = t;
    }
    if let Some(n) = tags.disc_nr {
        (*track).cd_nr = n;
    }
    if let Some(t) = tags.discs {
        (*track).cds = t;
    }
}

/// Replace `*slot` with a g_strdup of `value`, freeing whatever was there.
/// `g_free(NULL)` is a documented no-op.
unsafe fn set_str(slot: *mut *mut std::os::raw::c_char, value: Option<&str>) {
    ffi::g_free(*slot as *mut std::os::raw::c_void);
    *slot = match value {
        Some(s) => {
            // FLAC tags should not contain interior NULs but defend against it:
            // CString::new fails on NUL, in which case we skip the tag rather
            // than silently truncate or panic.
            match CString::new(s) {
                Ok(c) => ffi::g_strdup(c.as_ptr()),
                Err(_) => ptr::null_mut(),
            }
        }
        None => ptr::null_mut(),
    };
}

fn path_to_cstring(p: &Path) -> Result<CString> {
    let s = p
        .to_str()
        .ok_or_else(|| anyhow!("path contains non-UTF-8: {}", p.display()))?;
    CString::new(s)
        .map_err(|_| anyhow!("path contains interior NUL byte: {}", p.display()))
}

unsafe fn gerror_to_anyhow(api: &str, err: *mut ffi::GError) -> anyhow::Error {
    if err.is_null() {
        return anyhow!("{api} failed (no error detail)");
    }
    let msg = CStr::from_ptr((*err).message).to_string_lossy().into_owned();
    ffi::g_error_free(err);
    anyhow!("{api} failed: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_to_cstring_accepts_ascii() {
        let p = PathBuf::from(r"C:\foo\bar.m4a");
        let c = path_to_cstring(&p).expect("ascii path converts");
        assert_eq!(c.to_str().unwrap(), r"C:\foo\bar.m4a");
    }

    #[test]
    fn path_to_cstring_accepts_unc() {
        let p = PathBuf::from(r"\\server\share\file.flac");
        let c = path_to_cstring(&p).expect("UNC path converts");
        assert_eq!(c.to_str().unwrap(), r"\\server\share\file.flac");
    }

    #[test]
    fn tags_default_is_all_none() {
        let t = Tags::default();
        assert!(t.title.is_none());
        assert!(t.artist.is_none());
        assert!(t.year.is_none());
    }
}
