//! libgpod DB operations wrapped in RAII Rust types.

use crate::ffi;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackFileDisposition {
    DeleteAfterCommit,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistStructuralKind {
    Normal,
    Master,
    Podcast,
    Smart,
}

/// Read-only snapshot of every per-track artwork signal exposed by libgpod.
/// Kept as raw facts so audit policy and failure ordering remain pure/testable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackArtworkSignals {
    pub has_artwork: bool,
    pub mhii_link: u32,
    pub has_artwork_record: bool,
    pub has_thumbnail: bool,
    pub has_thumbnails: bool,
    pub decoded_thumbnail: bool,
}

/// The metadata fields we copy into `Itdb_Track`. Parsed from ffprobe by main.rs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Track length in milliseconds → iTunesDB `tracklen`. `None` leaves the
    /// libgpod default (0); the iPod shows -0:00 without it.
    pub duration_ms: Option<u32>,
}

impl OwnedDb {
    /// Parse the iTunesDB at `<ipod_mount>\iPod_Control\iTunes\iTunesDB` and
    /// its companion ArtworkDB from the mount.
    pub fn open(ipod_mount: &Path) -> Result<Self> {
        let mount_c = path_to_cstring(ipod_mount)?;
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            let db = ffi::itdb_parse(mount_c.as_ptr(), &mut err);
            if db.is_null() {
                return Err(gerror_to_anyhow("itdb_parse", err));
            }
            Ok(OwnedDb(db))
        }
    }

    #[cfg(unix)]
    /// Parse through an already-open regular-file authority without resolving
    /// the device pathname again.
    pub(crate) fn parse_from_file_handle(database: &File, ipod_mount: &Path) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let descriptor_root = Path::new("/proc/self/fd");
        #[cfg(not(target_os = "linux"))]
        let descriptor_root = Path::new("/dev/fd");
        let descriptor_path = descriptor_root.join(database.as_raw_fd().to_string());
        Self::open_path_at_mount(&descriptor_path, ipod_mount)
    }

    fn open_path_at_mount(db_path: &Path, ipod_mount: &Path) -> Result<Self> {
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
    ///
    /// Phase 3.z: if the first `itdb_write` fails, we cleanup the stale
    /// Play Counts.bak (which may have been re-created between our pre-emptive
    /// cleanup and itdb_write's rename attempt) and try one more time before
    /// bubbling the error. The user only sees a failure if BOTH attempts fail.
    pub fn write(&self) -> Result<()> {
        // Pre-emptive .bak cleanup (existing Phase 2.1 fix).
        self.cleanup_play_counts_bak();

        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_write(self.0, &mut err) == 0 {
                // Retry path: maybe .bak was created between our cleanup and
                // itdb_write's internal rename. Clean it again and try once more.
                self.cleanup_play_counts_bak();
                let mut err2: *mut ffi::GError = ptr::null_mut();
                if ffi::itdb_write(self.0, &mut err2) == 0 {
                    // Both attempts failed. Free the first error (the second
                    // is consumed by gerror_to_anyhow below) and surface the
                    // retry failure as the canonical error.
                    if !err.is_null() {
                        ffi::g_error_free(err);
                    }
                    return Err(gerror_to_anyhow("itdb_write (after .bak retry)", err2));
                }
                // Success on retry: free the first error and continue.
                if !err.is_null() {
                    ffi::g_error_free(err);
                }
            }
        }
        Ok(())
    }

    /// Delete `<mount>\iPod_Control\iTunes\Play Counts.bak` if it exists.
    ///
    /// libgpod's itdb_write renames `<mount>\iPod_Control\iTunes\Play Counts`
    /// to `Play Counts.bak` via POSIX rename(). On Windows, rename() fails
    /// (silently to libgpod, surfaced as a vague GError) if the target exists.
    /// Pre-delete the stale .bak so the rename always has a clean target.
    /// Discovered while building examples/wipe-tracks.rs on 2026-05-17.
    fn cleanup_play_counts_bak(&self) {
        unsafe {
            let mount_c = ffi::itdb_get_mountpoint(self.0);
            if !mount_c.is_null() {
                let mount = CStr::from_ptr(mount_c).to_string_lossy();
                let bak = crate::ipod::layout::play_counts_bak_path(Path::new(mount.as_ref()));
                let _ = std::fs::remove_file(&bak); // ignore NotFound; other errors surface on the subsequent write
            }
        }
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
                    // Non-fatal: thumbnailing needs gdk-pixbuf loader plugins,
                    // which may be absent in a bundled app on a machine without
                    // Homebrew. Add the track WITHOUT artwork rather than
                    // failing the whole sync over a missing cover.
                    tracing::warn!(
                        "artwork thumbnail failed (missing gdk-pixbuf loaders?); \
                         adding track without art"
                    );
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
                // Cleanup mirror of the itdb_cp_track_to_ipod failure path:
                // the file is already on the iPod and the track is linked into
                // the DB. Without cleanup, a Retry of the surrounding op
                // re-copies the same source as a NEW file each time, leaving
                // a trail of orphan .m4a blobs in iPod_Control\Music.
                let fname_c = ffi::itdb_filename_on_ipod(track);
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                if !fname_c.is_null() {
                    let path_str = CStr::from_ptr(fname_c).to_string_lossy().into_owned();
                    let _ = std::fs::remove_file(Path::new(&path_str));
                    ffi::g_free(fname_c as *mut std::os::raw::c_void);
                }
                return Err(anyhow!(
                    "master playlist missing on this iPod (corrupt DB?)"
                ));
            }
            ffi::itdb_playlist_add_track(master, track, -1);

            // Read the assigned dbid + ipod_path from the now-attached track.
            let dbid = (*track).dbid as u64;
            let relpath = read_ipod_relpath(track);
            Ok(TrackHandle {
                dbid,
                ipod_relpath: relpath,
            })
        }
    }

    /// Artwork-safe add variant. Unlike the historical add path, supplied
    /// artwork is required to attach successfully; a thumbnail failure rolls
    /// back the in-memory track and copied file instead of publishing artless.
    pub fn add_track_with_file_strict(
        &self,
        source_alac: &Path,
        tags: &Tags,
        art: Option<&[u8]>,
    ) -> Result<TrackHandle> {
        let handle = self.add_track_with_file(source_alac, tags, art)?;
        if art.is_some() && !self.track_has_artwork(handle.dbid) {
            let removed = self.unlink_track_keep_file(handle.dbid)?;
            if let Some(removed) = removed {
                if let Some(mount) = self.mount_path() {
                    let _ = std::fs::remove_file(
                        mount.join(
                            removed
                                .ipod_relpath
                                .replace('\\', std::path::MAIN_SEPARATOR_STR),
                        ),
                    );
                }
            }
            return Err(anyhow!(
                "artwork thumbnail preparation failed for dbid {}",
                handle.dbid
            ));
        }
        Ok(handle)
    }

    /// Add a track whose staged media already lives on the iPod.
    ///
    /// libgpod's normal helper copies the source bytes even when source and
    /// destination are on the same mounted device. This variant reserves the
    /// libgpod destination, lets the caller journal it, atomically renames the
    /// staged file into place, and then asks libgpod to finalize the track
    /// metadata.
    pub fn add_track_with_staged_file_strict(
        &self,
        staged_file: &Path,
        tags: &Tags,
        art: Option<&[u8]>,
        before_move: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<TrackHandle> {
        let staged_c = path_to_cstring(staged_file)?;
        unsafe {
            let track = ffi::itdb_track_new();
            if track.is_null() {
                return Err(anyhow!("itdb_track_new returned NULL"));
            }
            apply_tags(track, tags);

            if let Some(bytes) = art {
                if ffi::itdb_track_set_thumbnails_from_data(track, bytes.as_ptr(), bytes.len() as _)
                    == 0
                {
                    ffi::itdb_track_free(track);
                    return Err(anyhow!("artwork thumbnail preparation failed"));
                }
            }

            ffi::itdb_track_add(self.0, track, -1);
            let dbid = (*track).dbid as u64;
            if art.is_some() && !self.track_has_artwork(dbid) {
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(anyhow!(
                    "artwork thumbnail preparation failed for dbid {dbid}"
                ));
            }

            let master = ffi::itdb_playlist_mpl(self.0);
            if master.is_null() {
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(anyhow!(
                    "master playlist missing on this iPod (corrupt DB?)"
                ));
            }

            let mut error: *mut ffi::GError = ptr::null_mut();
            let destination_c =
                ffi::itdb_cp_get_dest_filename(track, ptr::null(), staged_c.as_ptr(), &mut error);
            if destination_c.is_null() {
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(gerror_to_anyhow("itdb_cp_get_dest_filename", error));
            }
            let destination =
                PathBuf::from(CStr::from_ptr(destination_c).to_string_lossy().as_ref());

            if let Err(error) = before_move(&destination) {
                ffi::g_free(destination_c.cast());
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(error).context("journal staged-file destination before move");
            }
            if let Err(error) = std::fs::rename(staged_file, &destination) {
                ffi::g_free(destination_c.cast());
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                return Err(error).with_context(|| {
                    format!(
                        "move staged media {} to {}",
                        staged_file.display(),
                        destination.display()
                    )
                });
            }

            let mut finalize_error: *mut ffi::GError = ptr::null_mut();
            let finalized =
                ffi::itdb_cp_finalize(track, ptr::null(), destination_c, &mut finalize_error);
            ffi::g_free(destination_c.cast());
            if finalized.is_null() {
                let failure = gerror_to_anyhow("itdb_cp_finalize", finalize_error);
                let restore = std::fs::rename(&destination, staged_file);
                ffi::itdb_track_unlink(track);
                ffi::itdb_track_free(track);
                if let Err(restore) = restore {
                    return Err(failure).context(format!(
                        "also failed to restore {} to {}: {restore}",
                        destination.display(),
                        staged_file.display()
                    ));
                }
                return Err(failure);
            }

            ffi::itdb_playlist_add_track(master, track, -1);
            Ok(TrackHandle {
                dbid,
                ipod_relpath: read_ipod_relpath(track),
            })
        }
    }

    unsafe fn unlink_track_from_all_playlists(&self, track: *mut ffi::Itdb_Track) -> Result<usize> {
        if track.is_null() {
            return Err(anyhow!("cannot unlink a null track"));
        }
        let mut containing = Vec::new();
        let mut node = (*self.0).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() && ffi::itdb_playlist_contains_track(playlist, track) != 0 {
                containing.push(playlist);
            }
            node = (*node).next;
        }
        for playlist in &containing {
            ffi::itdb_playlist_remove_track(*playlist, track);
        }
        Ok(containing.len())
    }

    /// Remove a track record after first unlinking every normal, smart, and
    /// master-playlist membership. Does not write the database.
    pub fn remove_track(&self, dbid: u64, disposition: TrackFileDisposition) -> Result<bool> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            if found.is_null() {
                return Ok(false);
            }
            let fname_c = ffi::itdb_filename_on_ipod(found);
            let file = if fname_c.is_null() {
                None
            } else {
                let path = PathBuf::from(CStr::from_ptr(fname_c).to_string_lossy().as_ref());
                ffi::g_free(fname_c as *mut std::os::raw::c_void);
                Some(path)
            };
            self.unlink_track_from_all_playlists(found)?;
            ffi::itdb_track_remove(found);
            if disposition == TrackFileDisposition::DeleteAfterCommit {
                if let Some(path) = file {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("delete removed track audio {}", path.display())
                            });
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    /// Compatibility wrapper for callers that want immediate file deletion.
    pub fn delete_track(&self, dbid: u64) -> Result<()> {
        self.remove_track(dbid, TrackFileDisposition::DeleteAfterCommit)
            .map(|_| ())
    }

    /// Remove a track record and playlist membership while retaining its
    /// audio file until the coordinated publication has been verified.
    pub fn unlink_track_keep_file(&self, dbid: u64) -> Result<Option<TrackHandle>> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            if found.is_null() {
                return Ok(None);
            }
            let handle = TrackHandle {
                dbid,
                ipod_relpath: read_ipod_relpath(found),
            };
            self.remove_track(dbid, TrackFileDisposition::Keep)?;
            Ok(Some(handle))
        }
    }

    /// Update an existing iPod track's tags + thumbnails without touching the
    /// audio file. Used by the Phase 3.x MetadataOnly path: the source file's
    /// audio is bit-identical to what's already on the iPod, so we just refresh
    /// the metadata libgpod tracks for it.
    ///
    /// Does NOT call `itdb_write` — caller batches that at end of run.
    /// Returns `Ok(())` even if the dbid isn't found (idempotent, matching
    /// `delete_track`'s semantics).
    pub fn update_track_metadata(&self, dbid: u64, tags: &Tags, art: Option<&[u8]>) -> Result<()> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
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

    pub fn set_track_metadata_and_art(
        &self,
        dbid: u64,
        tags: &Tags,
        art: Option<&[u8]>,
    ) -> Result<()> {
        self.update_track_metadata(dbid, tags, art)
    }

    /// Replace only the thumbnail input, preserving every metadata field.
    pub fn set_track_artwork(&self, dbid: u64, art: Option<&[u8]>) -> Result<()> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            if found.is_null() {
                return Err(anyhow!("track dbid {dbid} is missing"));
            }
            match art {
                Some(bytes) => {
                    if ffi::itdb_track_set_thumbnails_from_data(
                        found,
                        bytes.as_ptr(),
                        bytes.len() as _,
                    ) == 0
                    {
                        return Err(anyhow!(
                            "itdb_track_set_thumbnails_from_data failed for dbid {dbid}"
                        ));
                    }
                }
                None => ffi::itdb_track_remove_thumbnails(found),
            }
        }
        Ok(())
    }

    pub fn track_has_artwork(&self, dbid: u64) -> bool {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            !found.is_null()
                && (*found).has_artwork == 1
                && ffi::itdb_track_has_thumbnails(found) != 0
        }
    }

    /// Inspect a track's complete artwork chain without mutating the DB.
    /// The decoded thumbnail is a newly-referenced GdkPixbuf owned by the
    /// caller, so every non-null return must be released with `g_object_unref`.
    pub fn track_artwork_signals(&self, dbid: u64) -> Option<TrackArtworkSignals> {
        unsafe {
            let track = self.find_track_by_dbid(dbid);
            if track.is_null() {
                return None;
            }

            let artwork = (*track).artwork;
            let thumbnail = if artwork.is_null() {
                ptr::null_mut()
            } else {
                (*artwork).thumbnail
            };
            let pixbuf = ffi::itdb_track_get_thumbnail(track, -1, -1);
            let decoded_thumbnail = !pixbuf.is_null();
            if decoded_thumbnail {
                ffi::g_object_unref(pixbuf);
            }

            Some(TrackArtworkSignals {
                has_artwork: (*track).has_artwork == 1,
                mhii_link: (*track).mhii_link,
                has_artwork_record: !artwork.is_null(),
                has_thumbnail: !thumbnail.is_null(),
                has_thumbnails: ffi::itdb_track_has_thumbnails(track) != 0,
                decoded_thumbnail,
            })
        }
    }

    pub fn verify_track(
        &self,
        dbid: u64,
        expected_relpath: &str,
        expects_artwork: bool,
    ) -> Result<()> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            if found.is_null() {
                return Err(anyhow!("track dbid {dbid} is missing after publication"));
            }
            let actual = read_ipod_relpath(found);
            if normalize_relpath(&actual) != normalize_relpath(expected_relpath) {
                return Err(anyhow!(
                    "track dbid {dbid} path mismatch: expected {expected_relpath:?}, got {actual:?}"
                ));
            }
            if expects_artwork && !self.track_has_artwork(dbid) {
                return Err(anyhow!(
                    "track dbid {dbid} has no thumbnail after publication"
                ));
            }
        }
        Ok(())
    }

    pub fn referenced_paths(&self, mount: &Path) -> std::collections::HashSet<PathBuf> {
        self.list_tracks_for_rebuild()
            .into_iter()
            .filter(|track| !track.ipod_relpath.is_empty())
            .map(|track| {
                mount.join(
                    track
                        .ipod_relpath
                        .replace('\\', std::path::MAIN_SEPARATOR_STR),
                )
            })
            .collect()
    }

    pub fn mount_path(&self) -> Option<PathBuf> {
        unsafe {
            let mount = ffi::itdb_get_mountpoint(self.0);
            (!mount.is_null())
                .then(|| PathBuf::from(CStr::from_ptr(mount).to_string_lossy().as_ref()))
        }
    }

    /// Walk the DB's track GList and return the first track whose dbid
    /// matches, or NULL if none. libgpod doesn't expose a hashmap lookup;
    /// ~1,400 tracks at ~30ns per pointer-chase is fine.
    ///
    /// # Safety
    /// The returned pointer is only valid for the lifetime of `&self`.
    unsafe fn find_track_by_dbid(&self, dbid: u64) -> *mut ffi::Itdb_Track {
        let mut node = (*self.0).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            if !t.is_null() && (*t).dbid as u64 == dbid {
                return t;
            }
            node = (*node).next;
        }
        std::ptr::null_mut()
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

    /// Reconcile the in-memory iTunesDB with the iPod's on-disk
    /// `iPod_Control\Music\F**\` folder. Two failure modes this fixes:
    ///
    /// 1. **Orphans on disk** — files no track in the DB references.
    ///    Created when a previous sync called `itdb_cp_track_to_ipod`
    ///    (which copies the file AND adds to in-memory DB) but died
    ///    before `db.write()` persisted. Deleted from disk.
    /// 2. **Dangling DB refs** — DB tracks whose `ipod_path` points to
    ///    a file that no longer exists. Created when files are deleted
    ///    behind libgpod's back (the user, a previous botched sync,
    ///    iTunes Restore that wiped the partition). Removed from the
    ///    DB via `delete_track` (its `remove_file` step is a no-op
    ///    when the file is already gone).
    ///
    /// Both classes leave the system in an internally-inconsistent
    /// state that compounds across runs. Sweeping at sync start
    /// guarantees the diff sees a 1:1 baseline.
    ///
    /// The caller is expected to invoke this BEFORE the action-plan
    /// diff (otherwise the diff sees stale state) and AFTER
    /// `set_firewire_guid` (so subsequent `db.write()` is signed).
    /// Mutations only land on disk when the caller eventually calls
    /// `db.write()`; until then, DB removals are in-memory only and
    /// the per-orphan file deletions are independent of the DB write.
    pub fn reconcile_with_disk(&self, ipod_mount: &Path) -> ReconcileReport {
        let music_root = ipod_mount.join("iPod_Control").join("Music");

        // Set of relpaths the DB currently references (e.g.
        // `iPod_Control\Music\F38\libgpod794620.m4a`).
        let db_paths: std::collections::HashSet<String> = self
            .list_tracks_for_rebuild()
            .into_iter()
            .map(|t| t.ipod_relpath)
            .filter(|p| !p.is_empty())
            .collect();

        // Set of relpaths actually present under Music\. Same encoding
        // (Windows backslashes, relative to the mount root) so the two
        // sets are comparable.
        let disk_paths: std::collections::HashSet<String> = walkdir::WalkDir::new(&music_root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                e.path()
                    .strip_prefix(ipod_mount)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('/', "\\"))
            })
            .collect();

        // Orphans = on disk, not in DB. Delete the file directly.
        let mut orphans_deleted = 0;
        let mut orphans_failed = 0;
        for relpath in disk_paths.difference(&db_paths) {
            // `relpath` is Windows-backslash-encoded (to match `db_paths`).
            // Convert to the native separator so the join yields a real
            // filesystem path — on macOS a literal-backslash name doesn't exist,
            // so orphan deletion silently failed and files piled up.
            let full = ipod_mount.join(relpath.replace('\\', std::path::MAIN_SEPARATOR_STR));
            match std::fs::remove_file(&full) {
                Ok(()) => {
                    tracing::debug!("reconcile: deleted orphan {}", full.display());
                    orphans_deleted += 1;
                }
                Err(e) => {
                    tracing::warn!("reconcile: failed to delete orphan {}: {e}", full.display());
                    orphans_failed += 1;
                }
            }
        }

        // Dangling = in DB, file missing. Collect dbids first to avoid
        // mutation-during-iteration on the GList that
        // `list_tracks_for_rebuild` walked. Then `delete_track` each.
        let dangling_dbids: Vec<u64> = self
            .list_tracks_for_rebuild()
            .into_iter()
            .filter(|t| !t.ipod_relpath.is_empty() && !disk_paths.contains(&t.ipod_relpath))
            .map(|t| t.dbid)
            .collect();
        let mut dangling_removed = 0;
        for dbid in dangling_dbids {
            if self.delete_track(dbid).is_ok() {
                tracing::debug!("reconcile: removed dangling DB ref dbid={dbid}");
                dangling_removed += 1;
            }
        }

        ReconcileReport {
            orphans_deleted,
            orphans_failed,
            dangling_removed,
        }
    }
}

