mod rollback;

use anyhow::{bail, Context, Result};
use rollback::is_managed_artwork_output;
pub use rollback::{FailurePoint, RollbackSnapshot};
use std::path::{Path, PathBuf};

pub type DesiredPlaylist = (String, String, Vec<PathBuf>);

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions<'a> {
    pub desired_playlists: Option<&'a [DesiredPlaylist]>,
    pub playlist_state_root: Option<&'a Path>,
    pub device_identity: Option<&'a crate::ipod::device::LibgpodIdentity>,
}

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
        self.publish_with_options(journal, manifest, progress, PublishOptions::default())
    }

    pub fn recover_pending_with_options(
        &self,
        manifest: &mut crate::manifest::Manifest,
        progress: &crate::progress::Progress,
        options: PublishOptions<'_>,
    ) -> Result<Vec<CheckpointResult>> {
        let store = crate::pending_session::PendingSessionStore::new(self.mount);
        let discovery = store.discover(self.serial)?;
        let mut recovered = Vec::with_capacity(discovery.sessions.len());
        for mut journal in discovery.sessions {
            progress.log(format!(
                "Recovering interrupted sync session {} from {:?}",
                journal.session_id, journal.phase
            ));
            let result = if journal.phase == crate::pending_session::PendingPhase::Staging {
                self.abandon_interrupted_staging(&mut journal, &store)?
            } else {
                self.publish_with_options(&mut journal, manifest, progress, options)?
            };
            recovered.push(result);
        }
        if !discovery.rejected.is_empty() {
            let details = discovery
                .rejected
                .iter()
                .map(|rejected| format!("{}: {}", rejected.path.display(), rejected.reason))
                .collect::<Vec<_>>()
                .join("; ");
            bail!(
                "unsafe pending-session journal(s) must be resolved before a fresh sync: {details}"
            );
        }
        Ok(recovered)
    }

    pub fn publish_with_options(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        manifest: &mut crate::manifest::Manifest,
        progress: &crate::progress::Progress,
        options: PublishOptions<'_>,
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
            self.resume_or_publish_database(
                journal, manifest, &store, &snapshot, progress, options,
            )?;
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
                self.rollback_to_ready(journal, &store, &snapshot, options, error)?;
                return self.publish_with_options(journal, manifest, progress, options);
            }
            drop(reopened);

            progress.log("Publishing portable device manifest".to_string());
            let outcome = match self.manifest_store.publish_runtime(&candidate) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.rollback_to_ready(journal, &store, &snapshot, options, error)?;
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
        if journal.serial != self.serial {
            bail!(
                "pending-session serial {:?} does not exactly match connected device {:?}",
                journal.serial,
                self.serial
            );
        }
        Ok(())
    }

    fn abandon_interrupted_staging(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<CheckpointResult> {
        self.validate_journal(journal)?;
        let live = crate::ipod::db::OwnedDb::open(self.mount)
            .context("inspect live iTunesDB before abandoning interrupted staging")?;
        let referenced = live
            .referenced_paths(self.mount)
            .into_iter()
            .collect::<crate::pending_session::ReferencedPaths>();
        crate::pending_session::cleanup_unreferenced_staged_files(journal, &referenced)?;
        journal.phase = crate::pending_session::PendingPhase::CleanupComplete;
        store.save(journal)?;
        let result = self.result(journal, None);
        store.remove(journal.session_id)?;
        remove_dir_if_present(&store.snapshot_dir(journal.session_id))?;
        Ok(result)
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
        options: PublishOptions<'_>,
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
            self.restore_managed_playlist_record(journal, options)?;
            self.cleanup_after_rollback(journal)?;
            reset_staged_publication(journal);
            store.save(journal)?;
        }

        progress.log("Publishing staged albums and artwork".to_string());
        let db = crate::ipod::db::OwnedDb::open(self.mount)
            .context("open fresh publication database")?;
        apply_device_identity(&db, options.device_identity)?;
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

        if options.desired_playlists.is_some() {
            self.ensure_managed_playlist_record_snapshot(journal, store, options)?;
        }
        let preparation = (|| -> Result<()> {
            if let Some(desired) = options.desired_playlists {
                match options.playlist_state_root {
                    Some(root) => crate::apply_loop::reconcile_playlists_step_in(
                        &db,
                        desired,
                        &candidate,
                        root,
                        self.serial,
                        progress,
                    )?,
                    None => crate::apply_loop::reconcile_playlists_step(
                        &db,
                        desired,
                        &candidate,
                        self.serial,
                        progress,
                    )?,
                }
            }

            for entry in candidate.tracks.iter().filter(|entry| entry.source_known) {
                let art = self.artwork_cache.load_for_source(&entry.source_path)?;
                db.set_track_artwork(entry.ipod_dbid, art.as_deref())?;
            }
            journal.candidate_manifest = Some(candidate.clone());
            store
                .save(journal)
                .context("journal candidate before DB write")?;
            remove_stale_artwork_outputs(self.mount)
        })();
        if let Err(error) = preparation {
            let message = format!("{error:#}");
            drop(db);
            self.rollback_to_ready(journal, store, snapshot, options, error)?;
            bail!("candidate database preparation failed and was restored: {message}");
        }

        if let Err(error) = db.write().context("write candidate iTunesDB and artwork") {
            drop(db);
            self.rollback_to_ready(journal, store, snapshot, options, error)?;
            bail!("database publication failed; database and artwork restored");
        }
        drop(db);
        let reopened =
            match crate::ipod::db::OwnedDb::open(self.mount).context("reopen candidate iTunesDB") {
                Ok(db) => db,
                Err(error) => {
                    self.rollback_to_ready(journal, store, snapshot, options, error)?;
                    bail!("database verification failed; database and artwork restored");
                }
            };
        if let Err(error) = self.verify_candidate(&reopened, &candidate) {
            drop(reopened);
            self.rollback_to_ready(journal, store, snapshot, options, error)?;
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
        options: PublishOptions<'_>,
        cause: anyhow::Error,
    ) -> Result<()> {
        snapshot
            .restore(self.mount)
            .with_context(|| format!("restore rollback after {cause:#}"))?;
        self.restore_managed_playlist_record(journal, options)?;
        self.cleanup_after_rollback(journal)?;
        reset_staged_publication(journal);
        journal.phase = crate::pending_session::PendingPhase::ReadyToPublish;
        store.save(journal)
    }

    fn ensure_managed_playlist_record_snapshot(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
        options: PublishOptions<'_>,
    ) -> Result<()> {
        if journal.managed_playlist_record_snapshot.is_some() {
            return Ok(());
        }
        let path = self.managed_playlist_record_path(options)?;
        let contents = match std::fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("snapshot playlist record {}", path.display()));
            }
        };
        journal.managed_playlist_record_snapshot =
            Some(crate::pending_session::ManagedPlaylistRecordSnapshot { contents });
        store
            .save(journal)
            .context("journal managed-playlist record before reconciliation")
    }

    fn restore_managed_playlist_record(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        options: PublishOptions<'_>,
    ) -> Result<()> {
        if journal.managed_playlist_record_snapshot.is_none() {
            return Ok(());
        }
        let path = self.managed_playlist_record_path(options)?;
        let snapshot = journal.managed_playlist_record_snapshot.take().unwrap();
        match snapshot.contents {
            Some(contents) => crate::atomic_file::AtomicFileWriter::new()
                .write(&path, &contents)
                .with_context(|| format!("restore playlist record {}", path.display())),
            None => remove_file_if_present(&path),
        }
    }

    fn managed_playlist_record_path(&self, options: PublishOptions<'_>) -> Result<PathBuf> {
        match options.playlist_state_root {
            Some(root) => crate::device_state::managed_playlists_path_in(root, self.serial),
            None => crate::device_state::managed_playlists_path(self.serial),
        }
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

fn apply_device_identity(
    db: &crate::ipod::db::OwnedDb,
    identity: Option<&crate::ipod::device::LibgpodIdentity>,
) -> Result<()> {
    let Some(identity) = identity else {
        return Ok(());
    };
    unsafe {
        let device = (*db.as_ptr()).device;
        crate::ipod::device::set_firewire_guid(device, &identity.firewire_guid)?;
        crate::ipod::device::set_model_num(device, &identity.model_num_str)
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
    use crate::manifest::{Manifest, ManifestEntry};
    use crate::manifest_store::ManifestStore;
    use crate::pending_session::PendingPhase;
    use crate::pending_session::{PendingAlbum, PendingSession, PendingSessionStore, StagedFile};
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

    #[test]
    fn candidate_database_receives_complete_device_identity() {
        let mount =
            temp_mount().with_file_name(format!("transaction-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        write_valid_itunesdb(&mount);
        let db = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        let identity = crate::ipod::device::LibgpodIdentity {
            firewire_guid: "000A27002138B0A8".into(),
            model_num_str: "MC293".into(),
        };

        apply_device_identity(&db, Some(&identity)).unwrap();

        unsafe {
            let device = (*db.as_ptr()).device;
            for (key, expected) in [
                (c"FirewireGuid", identity.firewire_guid.as_str()),
                (c"ModelNumStr", identity.model_num_str.as_str()),
            ] {
                let value = crate::ffi::itdb_device_get_sysinfo(device, key.as_ptr());
                assert!(!value.is_null());
                assert_eq!(std::ffi::CStr::from_ptr(value).to_str().unwrap(), expected);
                crate::ffi::g_free(value.cast());
            }
        }
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

    #[test]
    fn coordinator_reconciles_playlists_against_post_staging_dbids() {
        let (mount, host, store, cache, mut manifest) = coordinator_fixture("playlists");
        let source = manifest
            .last_source_root
            .as_ref()
            .unwrap()
            .join("album/track.flac");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tagged.flac"),
            &source,
        )
        .unwrap();
        let pending = host.join("track.m4a");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bare.m4a"),
            &pending,
        )
        .unwrap();
        cache.record_no_art(&source).unwrap();

        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(13, "SERIAL", vec![album]);
        let mut staged = StagedFile::minimal(source.clone(), pending, None, 0);
        staged.candidate_entry = Some(ManifestEntry {
            source_path: source.clone(),
            source_mtime: 1,
            source_size: 2,
            source_fingerprint: "fingerprint".into(),
            ipod_dbid: 0,
            ipod_relpath: String::new(),
            source_known: true,
            audio_fingerprint: String::new(),
            encoder: "afconvert".into(),
            encoder_version: String::new(),
            source_format: "flac".into(),
        });
        journal.staged_files.push(staged);

        let desired = vec![("mix".to_string(), "Mix".to_string(), vec![source.clone()])];
        let state_root = host.join("state");
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: "SERIAL",
            manifest_store: &store,
            artwork_cache: cache,
        };

        coordinator
            .publish_with_options(
                &mut journal,
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                },
            )
            .unwrap();
        progress.finish(true).unwrap();

        let dbid = manifest
            .tracks
            .iter()
            .find(|entry| entry.source_path == source)
            .unwrap()
            .ipod_dbid;
        let reopened = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        unsafe {
            let name = CString::new("Mix").unwrap();
            let playlist =
                crate::ffi::itdb_playlist_by_name(reopened.as_ptr(), name.as_ptr() as *mut _);
            assert!(!playlist.is_null());
            let member = (*playlist).members;
            assert!(!member.is_null());
            assert_eq!(
                (*((*member).data as *mut crate::ffi::Itdb_Track)).dbid,
                dbid
            );
            assert!((*member).next.is_null());
        }
    }

    #[test]
    fn playlist_record_is_restored_when_checkpoint_rolls_back() {
        let (mount, host, store, cache, mut manifest) = coordinator_fixture("playlist-rollback");
        let state_root = host.join("state");
        let record = crate::device_state::managed_playlists_path_in(&state_root, "SERIAL").unwrap();
        let original = br#"{
  "names": [
    { "slug": "old", "name": "Old", "id": 123 }
  ]
}"#;
        std::fs::write(&record, original).unwrap();
        let manifest_path = crate::device_state::portable_manifest_path(&mount);
        std::fs::create_dir_all(&manifest_path).unwrap();

        let mut journal = PendingSession::new(14, "SERIAL", Vec::new());
        let desired = Vec::<DesiredPlaylist>::new();
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: "SERIAL",
            manifest_store: &store,
            artwork_cache: cache,
        };

        assert!(coordinator
            .publish_with_options(
                &mut journal,
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                },
            )
            .is_err());
        progress.finish(false).unwrap();

        assert_eq!(std::fs::read(record).unwrap(), original);
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert!(journal.managed_playlist_record_snapshot.is_none());
    }
}
