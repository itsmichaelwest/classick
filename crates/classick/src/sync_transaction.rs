mod rollback;

use anyhow::{bail, Context, Result};
use rollback::is_managed_artwork_output;
pub use rollback::{FailurePoint, RollbackSnapshot};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckpointResult {
    pub published_albums: usize,
    pub published_tracks: usize,
    pub host_cache_warning: Option<String>,
}

pub struct CheckpointCoordinator<'a> {
    pub mount: &'a Path,
    pub serial: &'a str,
    pub manifest_store: &'a crate::manifest_store::ManifestStore,
    pub artwork_cache: crate::artwork_cache::ArtworkCache,
}

impl CheckpointCoordinator<'_> {
    pub fn publish(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        manifest: &mut crate::manifest::Manifest,
        progress: &crate::progress::Progress,
    ) -> Result<CheckpointResult> {
        use crate::pending_session::PendingPhase;

        self.validate_journal(journal)?;
        let store = crate::pending_session::PendingSessionStore::new(self.mount);
        store.save(journal)?;

        if journal.phase == PendingPhase::Staging {
            self.prepare_all_artwork(journal, manifest)?;
            journal.phase = PendingPhase::ReadyToPublish;
            store.save(journal)?;
        }

        let snapshot = self.ensure_snapshot(&store, journal)?;
        if journal.phase == PendingPhase::ReadyToPublish {
            self.resume_or_publish_database(journal, manifest, &store, &snapshot, progress)?;
        }

        let candidate = journal
            .candidate_manifest
            .clone()
            .context("verified transaction has no candidate manifest")?;
        if journal.phase == PendingPhase::DatabaseVerified {
            let reopened =
                crate::ipod::db::OwnedDb::open(self.mount).context("reopen verified iTunesDB")?;
            if let Err(error) = self.verify_candidate(&reopened, &candidate) {
                drop(reopened);
                self.rollback_to_ready(journal, &store, &snapshot, error)?;
                return self.publish(journal, manifest, progress);
            }
            drop(reopened);

            progress.log("Publishing portable device manifest".to_string());
            let outcome = match self.manifest_store.publish_runtime(&candidate) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.rollback_to_ready(journal, &store, &snapshot, error)?;
                    bail!("device manifest publication failed; database and artwork restored");
                }
            };
            *manifest = candidate;
            journal.phase = PendingPhase::DeviceManifestPublished;
            store.save(journal)?;
            return self.finish_cleanup(journal, &store, outcome.host_cache_warning, progress);
        }

        if journal.phase == PendingPhase::DeviceManifestPublished {
            *manifest = candidate;
            return self.finish_cleanup(journal, &store, None, progress);
        }

        if journal.phase == PendingPhase::CleanupComplete {
            store.remove(journal.session_id)?;
            remove_dir_if_present(&store.snapshot_dir(journal.session_id))?;
            return Ok(self.result(journal, None));
        }
        bail!("unsupported pending phase {:?}", journal.phase)
    }

    fn validate_journal(&self, journal: &crate::pending_session::PendingSession) -> Result<()> {
        journal.validate()?;
        if crate::device_state::sanitize_serial(&journal.serial)
            != crate::device_state::sanitize_serial(self.serial)
        {
            bail!(
                "pending-session serial {:?} does not match connected device {:?}",
                journal.serial,
                self.serial
            );
        }
        Ok(())
    }

    fn prepare_all_artwork(
        &self,
        journal: &crate::pending_session::PendingSession,
        manifest: &crate::manifest::Manifest,
    ) -> Result<()> {
        for entry in manifest.tracks.iter().filter(|entry| entry.source_known) {
            self.artwork_cache
                .load_for_source(&entry.source_path)
                .with_context(|| format!("prepare artwork for {}", entry.source_path.display()))?;
        }
        for staged in &journal.staged_files {
            if let Some(hash) = staged.artwork_hash.as_deref() {
                self.artwork_cache.load_hash(hash).with_context(|| {
                    format!("prepare staged artwork for {}", staged.source.display())
                })?;
            } else {
                self.artwork_cache
                    .load_for_source(&staged.source)
                    .with_context(|| {
                        format!("prepare staged artwork for {}", staged.source.display())
                    })?;
            }
        }
        Ok(())
    }

    fn ensure_snapshot(
        &self,
        store: &crate::pending_session::PendingSessionStore,
        journal: &crate::pending_session::PendingSession,
    ) -> Result<RollbackSnapshot> {
        let path = store.snapshot_dir(journal.session_id);
        if path.exists() {
            return RollbackSnapshot::open(&path).context("validate existing rollback snapshot");
        }
        RollbackSnapshot::create(self.mount, &path).context("create complete DB/artwork rollback")
    }

    fn resume_or_publish_database(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        manifest: &crate::manifest::Manifest,
        store: &crate::pending_session::PendingSessionStore,
        snapshot: &RollbackSnapshot,
        progress: &crate::progress::Progress,
    ) -> Result<()> {
        use crate::pending_session::PendingPhase;

        if let Some(candidate) = journal.candidate_manifest.as_ref() {
            let live = crate::ipod::db::OwnedDb::open(self.mount)
                .context("inspect ambiguous live iTunesDB")?;
            if self.verify_candidate(&live, candidate).is_ok() {
                journal.phase = PendingPhase::DatabaseVerified;
                store.save(journal)?;
                return Ok(());
            }
            drop(live);
            snapshot.restore(self.mount)?;
            self.cleanup_after_rollback(journal)?;
            reset_staged_publication(journal);
            store.save(journal)?;
        }

        progress.log("Publishing staged albums and artwork".to_string());
        let db = crate::ipod::db::OwnedDb::open(self.mount)
            .context("open fresh publication database")?;
        let mut candidate = manifest.clone();
        for obsolete in &journal.obsolete_files {
            db.unlink_track_keep_file(obsolete.prior_dbid)?;
            candidate
                .tracks
                .retain(|entry| entry.ipod_dbid != obsolete.prior_dbid);
        }

        for index in journal.publication_indices()? {
            let staged = &journal.staged_files[index];
            let art = match staged.artwork_hash.as_deref() {
                Some(hash) => Some(self.artwork_cache.load_hash(hash)?),
                None => self.artwork_cache.load_for_source(&staged.source)?,
            };
            let handle =
                db.add_track_with_file_strict(&staged.pending_path, &staged.tags, art.as_deref())?;
            let final_path = self.mount.join(
                handle
                    .ipod_relpath
                    .replace('\\', std::path::MAIN_SEPARATOR_STR),
            );
            let staged = &mut journal.staged_files[index];
            staged.dbid = handle.dbid;
            staged.final_ipod_path = Some(final_path);
            let mut entry = staged
                .candidate_entry
                .clone()
                .context("staged file has no candidate manifest entry")?;
            candidate.tracks.retain(|existing| {
                existing.ipod_dbid != entry.ipod_dbid
                    && !(entry.source_known
                        && existing.source_known
                        && existing.source_path == entry.source_path)
            });
            entry.ipod_dbid = handle.dbid;
            entry.ipod_relpath = handle.ipod_relpath;
            staged.candidate_entry = Some(entry.clone());
            candidate.tracks.push(entry);
            journal.candidate_manifest = Some(candidate.clone());
            store
                .save(journal)
                .context("journal copied iPod path before DB write")?;
        }

        for entry in candidate.tracks.iter().filter(|entry| entry.source_known) {
            let art = self.artwork_cache.load_for_source(&entry.source_path)?;
            db.set_track_artwork(entry.ipod_dbid, art.as_deref())?;
        }
        journal.candidate_manifest = Some(candidate.clone());
        store
            .save(journal)
            .context("journal candidate before DB write")?;
        remove_stale_artwork_outputs(self.mount)?;

        if let Err(error) = db.write().context("write candidate iTunesDB and artwork") {
            drop(db);
            self.rollback_to_ready(journal, store, snapshot, error)?;
            bail!("database publication failed; database and artwork restored");
        }
        drop(db);
        let reopened =
            match crate::ipod::db::OwnedDb::open(self.mount).context("reopen candidate iTunesDB") {
                Ok(db) => db,
                Err(error) => {
                    self.rollback_to_ready(journal, store, snapshot, error)?;
                    bail!("database verification failed; database and artwork restored");
                }
            };
        if let Err(error) = self.verify_candidate(&reopened, &candidate) {
            drop(reopened);
            self.rollback_to_ready(journal, store, snapshot, error)?;
            bail!("database verification failed; database and artwork restored");
        }
        journal.phase = PendingPhase::DatabaseVerified;
        store.save(journal)?;
        Ok(())
    }

    fn verify_candidate(
        &self,
        db: &crate::ipod::db::OwnedDb,
        candidate: &crate::manifest::Manifest,
    ) -> Result<()> {
        for entry in &candidate.tracks {
            let expects_artwork = entry.source_known
                && self
                    .artwork_cache
                    .load_for_source(&entry.source_path)?
                    .is_some();
            db.verify_track(entry.ipod_dbid, &entry.ipod_relpath, expects_artwork)?;
        }
        Ok(())
    }

    fn rollback_to_ready(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
        snapshot: &RollbackSnapshot,
        cause: anyhow::Error,
    ) -> Result<()> {
        snapshot
            .restore(self.mount)
            .with_context(|| format!("restore rollback after {cause:#}"))?;
        self.cleanup_after_rollback(journal)?;
        reset_staged_publication(journal);
        journal.phase = crate::pending_session::PendingPhase::ReadyToPublish;
        store.save(journal)
    }

    fn cleanup_after_rollback(
        &self,
        journal: &crate::pending_session::PendingSession,
    ) -> Result<()> {
        let restored =
            crate::ipod::db::OwnedDb::open(self.mount).context("open restored iTunesDB")?;
        let referenced = restored
            .referenced_paths(self.mount)
            .into_iter()
            .collect::<crate::pending_session::ReferencedPaths>();
        crate::pending_session::cleanup_unreferenced_staged_files(journal, &referenced)
    }

    fn finish_cleanup(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
        host_cache_warning: Option<String>,
        progress: &crate::progress::Progress,
    ) -> Result<CheckpointResult> {
        progress.log("Cleaning staged and obsolete files".to_string());
        let db = crate::ipod::db::OwnedDb::open(self.mount)
            .context("open published database for cleanup")?;
        let referenced = db
            .referenced_paths(self.mount)
            .into_iter()
            .collect::<crate::pending_session::ReferencedPaths>();
        crate::pending_session::cleanup_unreferenced_staged_files(journal, &referenced)?;
        for obsolete in &journal.obsolete_files {
            if !referenced.contains(&obsolete.path) {
                remove_file_if_present(&obsolete.path)?;
            }
        }
        journal.phase = crate::pending_session::PendingPhase::CleanupComplete;
        store.save(journal)?;
        let result = self.result(journal, host_cache_warning);
        store.remove(journal.session_id)?;
        remove_dir_if_present(&store.snapshot_dir(journal.session_id))?;
        Ok(result)
    }

    fn result(
        &self,
        journal: &crate::pending_session::PendingSession,
        host_cache_warning: Option<String>,
    ) -> CheckpointResult {
        CheckpointResult {
            published_albums: journal.albums.len(),
            published_tracks: journal.staged_files.len(),
            host_cache_warning,
        }
    }
}