fn normalize_relpath(path: &str) -> String {
    path.trim_start_matches(['/', '\\'])
        .replace(['/', ':'], "\\")
        .to_ascii_lowercase()
}

/// Result of `OwnedDb::reconcile_with_disk`. All counts are zero on a
/// healthy iPod where DB and disk agree.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileReport {
    /// On-disk files removed because no DB track referenced them.
    pub orphans_deleted: usize,
    /// Orphans we couldn't remove (file in use, permissions, etc.).
    pub orphans_failed: usize,
    /// DB tracks removed because their referenced file was gone.
    pub dangling_removed: usize,
}

impl ReconcileReport {
    /// True when nothing was wrong; useful for the apply_loop's "log a
    /// summary line only if something happened" pattern.
    pub fn is_clean(&self) -> bool {
        self.orphans_deleted == 0 && self.orphans_failed == 0 && self.dangling_removed == 0
    }
}

impl Drop for OwnedDb {
    fn drop(&mut self) {
        unsafe {
            free_itunesdb(self.0);
        }
    }
}

#[repr(C)]
struct ItunesDbPrivateCompat {
    _mhsd5_playlists: *mut std::ffi::c_void,
    _platform: u16,
    _unk_0x22: u16,
    _id_0x24: u64,
    _lang: u16,
    _pid: u64,
    _unk_0x50: i32,
    _unk_0x54: i32,
    _audio_language: i16,
    _subtitle_language: i16,
    _unk_0xa4: i16,
    _unk_0xa6: i16,
    _unk_0xa8: i16,
    genius_cuid: *mut std::ffi::c_char,
}

