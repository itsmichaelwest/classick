//! libgpod DB operations wrapped in RAII Rust types.

use crate::ffi;
use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// Owns an `Itdb_iTunesDB *` and frees it on Drop. Holds the database in memory
/// (libgpod's parse loads the whole thing). All write operations are methods.
pub struct OwnedDb(*mut ffi::Itdb_iTunesDB);

/// Identifies a track on the iPod after add. Returned by `add_track_with_file`
/// and `list_tracks_for_rebuild`; recorded in `ManifestEntry`.
#[derive(Debug, Clone)]
pub struct TrackHandle {
    pub dbid: u64,
    /// Relative path with Windows backslashes: `iPod_Control\Music\F41\libgpod079263.m4a`.
    pub ipod_relpath: String,
}

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
    /// Parse the iTunesDB at `<ipod_mount>\iPod_Control\iTunes\iTunesDB` and
    /// wire the mountpoint into the DB so libgpod write helpers (itdb_cp_track_to_ipod,
    /// itdb_filename_on_ipod, etc.) know where to put files on disk.
    pub fn open(ipod_mount: &Path) -> Result<Self> {
        let db_path = ipod_mount
            .join("iPod_Control")
            .join("iTunes")
            .join("iTunesDB");
        let path_c = path_to_cstring(&db_path)?;
        let mount_c = path_to_cstring(ipod_mount)?;
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
            if db.is_null() {
                return Err(gerror_to_anyhow("itdb_parse_file", err));
            }
            // itdb_parse_file does NOT set the mountpoint (it only knows about the
            // DB file). Subsequent libgpod helpers that need to write files onto the
            // iPod (e.g. itdb_cp_track_to_ipod) assert on the mountpoint being set.
            ffi::itdb_set_mountpoint(db, mount_c.as_ptr());
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
        // libgpod's itdb_write renames `<mount>\iPod_Control\iTunes\Play Counts`
        // to `Play Counts.bak` via POSIX rename(). On Windows, rename() fails
        // (silently to libgpod, surfaced as a vague GError) if the target exists.
        // Pre-delete the stale .bak so the rename always has a clean target.
        // Discovered while building examples/wipe-tracks.rs on 2026-05-17.
        unsafe {
            let mount_c = ffi::itdb_get_mountpoint(self.0);
            if !mount_c.is_null() {
                let mount = CStr::from_ptr(mount_c).to_string_lossy();
                let bak = Path::new(mount.as_ref())
                    .join("iPod_Control")
                    .join("iTunes")
                    .join("Play Counts.bak");
                let _ = std::fs::remove_file(&bak); // ignore NotFound; surface other errors via the subsequent write
            }

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
    pub fn add_track_with_file(
        &self,
        source_alac: &Path,
        tags: &Tags,
        art: Option<&[u8]>,
    ) -> Result<TrackHandle> {
        let alac_c = path_to_cstring(source_alac)?;
        unsafe {
            let track = ffi::itdb_track_new();
            if track.is_null() {
                return Err(anyhow!("itdb_track_new returned NULL"));
            }
            apply_tags(track, tags);

            // Plan B (SPEC §8 row 3): write artwork via libgpod's ArtworkDB+ithmb
            // thumbnail system. The iPod Classic UI ignores embedded MP4 cover
            // atoms — it reads from iPod_Control\Artwork. Done before
            // itdb_track_add so a failure here doesn't leave a half-attached
            // track in the DB.
            if let Some(bytes) = art {
                let ok = ffi::itdb_track_set_thumbnails_from_data(
                    track,
                    bytes.as_ptr(),
                    bytes.len() as _,
                );
                if ok == 0 {
                    // Track isn't yet attached to db, so we own it.
                    ffi::itdb_track_free(track);
                    return Err(anyhow!("itdb_track_set_thumbnails_from_data failed"));
                }
            }

            // itdb_cp_track_to_ipod requires track->itdb to be set; that back-pointer
            // is wired by itdb_track_add. (Without this, libgpod aborts with
            // "assertion 'track->itdb' failed".) Add to DB first, then copy the file.
            ffi::itdb_track_add(self.0, track, -1);

            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_cp_track_to_ipod(track, alac_c.as_ptr(), &mut err) == 0 {
                // cp failed: unlink from DB (does not free) and free ourselves.
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(gerror_to_anyhow("itdb_cp_track_to_ipod", err));
            }
            // Track is in db.tracks; add to master playlist so it shows in Songs menu.
            let master = ffi::itdb_playlist_mpl(self.0);
            if master.is_null() {
                return Err(anyhow!(
                    "master playlist missing on this iPod (corrupt DB?)"
                ));
            }
            ffi::itdb_playlist_add_track(master, track, -1);

            // Read the assigned dbid + ipod_path from the now-attached track.
            let dbid = (*track).dbid as u64;
            let relpath = read_ipod_relpath(track);
            Ok(TrackHandle { dbid, ipod_relpath: relpath })
        }
    }

    /// Remove a track from the iPod by dbid. Idempotent (returns Ok if not present).
    /// Does NOT call `itdb_write`; the caller batches multiple removes + adds
    /// then calls `write` once.
    pub fn delete_track(&self, dbid: u64) -> Result<()> {
        unsafe {
            // Find the track by walking the GList. libgpod doesn't expose a
            // hashmap lookup; 1,400 tracks at ~30ns per pointer-chase is fine.
            let mut node = (*self.0).tracks;
            let mut found: *mut ffi::Itdb_Track = std::ptr::null_mut();
            while !node.is_null() {
                let t = (*node).data as *mut ffi::Itdb_Track;
                if !t.is_null() && (*t).dbid as u64 == dbid {
                    found = t;
                    break;
                }
                node = (*node).next;
            }
            if found.is_null() {
                return Ok(()); // already gone; idempotent
            }

            // Delete the on-iPod file via libgpod's path helper.
            let fname_c = ffi::itdb_filename_on_ipod(found);
            if !fname_c.is_null() {
                let path_str = std::ffi::CStr::from_ptr(fname_c).to_string_lossy().into_owned();
                let _ = std::fs::remove_file(std::path::Path::new(&path_str));
                ffi::g_free(fname_c as *mut std::os::raw::c_void);
            }
            // Remove from all playlists, then remove + free the track.
            ffi::itdb_playlist_remove_track(std::ptr::null_mut(), found);
            ffi::itdb_track_remove(found);
        }
        Ok(())
    }

    /// Update an existing iPod track's tags + thumbnails without touching the
    /// audio file. Used by the Phase 3.x MetadataOnly path: the source file's
    /// audio is bit-identical to what's already on the iPod, so we just refresh
    /// the metadata libgpod tracks for it.
    ///
    /// Does NOT call `itdb_write` — caller batches that at end of run.
    /// Returns `Ok(())` even if the dbid isn't found (idempotent, matching
    /// `delete_track`'s semantics).
    pub fn update_track_metadata(
        &self,
        dbid: u64,
        tags: &Tags,
        art: Option<&[u8]>,
    ) -> Result<()> {
        unsafe {
            let mut node = (*self.0).tracks;
            let mut found: *mut ffi::Itdb_Track = std::ptr::null_mut();
            while !node.is_null() {
                let t = (*node).data as *mut ffi::Itdb_Track;
                if !t.is_null() && (*t).dbid as u64 == dbid {
                    found = t;
                    break;
                }
                node = (*node).next;
            }
            if found.is_null() {
                return Ok(()); // idempotent: track not present
            }

            apply_tags(found, tags);

            if let Some(bytes) = art {
                let ok = ffi::itdb_track_set_thumbnails_from_data(
                    found,
                    bytes.as_ptr(),
                    bytes.len() as _,
                );
                if ok == 0 {
                    return Err(anyhow!(
                        "itdb_track_set_thumbnails_from_data failed for dbid {dbid}"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Walk all tracks currently in the DB and return their handles.
    /// Used by `--rebuild-manifest` to populate a fresh manifest with
    /// `source_known = false` entries.
    pub fn list_tracks_for_rebuild(&self) -> Vec<TrackHandle> {
        let mut out = Vec::new();
        unsafe {
            let mut node = (*self.0).tracks;
            while !node.is_null() {
                let t = (*node).data as *mut ffi::Itdb_Track;
                if !t.is_null() {
                    out.push(TrackHandle {
                        dbid: (*t).dbid as u64,
                        ipod_relpath: read_ipod_relpath(t),
                    });
                }
                node = (*node).next;
            }
        }
        out
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

/// Convert libgpod's colon-separated `ipod_path` to Windows backslashes,
/// stripping the leading colon. libgpod stores e.g. `:iPod_Control:Music:F12:KLMN.m4a`;
/// the manifest stores `iPod_Control\Music\F12\KLMN.m4a`.
unsafe fn read_ipod_relpath(track: *mut ffi::Itdb_Track) -> String {
    let p = (*track).ipod_path;
    if p.is_null() {
        return String::new();
    }
    let s = std::ffi::CStr::from_ptr(p).to_string_lossy();
    s.trim_start_matches(':').replace(':', "\\")
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