fn reset_staged_publication(journal: &mut crate::pending_session::PendingSession) {
    for staged in &mut journal.staged_files {
        staged.dbid = 0;
        staged.final_ipod_path = None;
    }
    journal.candidate_manifest = None;
}

fn remove_stale_artwork_outputs(mount: &Path) -> Result<()> {
    let artwork = mount.join("iPod_Control").join("Artwork");
    if !artwork.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&artwork)? {
        let path = entry?.path();
        if is_managed_artwork_output(&path) {
            remove_file_if_present(&path)?;
        }
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn remove_dir_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::AtomicFileWriter;
    use crate::manifest::Manifest;
    use crate::manifest_store::ManifestStore;
    use crate::pending_session::PendingPhase;
    use crate::pending_session::{PendingSession, PendingSessionStore};
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::ptr;

    #[test]
    fn publication_phase_order_is_total() {
        assert!(PendingPhase::Staging < PendingPhase::ReadyToPublish);
        assert!(PendingPhase::ReadyToPublish < PendingPhase::DatabaseVerified);
        assert!(PendingPhase::DatabaseVerified < PendingPhase::DeviceManifestPublished);
        assert!(PendingPhase::DeviceManifestPublished < PendingPhase::CleanupComplete);
    }

    #[test]
    fn rollback_is_required_at_each_authoritative_failure_boundary() {
        assert!(FailurePoint::ArtworkPreparation.requires_rollback());
        assert!(FailurePoint::DatabaseWrite.requires_rollback());
        assert!(FailurePoint::DatabaseVerification.requires_rollback());
        assert!(FailurePoint::DeviceManifest.requires_rollback());
        assert!(!FailurePoint::HostCache.requires_rollback());
    }

    fn temp_mount() -> std::path::PathBuf {
        let mount = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("transaction-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Artwork")).unwrap();
        mount
    }

    fn write_valid_itunesdb(mount: &Path) {
        unsafe {
            let db = crate::ffi::itdb_new();
            assert!(!db.is_null());
            let mount = CString::new(mount.to_str().unwrap()).unwrap();
            crate::ffi::itdb_set_mountpoint(db, mount.as_ptr());
            let title = CString::new("iPod").unwrap();
            let playlist = crate::ffi::itdb_playlist_new(title.as_ptr(), 0);
            crate::ffi::itdb_playlist_set_mpl(playlist);
            crate::ffi::itdb_playlist_add(db, playlist, -1);
            let mut error: *mut crate::ffi::GError = ptr::null_mut();
            assert_ne!(crate::ffi::itdb_write(db, &mut error), 0);
            crate::ffi::itdb_free(db);
        }
    }

    fn coordinator_fixture(
        label: &str,
    ) -> (
        PathBuf,
        PathBuf,
        ManifestStore,
        crate::artwork_cache::ArtworkCache,
        Manifest,
    ) {
        let mount =
            temp_mount().with_file_name(format!("transaction-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
        write_valid_itunesdb(&mount);
        let host = mount.with_file_name(format!("transaction-{label}-host"));
        let _ = std::fs::remove_dir_all(&host);
        let source = host.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let store = ManifestStore::new(
            mount.clone(),
            "SERIAL".into(),
            host.join("manifest.json"),
            host.join("legacy.json"),
            AtomicFileWriter::new(),
        );
        let cache = crate::artwork_cache::ArtworkCache::new(host.join("artwork"));
        let mut manifest = Manifest::empty();
        manifest.version = 2;
        manifest.ipod_serial = Some("SERIAL".into());
        manifest.last_source_root = Some(source);
        (mount, host, store, cache, manifest)
    }

    #[test]
    fn complete_snapshot_restores_db_and_art_without_touching_foreign_files() {
        let mount = temp_mount();
        let db = mount.join("iPod_Control/iTunes/iTunesDB");
        let artwork_db = mount.join("iPod_Control/Artwork/ArtworkDB");
        let ithmb = mount.join("iPod_Control/Artwork/F1069_1.ithmb");
        let foreign = mount.join("iPod_Control/Artwork/notes.txt");
        std::fs::write(&db, b"old db").unwrap();
        std::fs::write(&artwork_db, b"old art db").unwrap();
        std::fs::write(&ithmb, b"old thumbnails").unwrap();
        std::fs::write(&foreign, b"foreign").unwrap();

        let snapshot_dir = mount.join("iPod_Control/classick/pending/1.snapshot");
        let snapshot = RollbackSnapshot::create(&mount, &snapshot_dir).unwrap();
        std::fs::write(&db, b"broken db").unwrap();
        std::fs::remove_file(&ithmb).unwrap();
        std::fs::write(mount.join("iPod_Control/Artwork/F1055_1.ithmb"), b"new").unwrap();
        snapshot.restore(&mount).unwrap();

        assert_eq!(std::fs::read(db).unwrap(), b"old db");
        assert_eq!(std::fs::read(artwork_db).unwrap(), b"old art db");
        assert_eq!(std::fs::read(ithmb).unwrap(), b"old thumbnails");
        assert_eq!(std::fs::read(foreign).unwrap(), b"foreign");
        assert!(!mount.join("iPod_Control/Artwork/F1055_1.ithmb").exists());
    }

    #[test]
    fn coordinator_publishes_in_order_and_removes_journal_last() {
        let (mount, _host, store, cache, mut manifest) = coordinator_fixture("publish");
        let mut journal = PendingSession::new(11, "SERIAL", Vec::new());
        let journal_store = PendingSessionStore::new(&mount);
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: "SERIAL",
            manifest_store: &store,
            artwork_cache: cache,
        };

        let result = coordinator
            .publish(&mut journal, &mut manifest, &progress)
            .unwrap();
        progress.finish(true).unwrap();

        assert_eq!(journal.phase, PendingPhase::CleanupComplete);
        assert_eq!(result.published_tracks, 0);
        assert!(crate::device_state::portable_manifest_path(&mount).exists());
        assert!(!journal_store.path(11).exists());
        assert!(!journal_store.snapshot_dir(11).exists());
    }

    #[test]
    fn device_manifest_failure_restores_snapshot_and_keeps_ready_journal() {
        let (mount, _host, store, cache, mut manifest) = coordinator_fixture("rollback");
        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(12)).unwrap();
        let original_db = std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap();
        let mut journal = PendingSession::new(12, "SERIAL", Vec::new());
        journal.phase = PendingPhase::DatabaseVerified;
        journal.candidate_manifest = Some(manifest.clone());
        journal_store.save(&journal).unwrap();
        let manifest_path = crate::device_state::portable_manifest_path(&mount);
        std::fs::create_dir_all(&manifest_path).unwrap();
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: "SERIAL",
            manifest_store: &store,
            artwork_cache: cache,
        };

        assert!(coordinator
            .publish(&mut journal, &mut manifest, &progress)
            .is_err());
        progress.finish(false).unwrap();

        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert!(journal_store.path(12).exists());
        assert_eq!(
            std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap(),
            original_db
        );
        snapshot.validate().unwrap();
    }
}