unsafe fn free_itunesdb(db: *mut ffi::Itdb_iTunesDB) {
    // Our pinned libgpod frees a parsed Genius CUID twice. Take ownership of
    // that field first, then let itdb_free release the rest of the database.
    if !db.is_null() {
        let private = (*db).priv_ as *mut ItunesDbPrivateCompat;
        if !private.is_null() {
            let genius_cuid = (*private).genius_cuid;
            (*private).genius_cuid = ptr::null_mut();
            ffi::g_free(genius_cuid.cast());
        }
    }
    ffi::itdb_free(db);
}

/// Remove every track from `db`: delete each track's on-disk file (best
/// effort — a missing file is not an error, mirroring `delete_track`),
/// unlink it from every playlist, then remove + free the `Itdb_Track`.
/// Does NOT call `itdb_write`; the caller batches that once after this
/// returns. Returns the number of tracks removed.
///
/// Used by `--replace-library` (Task 11) to erase the device before
/// re-syncing the current selection from scratch. Proven sequence —
/// see `examples/wipe-tracks.rs` and the 2026-05-17 LEARNINGS entry:
/// Track DBIDs are collected before removing because removal frees the GList
/// node under an in-progress walk.
pub fn wipe_all_tracks(db: &OwnedDb) -> Result<usize> {
    let mut dbids = Vec::new();
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let track = (*node).data as *mut ffi::Itdb_Track;
            if !track.is_null() {
                dbids.push((*track).dbid);
            }
            node = (*node).next;
        }
    }
    let count = dbids.len();
    for dbid in dbids {
        db.remove_track(dbid, TrackFileDisposition::DeleteAfterCommit)?;
    }
    Ok(count)
}

/// List every playlist in the DB, in on-disk order (a playlist's position
/// is itself meaningful — the iPod firmware shows playlists in this order),
/// as `(name, is_mpl)` pairs. `is_mpl` is what callers MUST check before
/// removing or mutating anything derived from this list — the master
/// playlist is never one Classick manages. A playlist whose `name` pointer
/// is NULL (malformed/foreign DB state) reports an empty string rather than
/// dereferencing a null `CStr`.
pub fn list_playlists(db: &OwnedDb) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let pl = (*node).data as *mut ffi::Itdb_Playlist;
            if !pl.is_null() {
                let name = if (*pl).name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr((*pl).name).to_string_lossy().into_owned()
                };
                out.push((name, ffi::itdb_playlist_is_mpl(pl) != 0));
            }
            node = (*node).next;
        }
    }
    out
}

impl OwnedDb {
    pub fn playlist_kind_by_id(&self, id: u64) -> Option<PlaylistStructuralKind> {
        unsafe {
            let playlist = ffi::itdb_playlist_by_id(self.0, id);
            if playlist.is_null() {
                return None;
            }
            Some(if ffi::itdb_playlist_is_mpl(playlist) != 0 {
                PlaylistStructuralKind::Master
            } else if ffi::itdb_playlist_is_podcasts(playlist) != 0 || (*playlist).podcastflag != 0
            {
                PlaylistStructuralKind::Podcast
            } else if (*playlist).is_spl != 0 {
                PlaylistStructuralKind::Smart
            } else {
                PlaylistStructuralKind::Normal
            })
        }
    }

    pub fn normal_playlist_members_by_id(&self, id: u64) -> Result<Vec<(u64, String)>> {
        if self.playlist_kind_by_id(id) != Some(PlaylistStructuralKind::Normal) {
            return Err(anyhow!("playlist id {id} is not a normal playlist"));
        }
        unsafe {
            let playlist = ffi::itdb_playlist_by_id(self.0, id);
            let mut members = Vec::new();
            let mut node = (*playlist).members;
            while !node.is_null() {
                let track = (*node).data as *mut ffi::Itdb_Track;
                if track.is_null() {
                    return Err(anyhow!("playlist id {id} contains a null member"));
                }
                members.push(((*track).dbid as u64, read_ipod_relpath(track)));
                node = (*node).next;
            }
            Ok(members)
        }
    }
}

/// Find-or-create a Classick-managed playlist by **recorded itdb id**,
/// never by name, and make its membership exactly `dbids`, in order.
/// Returns the resulting playlist's itdb id so the caller
/// (`device_playlists::reconcile`) can persist it into
/// `managed_playlists.json` for the next run.
///
/// # Why id, not name
/// A name-based lookup (the pre-fix behavior) would find and rewrite a
/// FOREIGN playlist — one Classick never created — if its name happened to
/// collide with a desired managed-playlist name. Resolving by id closes
/// that hole: itdb ids are libgpod-assigned random 64-bit values, never
/// derived from (or colliding with) anything a foreign playlist could
/// plausibly share. See the `device_playlists` module doc for the full
/// invariant this protects (spec §3/§6: foreign playlists are never
/// modified).
///
/// # Id-assignment timing
/// Determined empirically via `examples/playlist-id-spike.rs` — libgpod
/// ships in this repo as vendored headers + prebuilt binaries, no C source
/// to read. Finding: `itdb_playlist_new` alone leaves `id == 0`; libgpod
/// assigns the real random 64-bit id inside `itdb_playlist_add`, and that
/// id is stable across `itdb_write` + a fresh reparse from disk. So this
/// function reads `(*pl).id` immediately after `itdb_playlist_add` for a
/// freshly created playlist — no post-write re-walk pass is needed to
/// learn the id.
///
/// # Resolution
/// - `recorded_id` is `Some(id)`, `itdb_playlist_by_id` resolves it to a
///   playlist in `db`, and that playlist is not the MPL: it's reused.
///   Membership is cleared and rewritten to `dbids`; the name is updated
///   in place to `name` if it changed since the last reconcile (rename
///   case — same id, new name).
/// - Otherwise (no recorded id, the id no longer resolves — e.g. the user
///   deleted the playlist — or it resolves to the MPL, which should be
///   structurally impossible but is guarded anyway): a NEW playlist is
///   created unconditionally via `itdb_playlist_new` + `itdb_playlist_add`.
///   `itdb_playlist_by_name` is deliberately never consulted here — a
///   same-named foreign playlist is left completely untouched, and the
///   desired playlist is created alongside it under the same name. Two
///   same-named playlists is a state iTunesDB tolerates fine, and is
///   strictly safer than guessing at ownership by name.
///
/// Note: this means a legacy `managed_playlists.json` record (name-only,
/// no id — pre-migration) never gets reused here even if its name still
/// matches a desired playlist: the caller passes `recorded_id: None` for
/// those, so a fresh playlist is always created and the old on-device one
/// (no longer traceable by id) is left alone — effectively becoming
/// "foreign" from Classick's perspective going forward. That's an
/// intentional one-time consequence of the migration, not a bug: it's the
/// same "never adopt by name" guarantee applied uniformly.
///
/// # Failure paths
/// - `name` contains an interior NUL byte: `Err`, nothing touched (can't
///   even form the `CString`).
/// - `itdb_playlist_new` returns NULL (OOM or a libgpod-internal failure):
///   `Err`, nothing touched.
/// - A `dbid` in `dbids` has no matching track currently in the DB (stale
///   reference — e.g. the track was removed since the manifest join that
///   produced `dbids` ran): that one reference is skipped with
///   `tracing::warn!` and the rest of the call proceeds normally. One bad
///   reference should not sink an otherwise-good playlist.
pub fn ensure_managed_playlist(
    db: &OwnedDb,
    name: &str,
    dbids: &[u64],
    recorded_id: Option<u64>,
) -> Result<u64> {
    let name_c = CString::new(name)
        .map_err(|_| anyhow!("playlist name contains interior NUL byte: {name:?}"))?;
    unsafe {
        let mut pl: *mut ffi::Itdb_Playlist = ptr::null_mut();
        if let Some(id) = recorded_id {
            if db.playlist_kind_by_id(id) == Some(PlaylistStructuralKind::Normal) {
                pl = ffi::itdb_playlist_by_id(db.as_ptr(), id);
            }
        }

        if pl.is_null() {
            let created = ffi::itdb_playlist_new(name_c.as_ptr(), 0);
            if created.is_null() {
                return Err(anyhow!("itdb_playlist_new returned NULL for {name:?}"));
            }
            ffi::itdb_playlist_add(db.as_ptr(), created, -1);
            pl = created;
        } else {
            // Reusing a recorded playlist: rename in place if the desired
            // display name changed since the last reconcile.
            let current_name = if (*pl).name.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*pl).name).to_string_lossy().into_owned()
            };
            if current_name != name {
                set_str(&mut (*pl).name, Some(name));
            }
        }

        // Build the dbid -> track pointer map once per call (a single pass
        // over the DB's track GList) rather than re-walking the full track
        // list per member via `find_track_by_dbid` — O(tracks + members)
        // instead of O(tracks * members).
        let mut track_by_dbid: std::collections::HashMap<u64, *mut ffi::Itdb_Track> =
            std::collections::HashMap::new();
        let mut tnode = (*db.as_ptr()).tracks;
        while !tnode.is_null() {
            let t = (*tnode).data as *mut ffi::Itdb_Track;
            if !t.is_null() {
                track_by_dbid.insert((*t).dbid as u64, t);
            }
            tnode = (*tnode).next;
        }

        // Collect current members into a Vec before removing any — mutating
        // while walking the GList frees nodes out from under the walk (same
        // hazard `wipe_all_tracks` documents for the DB's tracks GList).
        let mut members: Vec<*mut ffi::Itdb_Track> = Vec::new();
        let mut mnode = (*pl).members;
        while !mnode.is_null() {
            let t = (*mnode).data as *mut ffi::Itdb_Track;
            if !t.is_null() {
                members.push(t);
            }
            mnode = (*mnode).next;
        }
        for t in members {
            ffi::itdb_playlist_remove_track(pl, t);
        }

        for dbid in dbids {
            match track_by_dbid.get(dbid) {
                Some(&t) => {
                    ffi::itdb_playlist_add_track(pl, t, -1);
                }
                None => {
                    tracing::warn!(
                        "ensure_managed_playlist({name:?}): dbid {dbid} not found in DB; skipping"
                    );
                }
            }
        }

        Ok((*pl).id as u64)
    }
}

/// Remove the normal playlist with itdb id `id`, if present. No name-based
/// removal exists because only a device-authoritative exact ID grants
/// mutation authority.
///
/// # Failure paths
/// - No playlist with itdb id `id` exists: `Ok(false)` — same idempotent
///   "already gone" semantics as `remove_playlist_by_name`.
/// - `id` resolves to master, podcast, or smart: `Err`, nothing touched.
pub fn remove_playlist_by_id(db: &OwnedDb, id: u64) -> Result<bool> {
    unsafe {
        let pl = ffi::itdb_playlist_by_id(db.as_ptr(), id);
        if pl.is_null() {
            return Ok(false);
        }
        if db.playlist_kind_by_id(id) != Some(PlaylistStructuralKind::Normal) {
            return Err(anyhow!(
                "refusing to remove playlist id {id}: target is not a normal playlist"
            ));
        }
        ffi::itdb_playlist_remove(pl);
    }
    Ok(true)
}

/// File name (under `iPod_Control\iTunes\`) we copy `iTunesDB` to
/// before each sync session, providing a known-good restore point if
/// the sync crashes mid-write and corrupts the live DB.
///
/// Strategy:
///   * Empirically, libgpod's `itdb_write` uses MSVCRT `rename` which
///     on Windows 10+ resolves to `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`
///     — atomic at the FS layer.
///   * BUT the path isn't 100% audited (libgpod is many years of C),
///     and we've already had to patch one Windows-rename quirk for
///     `Play Counts` → `Play Counts.bak`. Defense in depth.
///   * One backup per sync session (NOT per checkpoint) is enough —
///     each new session restores from a clean prior-session state.
///   * Recovery is manual today: if `iTunesDB` is corrupted, copy this
///     file over it. A future `--restore-db-backup` subcommand can
///     automate that; for now the LEARNINGS entry documents the steps.
// Mirrors `crate::PROJECT_DIR`; can't be derived in a const context without
// pulling in `const_str` for compile-time concat. Update both if the project
// identifier ever changes.
pub const ITUNESDB_BACKUP_NAME: &str = "iTunesDB.classick-backup";

/// Make a session-start backup of `iTunesDB` at
/// `iPod_Control\iTunes\iTunesDB.classick-backup`. No-op when there
/// is no iTunesDB to copy (fresh-from-iTunes-Restore device — there's
/// nothing to lose yet). Returns `Ok(())` on success or if the source
/// is missing; logs a warning and returns `Ok(())` on copy failure so
/// a backup write hiccup doesn't block a healthy sync.
pub fn backup_itunesdb(ipod_mount: &Path) -> std::io::Result<()> {
    let src = crate::ipod::layout::itunes_db_path(ipod_mount);
    if !src.exists() {
        return Ok(());
    }
    let dst = ipod_mount
        .join("iPod_Control")
        .join("iTunes")
        .join(ITUNESDB_BACKUP_NAME);
    // Copy via a `.tmp` intermediate + rename so an interrupted backup
    // copy doesn't itself corrupt the prior backup. On Windows 10+ the
    // rename is MoveFileExW(REPLACE_EXISTING) — atomic.
    let tmp = dst.with_extension("classick-backup.tmp");
    match std::fs::copy(&src, &tmp) {
        Ok(_) => {
            if let Err(e) = std::fs::rename(&tmp, &dst) {
                tracing::warn!(
                    "backup_itunesdb: rename {} -> {} failed: {e}",
                    tmp.display(),
                    dst.display()
                );
                let _ = std::fs::remove_file(&tmp);
            }
        }
        Err(e) => {
            tracing::warn!(
                "backup_itunesdb: copy {} -> {} failed: {e}; sync will proceed without a fresh backup",
                src.display(),
                tmp.display()
            );
        }
    }
    Ok(())
}

/// File name (under `iPod_Control\iTunes\`) a corrupt live `iTunesDB` is
/// renamed to by `restore_itunesdb_from_backup` before the backup is
/// copied into its place. Single slot — a second corruption in a later
/// session overwrites whatever was set aside before, on the theory that
/// only the most recent corrupt copy is worth keeping for inspection.
pub const ITUNESDB_CORRUPT_ASIDE_NAME: &str = "iTunesDB.corrupt";

/// Parse `path` with libgpod and immediately discard the result. Used to
/// validate a candidate iTunesDB (e.g. the session backup) without
/// wiring it up as the live, in-memory `OwnedDb` for the rest of the run.
fn parse_check(path: &Path) -> Result<()> {
    let path_c = path_to_cstring(path)?;
    unsafe {
        let mut err: *mut ffi::GError = ptr::null_mut();
        let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
        if db.is_null() {
            return Err(gerror_to_anyhow("itdb_parse_file", err));
        }
        ffi::itdb_free(db);
    }
    Ok(())
}

/// Restore `iTunesDB` from the session backup (`iTunesDB.classick-backup`)
/// after detecting the live DB won't parse.
///
/// Order matters: the backup is validated with libgpod BEFORE anything on
/// disk is touched. If the backup is missing or itself unparseable, this
/// returns `Err` and the live (corrupt) DB is left exactly as found — we
/// never destroy the only copy of a DB on the strength of an unverified
/// replacement. Only once the backup is confirmed good do we proceed with
/// the restore sequence.
///
/// The sequence is designed to minimize the crash window where no live DB
/// exists (required by device-detection logic):
///   1. Copy backup → `.tmp` (same pattern as `backup_itunesdb`)
///   2. Rename live DB → `iTunesDB.corrupt` (replace-existing)
///   3. Rename `.tmp` → live `iTunesDB`
///
/// If step 1 or 3 fails, clean up the `.tmp` file. If a crash occurs after
/// step 2 but before step 3, the fully-validated `.tmp` and intact backup
/// sit next to the `.corrupt` aside, allowing safe manual recovery on the
/// next run.
pub fn restore_itunesdb_from_backup(ipod_mount: &Path) -> Result<()> {
    let backup = ipod_mount
        .join("iPod_Control")
        .join("iTunes")
        .join(ITUNESDB_BACKUP_NAME);
    if !backup.exists() {
        return Err(anyhow!(
            "no session backup found at {} to restore from",
            backup.display()
        ));
    }
    parse_check(&backup).map_err(|e| {
        anyhow!(
            "session backup at {} does not parse: {e:#}",
            backup.display()
        )
    })?;

    let live = crate::ipod::layout::itunes_db_path(ipod_mount);
    let aside = ipod_mount
        .join("iPod_Control")
        .join("iTunes")
        .join(ITUNESDB_CORRUPT_ASIDE_NAME);
    let tmp = live.with_extension("tmp");

    // Step 1: copy backup → tmp (same tmp naming pattern already used).
    std::fs::copy(&backup, &tmp).with_context(|| {
        format!(
            "failed to copy backup {} -> {}",
            backup.display(),
            tmp.display()
        )
    })?;

    // Step 2: rename live DB → corrupt aside (replace-existing).
    if live.exists() {
        std::fs::rename(&live, &aside).with_context(|| {
            format!(
                "failed to set aside corrupt DB {} -> {}",
                live.display(),
                aside.display()
            )
        })?;
    }

    // Step 3: rename tmp → live (atomic on POSIX + Windows 10+).
    if let Err(e) = std::fs::rename(&tmp, &live) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| {
            format!(
                "failed to rename restored DB {} -> {}",
                tmp.display(),
                live.display()
            )
        });
    }

    Ok(())
}

/// Open the live iTunesDB, silently self-healing from the session backup
/// if the live DB fails to parse (e.g. a crash mid-write left it
/// truncated/corrupt). `on_restore` fires exactly once, only when a
/// restore actually happened and the re-open succeeded — callers use it
/// to log the event and record it for the sync history, per the
/// invisible-trust design (no prompts; the user just sees a working sync).
///
/// If the live DB fails to parse AND the restore attempt also fails
/// (backup missing or itself unparseable), this returns the ORIGINAL
/// `OwnedDb::open` error — not the restore error — wrapped with context
/// naming both manual remedies (`--rebuild-manifest`, `--restore-db-backup`)
/// so the user isn't left with a dead end.
pub fn open_with_auto_restore(ipod_mount: &Path, on_restore: impl FnOnce()) -> Result<OwnedDb> {
    match OwnedDb::open(ipod_mount) {
        Ok(db) => Ok(db),
        Err(open_err) => {
            if restore_itunesdb_from_backup(ipod_mount).is_err() {
                return Err(open_err.context(
                    "iTunesDB failed to parse and could not be auto-restored from backup; \
                     try `--rebuild-manifest` to rebuild from the iPod's on-disk state, or \
                     `--restore-db-backup` to restore the session backup manually",
                ));
            }
            match OwnedDb::open(ipod_mount) {
                Ok(db) => {
                    on_restore();
                    Ok(db)
                }
                Err(reopen_err) => Err(reopen_err.context(
                    "restored iTunesDB from backup but it failed to re-open; try \
                     `--rebuild-manifest` to rebuild from the iPod's on-disk state, or \
                     `--restore-db-backup` to restore the session backup manually",
                )),
            }
        }
    }
}

/// Read the iPod's user-set name (the master playlist's `name` field
/// in iTunesDB). Returns `None` if the DB can't be parsed, the master
/// playlist is missing, or the name is blank.
///
/// This opens the full iTunesDB which can take 100ms–1s on large
/// libraries — callers in the daemon's async loop should wrap this in
/// `tokio::task::spawn_blocking` to avoid stalling the runtime.
pub fn read_ipod_name(ipod_mount: &Path) -> Option<String> {
    let db = OwnedDb::open(ipod_mount).ok()?;
    unsafe {
        let master = ffi::itdb_playlist_mpl(db.as_ptr());
        if master.is_null() {
            return None;
        }
        let name_ptr = (*master).name;
        if name_ptr.is_null() {
            return None;
        }
        let s = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
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
    // Duration (ms). Without this the iPod shows -0:00 and may cut tracks short;
    // libgpod doesn't backfill it from afconvert's ALAC container on macOS.
    if let Some(d) = tags.duration_ms {
        (*track).tracklen = d as i32;
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
    CString::new(s).map_err(|_| anyhow!("path contains interior NUL byte: {}", p.display()))
}

unsafe fn gerror_to_anyhow(api: &str, err: *mut ffi::GError) -> anyhow::Error {
    if err.is_null() {
        return anyhow!("{api} failed (no error detail)");
    }
    let msg = CStr::from_ptr((*err).message)
        .to_string_lossy()
        .into_owned();
    ffi::g_error_free(err);
    anyhow!("{api} failed: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn artwork_mount(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mount = std::env::temp_dir().join(format!(
            "classick-db-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();

        let device_id = crate::device::DeviceId::parse("000A27002138B0A8").unwrap();
        let profile_id = crate::ipod::CapabilityProfileId::parse("classic-late-2009-v1").unwrap();
        let profile = crate::ipod::resolve_validated_capability_profile(&profile_id)
            .unwrap()
            .unwrap();
        let projection = crate::ipod::project_sysinfo_extended(&device_id, &profile).unwrap();
        std::fs::write(
            mount.join("iPod_Control/Device/SysInfoExtended"),
            projection.bytes(),
        )
        .unwrap();

        unsafe {
            let raw = ffi::itdb_new();
            assert!(!raw.is_null());
            let mount_c = path_to_cstring(&mount).unwrap();
            ffi::itdb_set_mountpoint(raw, mount_c.as_ptr());
            crate::ipod::device::set_firewire_guid((*raw).device, "0x000A27002138B0A8").unwrap();
            crate::ipod::device::set_model_num((*raw).device, "MC293").unwrap();
            let master = ffi::itdb_playlist_new(c"iPod".as_ptr(), 0);
            assert!(!master.is_null());
            ffi::itdb_playlist_set_mpl(master);
            ffi::itdb_playlist_add(raw, master, -1);
            let mut error = ptr::null_mut();
            assert_ne!(ffi::itdb_write(raw, &mut error), 0);
            free_itunesdb(raw);
        }

        mount
    }

    fn jpeg_fixture() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    }

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

    #[test]
    fn libgpod_string_boundary_preserves_utf8_metadata() {
        let value = "日本語 🎵 – I’m So Frëe";
        let mut slot = ptr::null_mut();

        unsafe {
            set_str(&mut slot, Some(value));
            assert!(!slot.is_null());
            assert_eq!(CStr::from_ptr(slot).to_str().unwrap(), value);
            ffi::g_free(slot.cast());
        }
    }

    #[test]
    fn drop_frees_a_parsed_genius_identifier_exactly_once() {
        unsafe {
            let raw = ffi::itdb_new();
            assert!(!raw.is_null());
            let private = (*raw).priv_ as *mut ItunesDbPrivateCompat;
            assert!(!private.is_null());
            (*private).genius_cuid = ffi::g_strdup(c"0123456789ABCDEF0123456789ABCDEF".as_ptr());

            drop(OwnedDb(raw));
        }
    }

    #[test]
    fn open_loads_persisted_track_artwork_from_the_mount() {
        let mount = artwork_mount("persisted-artwork");
        let media = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bare.m4a");
        let art = jpeg_fixture();

        let db = OwnedDb::open(&mount).unwrap();
        unsafe {
            crate::ipod::device::set_firewire_guid((*db.as_ptr()).device, "0x000A27002138B0A8")
                .unwrap();
            crate::ipod::device::set_model_num((*db.as_ptr()).device, "MC293").unwrap();
        }
        let handle = db
            .add_track_with_file_strict(&media, &Tags::default(), Some(&art))
            .unwrap();
        db.write().unwrap();
        drop(db);

        let reopened = OwnedDb::open(&mount).unwrap();
        let signals = reopened.track_artwork_signals(handle.dbid).unwrap();
        assert!(signals.has_artwork);
        assert!(signals.has_thumbnail);
        assert!(signals.has_thumbnails);
        assert!(signals.decoded_thumbnail);
    }

    #[test]
    fn staged_add_journals_destination_before_moving_media() {
        let mount = artwork_mount("move-staged-media");
        let staged = mount.join("iPod_Control/classick/pending/7.staged/0.m4a");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bare.m4a"),
            &staged,
        )
        .unwrap();

        let db = OwnedDb::open(&mount).unwrap();
        let mut journaled_destination = None;
        let handle = db
            .add_track_with_staged_file_strict(&staged, &Tags::default(), None, |destination| {
                assert!(staged.exists());
                assert!(!destination.exists());
                journaled_destination = Some(destination.to_path_buf());
                Ok(())
            })
            .unwrap();

        let destination = journaled_destination.unwrap();
        assert!(!staged.exists());
        assert!(destination.exists());
        assert_eq!(
            destination,
            mount.join(
                handle
                    .ipod_relpath
                    .replace('\\', std::path::MAIN_SEPARATOR_STR)
            )
        );
    }
}
