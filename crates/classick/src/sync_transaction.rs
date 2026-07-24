mod playlist_publication;
mod rockbox_publication;
mod rollback;

use anyhow::{bail, Context, Result};
pub use playlist_publication::{verify_managed_playlists, PlaylistFailurePoint};
use rollback::is_managed_artwork_output;
pub use rollback::{FailurePoint, RollbackSnapshot};
use std::path::{Path, PathBuf};

pub type DesiredPlaylist = (String, String, Vec<PathBuf>);

#[derive(Debug, Clone, Copy, Default)]
pub struct PublishOptions<'a> {
    pub desired_playlists: Option<&'a [DesiredPlaylist]>,
    pub playlist_state_root: Option<&'a Path>,
    pub device_identity: Option<&'a crate::ipod::device::LibgpodIdentity>,
    pub playlist_failure_point: Option<PlaylistFailurePoint>,
    pub rockbox_compat: bool,
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
    pub mutation_session: &'a crate::device_coordination::DeviceMutationSession,
    pub manifest_store: &'a crate::manifest_store::ManifestStore,
    pub artwork_cache: crate::artwork_cache::ArtworkCache,
}

#[derive(Clone, Copy)]
enum RollbackCleanup {
    PreservePending,
    RemovePending,
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
        if !discovery.rejected.is_empty() {
            let details = discovery
                .rejected
                .iter()
                .map(|rejected| format!("{}: {}", rejected.path.display(), rejected.reason))
                .collect::<Vec<_>>()
                .join("; ");
            bail!("unsafe pending-session journal(s) must be resolved before recovery: {details}");
        }
        let mut recovered = Vec::with_capacity(discovery.sessions.len());
        for mut journal in discovery.sessions {
            progress.log(format!(
                "Recovering interrupted sync session {} from {:?}",
                journal.session_id, journal.phase
            ));
            self.resume_interrupted_verified_rollback(&mut journal, &store)?;
            let result = if journal.phase == crate::pending_session::PendingPhase::Staging {
                self.abandon_interrupted_staging(&mut journal, &store)?
            } else if let Some(result) =
                self.abandon_firmware_normalized_artwork(&mut journal, &store, progress)?
            {
                result
            } else if let Some(result) =
                self.abandon_lost_prepublication_inputs(&journal, &store)?
            {
                result
            } else {
                self.publish_with_options(&mut journal, manifest, progress, options)?
            };
            recovered.push(result);
        }
        Ok(recovered)
    }

    fn resume_interrupted_verified_rollback(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<bool> {
        use crate::pending_session::PendingPhase;

        if journal.phase != PendingPhase::DatabaseVerified {
            return Ok(false);
        }
        self.validate_journal(journal)?;
        let has_recoverable_media = !journal.staged_files.is_empty()
            && journal.staged_files.iter().all(|staged| {
                is_regular_file(&staged.pending_path)
                    || staged
                        .final_ipod_path
                        .as_deref()
                        .is_some_and(is_regular_file)
            });
        if !has_recoverable_media {
            return Ok(false);
        }
        let predecessor = journal
            .generation_before
            .as_ref()
            .context("database-verified recovery has no predecessor generation")?;
        let live = self.mutation_session.capture_current_generation()?;
        if !generation_matches_baseline_plus_runtime(predecessor, &live, &live) {
            return Ok(false);
        }

        self.cleanup_after_rollback(journal, RollbackCleanup::PreservePending)?;
        reset_staged_publication(journal);
        journal.phase = PendingPhase::ReadyToPublish;
        journal.published_generation = None;
        journal.verified_generation = None;
        store.save(journal)?;
        self.mutation_session.adopt_verified_generation(live)?;
        Ok(true)
    }

    fn abandon_firmware_normalized_artwork(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
        progress: &crate::progress::Progress,
    ) -> Result<Option<CheckpointResult>> {
        use crate::pending_session::PendingPhase;

        if journal.phase != PendingPhase::DatabaseVerified {
            return Ok(None);
        }
        self.validate_journal(journal)?;
        let expected = journal
            .verified_generation
            .as_ref()
            .context("database-verified recovery has no verified generation")?;
        let live = self.mutation_session.capture_current_generation()?;
        if !firmware_removed_only_ithmb_outputs(expected, &live) {
            return Ok(None);
        }

        progress.log(format!(
            "Abandoning interrupted sync {} after the iPod firmware removed generated artwork",
            journal.session_id
        ));
        let snapshot = RollbackSnapshot::open(&store.snapshot_dir(journal.session_id))
            .context("validate rollback snapshot after firmware artwork normalization")?;
        snapshot
            .restore(self.mount)
            .context("restore pre-sync database after firmware artwork normalization")?;
        self.cleanup_after_rollback(journal, RollbackCleanup::RemovePending)?;
        self.restore_device_manifest_preimage(journal)?;

        let restored = self.mutation_session.capture_current_generation()?;
        let generation_before = journal
            .generation_before
            .as_ref()
            .context("firmware-normalized recovery has no predecessor generation")?;
        if !generation_matches_baseline_plus_runtime(generation_before, &restored, &live) {
            bail!(
                "external_generation_changed: firmware-normalized rollback did not restore its recorded predecessor"
            );
        }
        journal.phase = PendingPhase::RollbackComplete;
        journal.published_generation = None;
        journal.verified_generation = Some(restored.clone());
        store.save(journal)?;
        self.mutation_session.adopt_verified_generation(restored)?;

        remove_empty_dir_if_present(&store.staged_dir(journal.session_id))?;
        remove_validated_snapshot_if_present(&store.snapshot_dir(journal.session_id))?;
        store.remove(journal.session_id)?;
        Ok(Some(CheckpointResult::default()))
    }

    pub fn publish_with_options(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        manifest: &mut crate::manifest::Manifest,
        progress: &crate::progress::Progress,
        options: PublishOptions<'_>,
    ) -> Result<CheckpointResult> {
        self.verify_generation_fence()?;
        let result = self.publish_unfenced_with_options(journal, manifest, progress, options);
        if result.is_ok() {
            self.mutation_session.accept_verified_generation()?;
        }
        result
    }

    fn publish_unfenced_with_options(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        manifest: &mut crate::manifest::Manifest,
        progress: &crate::progress::Progress,
        options: PublishOptions<'_>,
    ) -> Result<CheckpointResult> {
        use crate::pending_session::PendingPhase;

        self.validate_journal(journal)?;
        if journal.phase == PendingPhase::RollbackComplete {
            bail!(
                "pending sync {} is blocked after verified rollback; manual resolution is required",
                journal.session_id
            );
        }
        let store = crate::pending_session::PendingSessionStore::new(self.mount);
        self.prepare_generation_journal(journal)?;
        store.save(journal)?;

        if journal.phase == PendingPhase::Staging {
            self.prepare_all_artwork(journal, manifest)?;
            journal.phase = PendingPhase::ReadyToPublish;
            store.save(journal)?;
        }

        self.ensure_device_manifest_preimage(journal, &store)?;
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
        let mut manifest_cache_warning = None;
        if journal.phase > PendingPhase::DatabaseVerified {
            let reopened = crate::ipod::db::OwnedDb::open(self.mount)
                .context("reopen finalized iTunesDB for recovery verification")?;
            let verification = self
                .verify_candidate(&reopened, &candidate, journal)
                .and_then(|()| {
                    let Some(ownership) = journal.candidate_playlist_ownership.as_ref() else {
                        return Ok(());
                    };
                    let verified = playlist_publication::verify_managed_playlists(
                        &reopened,
                        ownership,
                        &journal.desired_playlist_memberships,
                    )?;
                    if verified != journal.verified_playlist_memberships {
                        bail!(
                            "reopened managed playlist verification differs from pending journal"
                        );
                    }
                    Ok(())
                });
            if let Err(error) = verification {
                let message = format!("{error:#}");
                drop(reopened);
                self.rollback_after_verified_mismatch(journal, &store, &snapshot, error)?;
                bail!("database verification failed; database and artwork restored: {message}");
            }
        }
        if journal.phase == PendingPhase::DatabaseVerified {
            let reopened =
                crate::ipod::db::OwnedDb::open(self.mount).context("reopen verified iTunesDB")?;
            if let Err(error) = self.verify_candidate(&reopened, &candidate, journal) {
                let message = format!("{error:#}");
                drop(reopened);
                self.rollback_after_verified_mismatch(journal, &store, &snapshot, error)?;
                bail!("database verification failed; database and artwork restored: {message}");
            }
            if let Some(ownership) = journal.candidate_playlist_ownership.as_ref() {
                match playlist_publication::verify_managed_playlists(
                    &reopened,
                    ownership,
                    &journal.desired_playlist_memberships,
                ) {
                    Ok(verified) => journal.verified_playlist_memberships = verified,
                    Err(error) => {
                        drop(reopened);
                        self.rollback_after_verified_mismatch(journal, &store, &snapshot, error)?;
                        bail!("playlist verification failed; database and artwork restored");
                    }
                }
                store.save(journal)?;
            }
            drop(reopened);

            self.verify_generation_fence()
                .context("manifest pre-publication generation fence")?;
            progress.log("Publishing portable device manifest".to_string());
            let outcome = match self.manifest_store.publish_runtime(&candidate) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let message = format!("{error:#}");
                    self.rollback_to_ready(journal, &store, &snapshot, error)?;
                    bail!(
                        "device manifest publication failed; database and artwork restored: {message}"
                    );
                }
            };
            *manifest = candidate.clone();
            manifest_cache_warning = outcome.host_cache_warning;
            journal.phase = PendingPhase::DeviceManifestPublished;
            self.record_verified_generation(journal, &store)?;
            if journal.candidate_playlist_ownership.is_none() {
                self.publish_manifest_authority(journal, &store)?;
                return self.finish_cleanup(journal, &store, manifest_cache_warning, progress);
            }
        }

        if journal.phase >= PendingPhase::DeviceManifestPublished {
            *manifest = candidate;
        }

        if journal.candidate_playlist_ownership.is_some() {
            let ownership = playlist_publication::ownership_store(
                self.mount,
                self.serial,
                options.playlist_state_root,
            )?;
            if journal.phase == PendingPhase::DeviceManifestPublished {
                self.verify_generation_fence()
                    .context("Rockbox plan pre-publication generation fence")?;
                let settled = ownership.load_device_read_only()?;
                rockbox_publication::prepare_playlist_projection(
                    journal,
                    &store,
                    &settled,
                    options.rockbox_compat,
                    options.desired_playlists,
                    &crate::rockbox_projection_fs::DeviceProjectionFs::new(
                        self.mount.to_path_buf(),
                    ),
                    options.playlist_failure_point,
                )?;
                self.record_verified_generation(journal, &store)?;
            }
            if journal.phase == PendingPhase::RockboxProjectionsPrepared {
                self.verify_generation_fence()
                    .context("playlist ownership pre-publication generation fence")?;
                if let Err(error) = playlist_publication::publish_ownership(
                    journal,
                    &store,
                    &ownership,
                    options.playlist_failure_point,
                ) {
                    self.record_interrupted_generation(journal, &store)?;
                    return Err(error);
                }
                self.record_verified_generation(journal, &store)?;
            }
            if journal.phase == PendingPhase::PlaylistOwnershipPublished {
                self.verify_generation_fence()
                    .context("Rockbox projection pre-publication generation fence")?;
                let verified = journal
                    .verified_playlist_memberships
                    .iter()
                    .cloned()
                    .map(|membership| (membership.slug.clone(), membership))
                    .collect();
                if let Err(error) = rockbox_publication::publish_playlist_finalization(
                    &store,
                    journal,
                    &ownership,
                    &crate::rockbox_projection_fs::DeviceProjectionFs::new(
                        self.mount.to_path_buf(),
                    ),
                    &verified,
                ) {
                    self.record_interrupted_generation(journal, &store)?;
                    return Err(error);
                }
                self.record_verified_generation(journal, &store)?;
            }
            if journal.phase == PendingPhase::RockboxProjectionsPublished {
                self.publish_manifest_authority(journal, &store)?;
                let warning = playlist_publication::refresh_host_cache(
                    journal,
                    &ownership,
                    options.playlist_failure_point,
                );
                return self.finish_cleanup(
                    journal,
                    &store,
                    merge_warnings(manifest_cache_warning, warning),
                    progress,
                );
            }
        } else if journal.phase == PendingPhase::DeviceManifestPublished {
            return self.finish_cleanup(journal, &store, None, progress);
        }

        if journal.phase == PendingPhase::CleanupComplete {
            remove_empty_dir_if_present(&store.staged_dir(journal.session_id))?;
            remove_validated_snapshot_if_present(&store.snapshot_dir(journal.session_id))?;
            store.remove(journal.session_id)?;
            return Ok(self.result(journal, None));
        }
        bail!("unsupported pending phase {:?}", journal.phase)
    }

    fn publish_manifest_authority(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        self.verify_generation_fence()
            .context("portable manifest authority pre-publication generation fence")?;
        crate::portable::coordinator::publish_manifest_authority(self.mutation_session)
            .context("publish portable manifest authority")?;
        self.record_verified_generation(journal, store)
    }

    fn verify_generation_fence(&self) -> Result<()> {
        let device_id = crate::device::DeviceId::parse(self.serial)
            .context("checkpoint serial is not a canonical device identity")?;
        self.mutation_session
            .verify_scope(self.mount, &device_id)
            .context("checkpoint mutation session scope")?;
        self.mutation_session
            .verify_expected_generation()
            .context("checkpoint generation fence")
    }

    fn prepare_generation_journal(
        &self,
        journal: &mut crate::pending_session::PendingSession,
    ) -> Result<()> {
        let live = self.mutation_session.capture_current_generation()?;
        match (
            &journal.generation_before,
            &journal.published_generation,
            &journal.verified_generation,
        ) {
            (None, None, None)
                if journal.phase <= crate::pending_session::PendingPhase::ReadyToPublish =>
            {
                journal.generation_before = Some(live);
                Ok(())
            }
            (Some(before), None, None)
                if journal.phase <= crate::pending_session::PendingPhase::ReadyToPublish =>
            {
                if &live != before
                    && !generation_matches_baseline_plus_runtime(before, &live, &live)
                {
                    bail!(
                        "external_generation_changed: pending publication predecessor is no longer live"
                    );
                }
                Ok(())
            }
            (Some(_), Some(published), None)
                if journal.phase <= crate::pending_session::PendingPhase::ReadyToPublish =>
            {
                if &live != published {
                    bail!(
                        "external_generation_changed: pending candidate generation is no longer live"
                    );
                }
                Ok(())
            }
            (Some(_), Some(published), Some(_))
                if journal.phase >= crate::pending_session::PendingPhase::DatabaseVerified =>
            {
                if &live != published {
                    bail!(
                        "external_generation_changed: interrupted publication generation is no longer live"
                    );
                }
                Ok(())
            }
            (Some(_), None, Some(verified))
                if journal.phase >= crate::pending_session::PendingPhase::DatabaseVerified =>
            {
                if &live != verified {
                    bail!(
                        "external_generation_changed: pending publication generation is no longer live"
                    );
                }
                Ok(())
            }
            _ => {
                bail!("recovery_required: pending publication predates generation-fenced recovery")
            }
        }
    }

    fn record_verified_generation(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        let generation = self.mutation_session.capture_current_generation()?;
        journal.published_generation = None;
        journal.verified_generation = Some(generation.clone());
        store.save(journal)?;
        self.mutation_session.adopt_verified_generation(generation)
    }

    fn record_published_generation(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        let generation = self.mutation_session.capture_current_generation()?;
        journal.published_generation = Some(generation.clone());
        store.save(journal)?;
        self.mutation_session.adopt_verified_generation(generation)
    }

    fn record_interrupted_generation(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        self.record_published_generation(journal, store)
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
        remove_empty_dir_if_present(&store.staged_dir(journal.session_id))?;
        remove_validated_snapshot_if_present(&store.snapshot_dir(journal.session_id))?;
        store.remove(journal.session_id)?;
        Ok(result)
    }

    fn abandon_lost_prepublication_inputs(
        &self,
        journal: &crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<Option<CheckpointResult>> {
        use crate::pending_session::PendingPhase;

        if journal.phase != PendingPhase::ReadyToPublish || journal.staged_files.is_empty() {
            return Ok(None);
        }
        let (staged_dir, snapshot_dir, journal_path) =
            self.validate_abandonment_owned_paths(journal, store)?;

        let mut missing = 0;
        for staged in &journal.staged_files {
            match std::fs::symlink_metadata(&staged.pending_path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    bail!(
                        "recovery_required: pending staged input is not a regular file: {}",
                        staged.pending_path.display()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing += 1,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "inspect pending staged input {}",
                            staged.pending_path.display()
                        )
                    });
                }
            }
        }
        if missing == 0 {
            return Ok(None);
        }
        if missing != journal.staged_files.len() {
            bail!("recovery_required: pending publication has only some staged inputs remaining");
        }

        self.validate_journal(journal)?;
        // Retry rollback deletes candidate files before clearing these fields and
        // saving ReadyToPublish, so the cleared journal is the durable proof that
        // no candidate publication remains.
        let publication_is_cleared = journal.published_generation.is_none()
            && journal.verified_generation.is_none()
            && journal
                .staged_files
                .iter()
                .all(|staged| staged.dbid == 0 && staged.final_ipod_path.is_none())
            && journal.candidate_manifest.is_none()
            && journal.managed_playlist_record_snapshot.is_none()
            && journal.candidate_playlist_ownership.is_none()
            && journal.desired_playlist_memberships.is_empty()
            && journal.verified_playlist_memberships.is_empty()
            && journal.pending_rockbox_ops.is_empty()
            && journal.rockbox_projection_plan_version.is_none();
        if !publication_is_cleared {
            bail!("recovery_required: missing staged inputs retain ambiguous publication evidence");
        }

        self.verify_generation_fence()?;
        let predecessor = journal
            .generation_before
            .as_ref()
            .context("recovery_required: lost staged inputs have no recorded predecessor")?;
        let live = self.mutation_session.capture_current_generation()?;
        if &live != predecessor {
            bail!(
                "external_generation_changed: refusing to abandon over an unknown device generation"
            );
        }

        require_empty_owned_directory(&staged_dir, "staged transaction")?;
        let snapshot = require_owned_directory_if_present(&snapshot_dir, "rollback snapshot")?
            .then(|| {
                RollbackSnapshot::open_for_deletion(&snapshot_dir)
                    .context("validate rollback snapshot before abandoning lost staged inputs")
            })
            .transpose()?;

        let result = CheckpointResult::default();
        self.validate_abandonment_owned_paths(journal, store)?;
        require_empty_owned_directory(&staged_dir, "staged transaction")?;
        remove_empty_dir_if_present(&staged_dir)?;
        if let Some(snapshot) = snapshot {
            self.validate_abandonment_owned_paths(journal, store)?;
            snapshot.remove_for_deletion()?;
        }
        self.validate_abandonment_owned_paths(journal, store)?;
        remove_file_if_present(&journal_path)?;
        Ok(Some(result))
    }

    fn validate_abandonment_owned_paths(
        &self,
        journal: &crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<(PathBuf, PathBuf, PathBuf)> {
        let mount = self.mutation_session.mount();
        let pending_root = crate::device_state::pending_sessions_dir(mount);
        require_real_directory_chain(mount, &pending_root, "pending-session root")?;

        let journal_path = pending_root.join(format!("{}.json", journal.session_id));
        if store.path(journal.session_id) != journal_path {
            bail!("recovery_required: unsafe pending-session store path");
        }
        require_regular_file(&journal_path, "pending-session journal")?;

        Ok((
            pending_root.join(format!("{}.staged", journal.session_id)),
            pending_root.join(format!("{}.snapshot", journal.session_id)),
            journal_path,
        ))
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
        for update in &journal.metadata_updates {
            let source = &update.candidate_entry.source_path;
            if let Some(hash) = update.artwork_hash.as_deref() {
                self.artwork_cache.load_hash(hash).with_context(|| {
                    format!("prepare metadata artwork for {}", source.display())
                })?;
            } else {
                self.artwork_cache
                    .load_for_source(source)
                    .with_context(|| {
                        format!("prepare metadata artwork for {}", source.display())
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

    fn ensure_device_manifest_preimage(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        use crate::pending_session::{DeviceManifestPreimage, PendingPhase};

        if journal.device_manifest_preimage.is_some() {
            return Ok(());
        }
        if journal.phase >= PendingPhase::DatabaseVerified {
            bail!(
                "pending sync {} may have published its device manifest but has no safe preimage",
                journal.session_id
            );
        }
        let path = crate::device_state::portable_manifest_path(self.mount);
        let contents = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("capture device manifest {}", path.display()));
            }
        };
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents });
        store
            .save(journal)
            .context("persist device manifest rollback preimage")
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
            if self.verify_candidate(&live, candidate, journal).is_ok() {
                journal.phase = PendingPhase::DatabaseVerified;
                self.record_verified_generation(journal, store)?;
                return Ok(());
            }
            drop(live);
            snapshot.restore(self.mount)?;
            self.cleanup_after_rollback(journal, RollbackCleanup::PreservePending)?;
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
            let pending_path = journal.staged_files[index].pending_path.clone();
            let tags = journal.staged_files[index].tags.clone();
            let artwork_hash = journal.staged_files[index].artwork_hash.clone();
            let source = journal.staged_files[index].source.clone();
            let art = match artwork_hash.as_deref() {
                Some(hash) => Some(self.artwork_cache.load_hash(hash)?),
                None => self.artwork_cache.load_for_source(&source)?,
            };
            let handle = db.add_track_with_staged_file_strict(
                &pending_path,
                &tags,
                art.as_deref(),
                |final_path| {
                    journal.staged_files[index].final_ipod_path = Some(final_path.to_path_buf());
                    store
                        .save(journal)
                        .context("journal reserved iPod path before moving staged media")
                },
            )?;
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

        for index in 0..journal.metadata_updates.len() {
            let update = journal.metadata_updates[index].clone();
            let dbid = update.candidate_entry.ipod_dbid;
            let source = &update.candidate_entry.source_path;
            let predecessor_index = candidate
                .tracks
                .iter()
                .position(|entry| entry.ipod_dbid == dbid)
                .context("metadata update predecessor is missing from the manifest")?;
            let predecessor = &candidate.tracks[predecessor_index];
            if &predecessor.source_path != source
                || predecessor.ipod_relpath != update.candidate_entry.ipod_relpath
                || predecessor.audio_fingerprint != update.candidate_entry.audio_fingerprint
                || predecessor.encoder != update.candidate_entry.encoder
                || predecessor.encoder_version != update.candidate_entry.encoder_version
                || predecessor.source_format != update.candidate_entry.source_format
                || predecessor.transcode_profile != update.candidate_entry.transcode_profile
            {
                bail!("metadata update changes immutable media identity");
            }
            let art = match update.artwork_hash.as_deref() {
                Some(hash) => Some(self.artwork_cache.load_hash(hash)?),
                None => self.artwork_cache.load_for_source(source)?,
            };
            db.update_track_metadata(dbid, &update.tags, None)
                .with_context(|| format!("update metadata for dbid {dbid}"))?;
            db.set_track_artwork(dbid, art.as_deref())
                .with_context(|| format!("update artwork for dbid {dbid}"))?;
            candidate.tracks[predecessor_index] = update.candidate_entry;
            journal.candidate_manifest = Some(candidate.clone());
            store
                .save(journal)
                .context("journal metadata update before DB write")?;
        }

        let preparation = (|| -> Result<()> {
            if let Some(desired) = options.desired_playlists {
                let outcome = match options.playlist_state_root {
                    Some(root) => crate::apply_loop::reconcile_playlists_candidate_step_in(
                        &db,
                        desired,
                        &candidate,
                        root,
                        self.serial,
                        progress,
                    )?,
                    None => crate::apply_loop::reconcile_playlists_candidate_step(
                        &db,
                        desired,
                        &candidate,
                        self.serial,
                        progress,
                    )?,
                };
                journal.candidate_playlist_ownership = Some(outcome.candidate_ownership);
                journal.desired_playlist_memberships = outcome.desired_memberships;
            }

            for entry in candidate.tracks.iter().filter(|entry| entry.source_known) {
                let art = self.artwork_cache.load_for_source(&entry.source_path)?;
                db.set_track_artwork(entry.ipod_dbid, art.as_deref())?;
            }
            journal.candidate_manifest = Some(candidate.clone());
            store
                .save(journal)
                .context("journal candidate before DB write")?;
            playlist_publication::inject(
                options.playlist_failure_point,
                PlaylistFailurePoint::BeforeDatabaseWrite,
            )
        })();
        if let Err(error) = preparation {
            let message = format!("{error:#}");
            drop(db);
            self.rollback_to_ready(journal, store, snapshot, error)?;
            bail!("candidate database preparation failed and was restored: {message}");
        }

        self.verify_generation_fence()
            .context("database pre-publication generation fence")?;
        if let Err(error) =
            remove_stale_artwork_outputs(self.mount).and_then(|()| write_coordinated_database(&db))
        {
            self.record_interrupted_generation(journal, store)?;
            drop(db);
            self.rollback_to_ready(journal, store, snapshot, error)?;
            bail!("database publication failed; database and artwork restored");
        }
        drop(db);
        self.record_published_generation(journal, store)?;
        let reopened =
            match crate::ipod::db::OwnedDb::open(self.mount).context("reopen candidate iTunesDB") {
                Ok(db) => db,
                Err(error) => {
                    self.rollback_to_ready(journal, store, snapshot, error)?;
                    bail!("database verification failed; database and artwork restored");
                }
            };
        if let Err(error) = self.verify_candidate(&reopened, &candidate, journal) {
            let message = format!("{error:#}");
            drop(reopened);
            self.rollback_to_ready(journal, store, snapshot, error)?;
            bail!("database verification failed; database and artwork restored: {message}");
        }
        if let Some(ownership) = journal.candidate_playlist_ownership.as_ref() {
            match playlist_publication::verify_managed_playlists(
                &reopened,
                ownership,
                &journal.desired_playlist_memberships,
            ) {
                Ok(verified) => journal.verified_playlist_memberships = verified,
                Err(error) => {
                    drop(reopened);
                    self.rollback_to_ready(journal, store, snapshot, error)?;
                    bail!("playlist verification failed; database and artwork restored");
                }
            }
        }
        journal.phase = PendingPhase::DatabaseVerified;
        self.record_verified_generation(journal, store)?;
        playlist_publication::inject(
            options.playlist_failure_point,
            PlaylistFailurePoint::AfterDatabaseVerified,
        )?;
        Ok(())
    }

    fn verify_candidate(
        &self,
        db: &crate::ipod::db::OwnedDb,
        candidate: &crate::manifest::Manifest,
        journal: &crate::pending_session::PendingSession,
    ) -> Result<()> {
        for entry in &candidate.tracks {
            let expects_artwork = entry.source_known
                && self
                    .artwork_cache
                    .load_for_source(&entry.source_path)?
                    .is_some();
            db.verify_track(entry.ipod_dbid, &entry.ipod_relpath, expects_artwork)?;
        }
        let candidate_dbids: std::collections::HashSet<_> = candidate
            .tracks
            .iter()
            .map(|entry| entry.ipod_dbid)
            .collect();
        let live_dbids: std::collections::HashSet<_> = db
            .list_tracks_for_rebuild()
            .into_iter()
            .map(|track| track.dbid)
            .collect();
        for obsolete in &journal.obsolete_files {
            if !candidate_dbids.contains(&obsolete.prior_dbid)
                && live_dbids.contains(&obsolete.prior_dbid)
            {
                bail!(
                    "obsolete track {} remains in the candidate database",
                    obsolete.prior_dbid
                );
            }
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
        self.require_known_rollback_generation(journal)?;
        snapshot
            .restore(self.mount)
            .with_context(|| format!("restore rollback after {cause:#}"))?;
        self.cleanup_after_rollback(journal, RollbackCleanup::PreservePending)?;
        self.restore_device_manifest_preimage(journal)?;
        reset_staged_publication(journal);
        journal.phase = crate::pending_session::PendingPhase::ReadyToPublish;
        self.finish_verified_rollback(journal, store)
    }

    fn rollback_after_verified_mismatch(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
        snapshot: &RollbackSnapshot,
        cause: anyhow::Error,
    ) -> Result<()> {
        self.require_known_rollback_generation(journal)?;
        snapshot
            .restore(self.mount)
            .with_context(|| format!("restore rollback after {cause:#}"))?;
        self.cleanup_after_rollback(journal, RollbackCleanup::RemovePending)?;
        self.restore_device_manifest_preimage(journal)?;
        journal.phase = crate::pending_session::PendingPhase::RollbackComplete;
        self.finish_verified_rollback(journal, store)
    }

    fn require_known_rollback_generation(
        &self,
        journal: &crate::pending_session::PendingSession,
    ) -> Result<()> {
        let live = self.mutation_session.capture_current_generation()?;
        let known = journal
            .verified_generation
            .as_ref()
            .or(journal.published_generation.as_ref())
            .or(journal.generation_before.as_ref())
            .context("recovery_required: pending rollback has no recorded generation")?;
        if &live != known && !generation_matches_baseline_plus_runtime(known, &live, &live) {
            bail!(
                "external_generation_changed: refusing to roll back over an unknown device generation"
            );
        }
        Ok(())
    }

    fn finish_verified_rollback(
        &self,
        journal: &mut crate::pending_session::PendingSession,
        store: &crate::pending_session::PendingSessionStore,
    ) -> Result<()> {
        let restored = self.mutation_session.capture_current_generation()?;
        let expected = journal
            .generation_before
            .as_ref()
            .context("verified rollback has no predecessor generation")?;
        if &restored != expected
            && !generation_matches_baseline_plus_runtime(expected, &restored, &restored)
        {
            bail!("rollback did not restore the recorded predecessor generation");
        }
        journal.published_generation = None;
        journal.verified_generation =
            if journal.phase >= crate::pending_session::PendingPhase::DatabaseVerified {
                Some(restored.clone())
            } else {
                None
            };
        store.save(journal)?;
        self.mutation_session.adopt_verified_generation(restored)
    }

    fn restore_device_manifest_preimage(
        &self,
        journal: &crate::pending_session::PendingSession,
    ) -> Result<()> {
        let preimage = journal
            .device_manifest_preimage
            .as_ref()
            .context("verified rollback has no device manifest preimage")?;
        let path = crate::device_state::portable_manifest_path(self.mount);
        match preimage.contents.as_deref() {
            Some(bytes) => crate::atomic_file::AtomicFileWriter::new()
                .write(&path, bytes)
                .context("restore exact device manifest preimage"),
            None => {
                remove_file_if_present(&path).context("restore absent device manifest preimage")
            }
        }
    }

    fn cleanup_after_rollback(
        &self,
        journal: &crate::pending_session::PendingSession,
        cleanup: RollbackCleanup,
    ) -> Result<()> {
        let restored =
            crate::ipod::db::OwnedDb::open(self.mount).context("open restored iTunesDB")?;
        let referenced = restored
            .referenced_paths(self.mount)
            .into_iter()
            .collect::<crate::pending_session::ReferencedPaths>();
        match cleanup {
            RollbackCleanup::PreservePending => {
                for staged in &journal.staged_files {
                    if let Some(path) = &staged.final_ipod_path {
                        if !referenced.contains(path) {
                            if staged.pending_path.exists() {
                                remove_file_if_present(path)?;
                            } else if path.exists() {
                                std::fs::rename(path, &staged.pending_path).with_context(|| {
                                    format!(
                                        "restore moved staged media {} to {}",
                                        path.display(),
                                        staged.pending_path.display()
                                    )
                                })?;
                            }
                        }
                    }
                }
            }
            RollbackCleanup::RemovePending => {
                crate::pending_session::cleanup_unreferenced_staged_files(journal, &referenced)?;
            }
        }
        Ok(())
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
        remove_empty_dir_if_present(&store.staged_dir(journal.session_id))?;
        remove_validated_snapshot_if_present(&store.snapshot_dir(journal.session_id))?;
        store.remove(journal.session_id)?;
        Ok(result)
    }

    fn result(
        &self,
        journal: &crate::pending_session::PendingSession,
        host_cache_warning: Option<String>,
    ) -> CheckpointResult {
        CheckpointResult {
            published_albums: journal.albums.len(),
            published_tracks: journal.staged_files.len() + journal.metadata_updates.len(),
            host_cache_warning,
        }
    }
}

pub fn write_coordinated_database(db: &crate::ipod::db::OwnedDb) -> Result<()> {
    crate::ipod::playlist_normalize::normalize_firmware_playlists(db)
        .context("normalize exact firmware playlist duplicates")?;
    db.write().context("write candidate iTunesDB and artwork")
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
    journal.candidate_playlist_ownership = None;
    journal.desired_playlist_memberships.clear();
    journal.verified_playlist_memberships.clear();
    journal.pending_rockbox_ops.clear();
    journal.rockbox_projection_plan_version = None;
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

fn firmware_removed_only_ithmb_outputs(
    expected: &crate::device_coordination::DeviceGeneration,
    live: &crate::device_coordination::DeviceGeneration,
) -> bool {
    let expected = expected
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    let live = live
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();

    if live.iter().any(|(path, entry)| {
        !is_firmware_runtime_generation_path(path)
            && expected
                .get(path)
                .is_none_or(|expected| *expected != *entry)
    }) {
        return false;
    }

    let missing = expected
        .keys()
        .filter(|path| !is_firmware_runtime_generation_path(path) && !live.contains_key(*path))
        .copied()
        .collect::<Vec<_>>();
    !missing.is_empty() && missing.into_iter().all(is_ithmb_generation_path)
}

fn generation_matches_baseline_plus_runtime(
    baseline: &crate::device_coordination::DeviceGeneration,
    restored: &crate::device_coordination::DeviceGeneration,
    runtime_source: &crate::device_coordination::DeviceGeneration,
) -> bool {
    let without_runtime = |generation: &crate::device_coordination::DeviceGeneration| {
        generation
            .entries
            .iter()
            .filter(|entry| !is_firmware_runtime_generation_path(&entry.path))
            .cloned()
            .collect::<Vec<_>>()
    };
    let runtime = |generation: &crate::device_coordination::DeviceGeneration| {
        generation
            .entries
            .iter()
            .filter(|entry| is_firmware_runtime_generation_path(&entry.path))
            .cloned()
            .collect::<Vec<_>>()
    };
    without_runtime(baseline) == without_runtime(restored)
        && runtime(restored) == runtime(runtime_source)
}

fn is_firmware_runtime_generation_path(path: &str) -> bool {
    matches!(
        path,
        "iPod_Control/iTunes/Play Counts" | "iPod_Control/iTunes/Play Counts.bak"
    )
}

fn is_ithmb_generation_path(path: &str) -> bool {
    let path = Path::new(path);
    if path.parent() != Some(Path::new("iPod_Control/Artwork")) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name
        .strip_prefix('F')
        .and_then(|name| name.strip_suffix(".ithmb"))
    else {
        return false;
    };
    let mut parts = stem.split('_');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(format), Some(index), None)
            if !format.is_empty()
                && !index.is_empty()
                && format.bytes().all(|byte| byte.is_ascii_digit())
                && index.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn remove_validated_snapshot_if_present(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => RollbackSnapshot::open_for_deletion(path)?.remove_for_deletion(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("inspect rollback snapshot {}", path.display()))
        }
    }
}

fn remove_empty_dir_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_dir(path) {
        Ok(()) => remove_file_if_present(&appledouble_sibling(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove empty {}", path.display())),
    }
}

fn appledouble_sibling(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    path.with_file_name(format!("._{}", name.to_string_lossy()))
}

fn require_empty_owned_directory(path: &Path, label: &str) -> Result<()> {
    if !require_owned_directory_if_present(path, label)? {
        return Ok(());
    }
    let mut entries =
        std::fs::read_dir(path).with_context(|| format!("inspect {label} {}", path.display()))?;
    if entries.next().transpose()?.is_some() {
        bail!(
            "recovery_required: {label} is not empty: {}",
            path.display()
        );
    }
    Ok(())
}

fn require_owned_directory_if_present(path: &Path, label: &str) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => bail!(
            "recovery_required: {label} is not a real directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {label} {}", path.display())),
    }
}

fn require_real_directory_chain(root: &Path, target: &Path, label: &str) -> Result<()> {
    let relative = target.strip_prefix(root).with_context(|| {
        format!(
            "recovery_required: unsafe {label} path {}",
            target.display()
        )
    })?;
    require_real_directory(root, "device mount")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            bail!(
                "recovery_required: unsafe {label} path component in {}",
                target.display()
            );
        };
        current.push(component);
        require_real_directory(&current, label)?;
    }
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => bail!(
            "recovery_required: unsafe {label} is not a real directory: {}",
            path.display()
        ),
        Err(error) => Err(error).with_context(|| format!("inspect {label} {}", path.display())),
    }
}

fn require_regular_file(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => bail!(
            "recovery_required: unsafe {label} is not a regular file: {}",
            path.display()
        ),
        Err(error) => Err(error).with_context(|| format!("inspect {label} {}", path.display())),
    }
}

fn merge_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::AtomicFileWriter;
    use crate::manifest::{Manifest, ManifestEntry};
    use crate::manifest_store::ManifestStore;
    use crate::pending_session::PendingPhase;
    use crate::pending_session::{
        DeviceManifestPreimage, PendingAlbum, PendingMetadataUpdate, PendingSession,
        PendingSessionStore, StagedFile,
    };
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::ptr;
    use std::sync::atomic::{AtomicU64, Ordering};

    const TEST_DEVICE_ID: &str = "000A27002138B0A8";
    static NEXT_TEMP_MOUNT: AtomicU64 = AtomicU64::new(0);

    fn mutation_session(mount: &Path) -> crate::device_coordination::DeviceMutationSession {
        crate::device_coordination::DeviceMutationSession::acquire(
            mount,
            crate::device::DeviceId::parse(TEST_DEVICE_ID).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn publication_phase_order_is_total() {
        assert!(PendingPhase::Staging < PendingPhase::ReadyToPublish);
        assert!(PendingPhase::ReadyToPublish < PendingPhase::DatabaseVerified);
        assert!(PendingPhase::DatabaseVerified < PendingPhase::DeviceManifestPublished);
        assert!(PendingPhase::DeviceManifestPublished < PendingPhase::RockboxProjectionsPrepared);
        assert!(
            PendingPhase::RockboxProjectionsPrepared < PendingPhase::PlaylistOwnershipPublished
        );
        assert!(
            PendingPhase::PlaylistOwnershipPublished < PendingPhase::RockboxProjectionsPublished
        );
        assert!(PendingPhase::RockboxProjectionsPublished < PendingPhase::CleanupComplete);
        assert!(PendingPhase::DeviceManifestPublished < PendingPhase::CleanupComplete);
    }

    #[test]
    fn detects_only_firmware_removed_ithmb_outputs() {
        use crate::device_coordination::{DeviceGeneration, GenerationEntry};

        let entry = |path: &str, length: u64, hash: &str| GenerationEntry {
            path: path.to_owned(),
            length,
            blake3: hash.to_owned(),
        };
        let expected = DeviceGeneration {
            entries: vec![
                entry("iPod_Control/Artwork/ArtworkDB", 3000, "artwork-db"),
                entry("iPod_Control/Artwork/F1027_1.ithmb", 100_000, "full"),
                entry("iPod_Control/Artwork/F1031_1.ithmb", 17_640, "thumb"),
                entry("iPod_Control/iTunes/iTunesDB", 14_406, "itunes-db"),
            ],
        };
        let live = DeviceGeneration {
            entries: vec![
                entry("iPod_Control/Artwork/ArtworkDB", 3000, "artwork-db"),
                entry("iPod_Control/Artwork/F1031_1.ithmb", 17_640, "thumb"),
                entry("iPod_Control/iTunes/Play Counts", 156, "play-counts"),
                entry(
                    "iPod_Control/iTunes/Play Counts.bak",
                    156,
                    "play-counts-backup",
                ),
                entry("iPod_Control/iTunes/iTunesDB", 14_406, "itunes-db"),
            ],
        };

        assert!(firmware_removed_only_ithmb_outputs(&expected, &live));

        let changed_db = DeviceGeneration {
            entries: vec![
                entry("iPod_Control/Artwork/ArtworkDB", 3000, "artwork-db"),
                entry("iPod_Control/Artwork/F1031_1.ithmb", 17_640, "thumb"),
                entry("iPod_Control/iTunes/iTunesDB", 14_407, "changed"),
            ],
        };
        assert!(!firmware_removed_only_ithmb_outputs(&expected, &changed_db));
        assert!(!firmware_removed_only_ithmb_outputs(&expected, &expected));
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
            .join(format!(
                "transaction-snapshot-{}-{}",
                std::process::id(),
                NEXT_TEMP_MOUNT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Artwork")).unwrap();
        mount
    }

    #[test]
    fn temp_mount_is_unique_within_the_test_process() {
        assert_ne!(temp_mount(), temp_mount());
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
            TEST_DEVICE_ID.into(),
            host.join("manifest.json"),
            host.join("legacy.json"),
            AtomicFileWriter::new(),
        );
        let cache = crate::artwork_cache::ArtworkCache::new(host.join("artwork"));
        let mut manifest = Manifest::empty();
        manifest.version = 2;
        manifest.ipod_serial = Some(TEST_DEVICE_ID.into());
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
    fn snapshot_cleanup_accepts_only_paired_macos_appledouble_files() {
        let mount = temp_mount();
        let db = mount.join("iPod_Control/iTunes/iTunesDB");
        std::fs::write(&db, b"old db").unwrap();
        let snapshot_dir = mount.join("iPod_Control/classick/pending/2.snapshot");
        let snapshot = RollbackSnapshot::create(&mount, &snapshot_dir).unwrap();
        drop(snapshot);
        for relative in [
            "._snapshot.json",
            "._iPod_Control",
            "iPod_Control/._iTunes",
            "iPod_Control/iTunes/._iTunesDB",
        ] {
            std::fs::write(snapshot_dir.join(relative), b"AppleDouble metadata").unwrap();
        }

        RollbackSnapshot::open_for_deletion(&snapshot_dir)
            .unwrap()
            .remove_for_deletion()
            .unwrap();

        assert!(!snapshot_dir.exists());
    }

    #[test]
    fn coordinator_publishes_in_order_and_removes_journal_last() {
        let (mount, _host, store, cache, mut manifest) = coordinator_fixture("publish");
        let mut journal = PendingSession::new(11, TEST_DEVICE_ID, Vec::new());
        let journal_store = PendingSessionStore::new(&mount);
        let staged_dir = crate::device_state::pending_sessions_dir(&mount).join("11.staged");
        std::fs::create_dir_all(&staged_dir).unwrap();
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
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
        assert!(!staged_dir.exists());
    }

    #[test]
    fn metadata_publication_preserves_media_file_path_and_dbid() {
        let (mount, host, store, cache, mut manifest) = coordinator_fixture("metadata-in-place");
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
        let media = host.join("track.m4a");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bare.m4a"),
            &media,
        )
        .unwrap();

        let db = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        let handle = db
            .add_track_with_file_strict(
                &media,
                &crate::ipod::db::Tags {
                    title: Some("Old title".into()),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        db.write().unwrap();
        drop(db);

        let device_file = mount.join(
            handle
                .ipod_relpath
                .replace('\\', std::path::MAIN_SEPARATOR_STR),
        );
        let original_media = std::fs::read(&device_file).unwrap();
        let existing = ManifestEntry {
            source_path: source.clone(),
            source_mtime: 1,
            source_size: 2,
            source_fingerprint: "old-fingerprint".into(),
            ipod_dbid: handle.dbid,
            ipod_relpath: handle.ipod_relpath.clone(),
            source_known: true,
            audio_fingerprint: "audio-fingerprint".into(),
            encoder: "afconvert".into(),
            encoder_version: "system".into(),
            source_format: "flac".into(),
            transcode_profile: Some(crate::portable::profile::TranscodeProfile::Alac),
        };
        manifest.tracks.push(existing.clone());
        cache.record_no_art(&source).unwrap();

        let mut candidate = existing;
        candidate.source_mtime = 3;
        candidate.source_size = 4;
        candidate.source_fingerprint = "new-fingerprint".into();
        let mut journal =
            PendingSession::new(32, TEST_DEVICE_ID, vec![PendingAlbum::new("album", 0)]);
        journal.metadata_updates.push(PendingMetadataUpdate {
            tags: crate::ipod::db::Tags {
                title: Some("New title".into()),
                ..Default::default()
            },
            artwork_hash: None,
            candidate_entry: candidate,
        });

        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &store,
            artwork_cache: cache,
        };

        let result = coordinator
            .publish(&mut journal, &mut manifest, &progress)
            .unwrap();
        progress.finish(true).unwrap();

        assert_eq!(result.published_tracks, 1);
        assert_eq!(manifest.tracks.len(), 1);
        assert_eq!(manifest.tracks[0].ipod_dbid, handle.dbid);
        assert_eq!(manifest.tracks[0].ipod_relpath, handle.ipod_relpath);
        assert_eq!(manifest.tracks[0].source_fingerprint, "new-fingerprint");
        assert_eq!(std::fs::read(&device_file).unwrap(), original_media);

        let reopened = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        unsafe {
            let mut node = (*reopened.as_ptr()).tracks;
            let mut title = None;
            while !node.is_null() {
                let track = (*node).data.cast::<crate::ffi::Itdb_Track>();
                if !track.is_null() && (*track).dbid as u64 == handle.dbid {
                    title = Some(
                        std::ffi::CStr::from_ptr((*track).title)
                            .to_string_lossy()
                            .into_owned(),
                    );
                    break;
                }
                node = (*node).next;
            }
            assert_eq!(title.as_deref(), Some("New title"));
        }
    }

    #[test]
    fn stale_artwork_cleanup_occurs_inside_the_publication_generation_fence() {
        let (mount, _host, store, cache, mut manifest) = coordinator_fixture("existing-artwork");
        let artwork = mount.join("iPod_Control/Artwork");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::write(artwork.join("ArtworkDB"), b"stale artwork database").unwrap();
        std::fs::write(artwork.join("F1069_1.ithmb"), b"stale thumbnails").unwrap();
        let mutation_session = mutation_session(&mount);
        let mut journal = PendingSession::new(31, TEST_DEVICE_ID, Vec::new());
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &store,
            artwork_cache: cache,
        };

        let result = coordinator.publish(&mut journal, &mut manifest, &progress);
        progress.finish(result.is_ok()).unwrap();

        assert!(result.is_ok(), "{result:#?}");
        assert!(!PendingSessionStore::new(&mount).path(31).exists());
    }

    #[test]
    fn database_verified_journal_at_predecessor_resumes_ready_to_publish() {
        let (mount, _host, manifest_store, cache, _manifest) =
            coordinator_fixture("resume-interrupted-rollback");
        let journal_store = PendingSessionStore::new(&mount);
        let pending = journal_store
            .path(33)
            .with_file_name("33.staged")
            .join("track.m4a");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, b"pending transcode").unwrap();
        let play_counts = mount.join("iPod_Control/iTunes/Play Counts");
        let play_counts_backup = mount.join("iPod_Control/iTunes/Play Counts.bak");
        std::fs::write(&play_counts, b"firmware runtime").unwrap();

        let mutation_session = mutation_session(&mount);
        let generation = mutation_session.current_generation().unwrap();
        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(33, TEST_DEVICE_ID, vec![album]);
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(generation.clone());
        journal.published_generation = Some(generation.clone());
        journal.verified_generation = Some(generation);
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending,
            Some(mount.join("iPod_Control/Music/F00/candidate.m4a")),
            41,
        ));
        journal.candidate_manifest = Some(Manifest::empty());
        journal_store.save(&journal).unwrap();
        std::fs::rename(&play_counts, &play_counts_backup).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };

        assert!(coordinator
            .resume_interrupted_verified_rollback(&mut journal, &journal_store)
            .unwrap());
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert!(journal.published_generation.is_none());
        assert!(journal.verified_generation.is_none());
        assert!(journal.candidate_manifest.is_none());
        assert_eq!(journal.staged_files[0].dbid, 0);
        assert!(journal.staged_files[0].final_ipod_path.is_none());
        assert_eq!(journal_store.load(33).unwrap(), journal);
        coordinator
            .prepare_generation_journal(&mut journal)
            .unwrap();
    }

    #[test]
    fn device_manifest_failure_restores_snapshot_and_keeps_ready_journal() {
        let (mount, host, _store, cache, mut manifest) = coordinator_fixture("rollback");
        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(12)).unwrap();
        let original_db = std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap();
        let mutation_session = mutation_session(&mount);
        let mut journal = PendingSession::new(12, TEST_DEVICE_ID, Vec::new());
        journal.phase = PendingPhase::DatabaseVerified;
        let generation = mutation_session.current_generation().unwrap();
        journal.generation_before = Some(generation.clone());
        journal.verified_generation = Some(generation);
        journal.candidate_manifest = Some(manifest.clone());
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        journal_store.save(&journal).unwrap();
        let manifest_path = crate::device_state::portable_manifest_path(&mount);
        let store = ManifestStore::new(
            mount.clone(),
            TEST_DEVICE_ID.into(),
            host.join("manifest.json"),
            host.join("legacy.json"),
            AtomicFileWriter::failing_before_replace(manifest_path),
        );
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &store,
            artwork_cache: cache,
        };

        let error = coordinator
            .publish(&mut journal, &mut manifest, &progress)
            .unwrap_err();
        progress.finish(false).unwrap();

        assert!(
            format!("{error:#}").contains("injected failure before atomic replace"),
            "{error:#}"
        );
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert!(journal_store.path(12).exists());
        assert_eq!(
            std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap(),
            original_db
        );
        snapshot.validate().unwrap();
    }

    #[test]
    fn verified_rollback_preserves_pending_transcode_and_resets_publication() {
        let (mount, _host, manifest_store, cache, _manifest) =
            coordinator_fixture("rollback-preserves-pending");
        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(18)).unwrap();
        let pending = journal_store
            .path(18)
            .with_file_name("18.staged")
            .join("track.m4a");
        let published = mount.join("iPod_Control/Music/F00/published.m4a");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, b"pending transcode").unwrap();
        std::fs::write(&published, b"published copy").unwrap();

        let mutation_session = mutation_session(&mount);
        let generation = mutation_session.current_generation().unwrap();
        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(18, TEST_DEVICE_ID, vec![album]);
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(generation.clone());
        journal.verified_generation = Some(generation);
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending.clone(),
            Some(published.clone()),
            41,
        ));
        journal.candidate_manifest = Some(Manifest::empty());
        journal.candidate_playlist_ownership =
            Some(crate::ipod::playlist_ownership::ManagedPlaylistOwnership {
                schema_version: crate::ipod::playlist_ownership::MANAGED_PLAYLIST_OWNERSHIP_VERSION,
                device_serial: TEST_DEVICE_ID.into(),
                playlists: std::collections::BTreeMap::from([(
                    "mix".into(),
                    crate::ipod::playlist_ownership::ManagedPlaylistEntry {
                        apple_playlist_id: 71,
                        expected_kind: crate::ipod::playlist_ownership::ManagedPlaylistKind::Normal,
                        rockbox: None,
                    },
                )]),
            });
        journal
            .desired_playlist_memberships
            .insert("mix".into(), vec![41]);
        journal.verified_playlist_memberships.push(
            crate::ipod::device_playlists::VerifiedPlaylistMembership {
                slug: "mix".into(),
                apple_playlist_id: 71,
                ordered_dbids: vec![41],
                ordered_ipod_paths: vec!["/iPod_Control/Music/F00/published.m4a".into()],
            },
        );
        journal.pending_rockbox_ops.insert(
            "mix".into(),
            crate::pending_session::PendingRockboxOp {
                previous: None,
                desired: None,
            },
        );
        journal.rockbox_projection_plan_version =
            Some(crate::pending_session::ROCKBOX_PROJECTION_PLAN_VERSION);
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };

        coordinator
            .rollback_to_ready(
                &mut journal,
                &journal_store,
                &snapshot,
                anyhow::anyhow!("injected verification failure"),
            )
            .unwrap();

        assert_eq!(std::fs::read(&pending).unwrap(), b"pending transcode");
        assert!(!published.exists());
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert_eq!(journal.staged_files[0].dbid, 0);
        assert!(journal.staged_files[0].final_ipod_path.is_none());
        assert!(journal.candidate_manifest.is_none());
        assert!(journal.candidate_playlist_ownership.is_none());
        assert!(journal.desired_playlist_memberships.is_empty());
        assert!(journal.verified_playlist_memberships.is_empty());
        assert!(journal.pending_rockbox_ops.is_empty());
        assert!(journal.rockbox_projection_plan_version.is_none());
        assert_eq!(journal_store.load(18).unwrap(), journal);
    }

    #[test]
    fn verified_rollback_moves_published_media_back_to_staging() {
        let (mount, _host, manifest_store, cache, _manifest) =
            coordinator_fixture("rollback-restores-moved-staging");
        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(19)).unwrap();
        let pending = journal_store
            .path(19)
            .with_file_name("19.staged")
            .join("track.m4a");
        let published = mount.join("iPod_Control/Music/F00/published.m4a");
        let play_counts = mount.join("iPod_Control/iTunes/Play Counts");
        let play_counts_backup = mount.join("iPod_Control/iTunes/Play Counts.bak");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&published, b"moved transcode").unwrap();
        let play_counts_bytes = b"firmware runtime";
        std::fs::write(&play_counts, play_counts_bytes).unwrap();

        let mutation_session = mutation_session(&mount);
        let mut legacy_generation = mutation_session.current_generation().unwrap();
        legacy_generation
            .entries
            .push(crate::device_coordination::GenerationEntry {
                path: "iPod_Control/iTunes/Play Counts".to_owned(),
                length: play_counts_bytes.len() as u64,
                blake3: blake3::hash(play_counts_bytes).to_hex().to_string(),
            });
        legacy_generation
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(19, TEST_DEVICE_ID, vec![album]);
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(legacy_generation.clone());
        journal.verified_generation = Some(legacy_generation);
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending.clone(),
            Some(published.clone()),
            41,
        ));
        journal.candidate_manifest = Some(Manifest::empty());
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };
        std::fs::rename(&play_counts, &play_counts_backup).unwrap();

        coordinator
            .rollback_to_ready(
                &mut journal,
                &journal_store,
                &snapshot,
                anyhow::anyhow!("injected verification failure"),
            )
            .unwrap();

        assert_eq!(std::fs::read(&pending).unwrap(), b"moved transcode");
        assert!(!published.exists());
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert_eq!(journal.staged_files[0].dbid, 0);
        assert!(journal.staged_files[0].final_ipod_path.is_none());
        assert_eq!(journal_store.load(19).unwrap(), journal);
    }

    #[test]
    fn retryable_rollback_preserves_a_published_file_referenced_by_restored_database() {
        let (mount, _host, manifest_store, cache, _manifest) =
            coordinator_fixture("rollback-preserves-referenced-final");
        let retained = mount.join("iPod_Control/Music/F00/retained.m4a");
        std::fs::write(&retained, b"referenced published copy").unwrap();
        let db = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        unsafe {
            let track = crate::ffi::itdb_track_new();
            assert!(!track.is_null());
            (*track).dbid = 72;
            (*track).ipod_path =
                crate::ffi::g_strdup(c":iPod_Control:Music:F00:retained.m4a".as_ptr());
            crate::ffi::itdb_track_add(db.as_ptr(), track, -1);
        }
        db.write().unwrap();
        drop(db);

        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(20)).unwrap();
        let pending = journal_store
            .path(20)
            .with_file_name("20.staged")
            .join("track.m4a");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, b"pending transcode").unwrap();
        let mutation_session = mutation_session(&mount);
        let generation = mutation_session.current_generation().unwrap();
        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(20, TEST_DEVICE_ID, vec![album]);
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(generation.clone());
        journal.verified_generation = Some(generation);
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending.clone(),
            Some(retained.clone()),
            72,
        ));
        journal.candidate_manifest = Some(Manifest::empty());
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };

        coordinator
            .rollback_to_ready(
                &mut journal,
                &journal_store,
                &snapshot,
                anyhow::anyhow!("injected verification failure"),
            )
            .unwrap();

        assert_eq!(std::fs::read(pending).unwrap(), b"pending transcode");
        assert_eq!(
            std::fs::read(retained).unwrap(),
            b"referenced published copy"
        );
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
    }

    #[test]
    fn terminal_verified_mismatch_rollback_removes_all_staged_artifacts() {
        let (mount, _host, manifest_store, cache, _manifest) =
            coordinator_fixture("terminal-rollback-cleans-staged");
        let journal_store = PendingSessionStore::new(&mount);
        let snapshot = RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(19)).unwrap();
        let pending = journal_store
            .path(19)
            .with_file_name("19.staged")
            .join("track.m4a");
        let published = mount.join("iPod_Control/Music/F00/published.m4a");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, b"pending transcode").unwrap();
        std::fs::write(&published, b"published copy").unwrap();

        let mutation_session = mutation_session(&mount);
        let generation = mutation_session.current_generation().unwrap();
        let mut album = PendingAlbum::new("album", 0);
        album.staged_file_indices.push(0);
        let mut journal = PendingSession::new(19, TEST_DEVICE_ID, vec![album]);
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(generation.clone());
        journal.verified_generation = Some(generation);
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending.clone(),
            Some(published.clone()),
            41,
        ));
        journal.candidate_manifest = Some(Manifest::empty());
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };

        coordinator
            .rollback_after_verified_mismatch(
                &mut journal,
                &journal_store,
                &snapshot,
                anyhow::anyhow!("injected terminal verification mismatch"),
            )
            .unwrap();

        assert!(!pending.exists());
        assert!(!published.exists());
        assert_eq!(journal.phase, PendingPhase::RollbackComplete);
        assert_eq!(journal_store.load(19).unwrap(), journal);
    }

    #[test]
    fn recovery_abandons_candidate_when_firmware_removed_only_an_ithmb_output() {
        let (mount, _host, manifest_store, cache, mut manifest) =
            coordinator_fixture("firmware-artwork-normalization");
        let journal_store = PendingSessionStore::new(&mount);
        let baseline_session = mutation_session(&mount);
        let generation_before = baseline_session.current_generation().unwrap();
        drop(baseline_session);
        RollbackSnapshot::create(&mount, &journal_store.snapshot_dir(33)).unwrap();

        let artwork = mount.join("iPod_Control/Artwork");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::write(artwork.join("ArtworkDB"), b"candidate artwork database").unwrap();
        std::fs::write(artwork.join("F1027_1.ithmb"), b"full artwork").unwrap();
        std::fs::write(artwork.join("F1031_1.ithmb"), b"thumbnail artwork").unwrap();
        let staged_dir = journal_store.staged_dir(33);
        std::fs::create_dir_all(&staged_dir).unwrap();

        let published_session = mutation_session(&mount);
        let verified_generation = published_session.current_generation().unwrap();
        drop(published_session);
        let mut journal = PendingSession::new(33, TEST_DEVICE_ID, Vec::new());
        journal.phase = PendingPhase::DatabaseVerified;
        journal.generation_before = Some(generation_before.clone());
        journal.verified_generation = Some(verified_generation);
        journal.candidate_manifest = Some(manifest.clone());
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
        journal_store.save(&journal).unwrap();

        std::fs::remove_file(artwork.join("F1027_1.ithmb")).unwrap();
        let play_counts = mount.join("iPod_Control/iTunes/Play Counts");
        std::fs::write(&play_counts, b"firmware playback state").unwrap();
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();

        let recovered = coordinator
            .recover_pending_with_options(&mut manifest, &progress, PublishOptions::default())
            .unwrap();
        progress.finish(true).unwrap();

        assert_eq!(recovered.len(), 1);
        assert!(!journal_store.path(33).exists());
        assert!(!journal_store.snapshot_dir(33).exists());
        assert!(!staged_dir.exists());
        assert!(!artwork.join("ArtworkDB").exists());
        assert!(!artwork.join("F1031_1.ithmb").exists());
        assert_eq!(
            std::fs::read(&play_counts).unwrap(),
            b"firmware playback state"
        );
        let restored = mutation_session.current_generation().unwrap();
        assert!(generation_matches_baseline_plus_runtime(
            &generation_before,
            &restored,
            &restored,
        ));
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
        let mut journal = PendingSession::new(13, TEST_DEVICE_ID, vec![album]);
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
            transcode_profile: None,
        });
        journal.staged_files.push(staged);

        let desired = vec![("mix".to_string(), "Mix".to_string(), vec![source.clone()])];
        let state_root = host.join("state");
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
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
                    playlist_failure_point: None,
                    rockbox_compat: false,
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
    fn ownership_failure_keeps_journal_and_recovery_reuses_verified_ids() {
        let (mount, host, manifest_store, cache, mut manifest) =
            coordinator_fixture("playlist-ownership-recovery");
        let state_root = host.join("state");
        let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
        let store = PendingSessionStore::new(&mount);
        let mut journal = PendingSession::new(15, TEST_DEVICE_ID, Vec::new());
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();

        let error = coordinator
            .publish_with_options(
                &mut journal,
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                    playlist_failure_point: Some(PlaylistFailurePoint::BeforeDeviceOwnershipRename),
                    rockbox_compat: false,
                },
            )
            .unwrap_err();
        progress.finish(false).unwrap();

        assert!(format!("{error:#}").contains("publish device playlist ownership"));
        let pending = store.load(15).unwrap();
        assert_eq!(pending.phase, PendingPhase::RockboxProjectionsPrepared);
        let candidate_id = pending
            .candidate_playlist_ownership
            .as_ref()
            .unwrap()
            .playlists["mix"]
            .apple_playlist_id;
        assert!(!crate::ipod::layout::managed_playlists_path(&mount).exists());

        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let recovered = coordinator
            .recover_pending_with_options(
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                    playlist_failure_point: None,
                    rockbox_compat: false,
                },
            )
            .unwrap();
        progress.finish(true).unwrap();

        assert_eq!(recovered.len(), 1);
        let ownership =
            playlist_publication::ownership_store(&mount, TEST_DEVICE_ID, Some(&state_root))
                .unwrap()
                .load_device()
                .unwrap();
        assert_eq!(ownership.playlists["mix"].apple_playlist_id, candidate_id);
        assert_eq!(
            crate::ipod::db::list_playlists(&crate::ipod::db::OwnedDb::open(&mount).unwrap())
                .iter()
                .filter(|(name, is_master)| name == "Mix" && !is_master)
                .count(),
            1
        );
        assert!(!store.path(15).exists());
    }

    #[test]
    fn playlist_failure_boundaries_retain_the_exact_recovery_phase() {
        let cases = [
            (
                PlaylistFailurePoint::BeforeDatabaseWrite,
                Some(PendingPhase::ReadyToPublish),
            ),
            (
                PlaylistFailurePoint::AfterDatabaseVerified,
                Some(PendingPhase::DatabaseVerified),
            ),
            (
                PlaylistFailurePoint::BeforeProjectionPlanPersist,
                Some(PendingPhase::DeviceManifestPublished),
            ),
            (
                PlaylistFailurePoint::AfterProjectionPlanPrepared,
                Some(PendingPhase::RockboxProjectionsPrepared),
            ),
            (
                PlaylistFailurePoint::BeforeDeviceOwnershipRename,
                Some(PendingPhase::RockboxProjectionsPrepared),
            ),
            (
                PlaylistFailurePoint::AfterDeviceOwnershipRename,
                Some(PendingPhase::PlaylistOwnershipPublished),
            ),
            (PlaylistFailurePoint::BeforeHostCacheRefresh, None),
        ];

        for (index, (failure, expected_phase)) in cases.into_iter().enumerate() {
            let (mount, host, manifest_store, cache, mut manifest) =
                coordinator_fixture(&format!("playlist-boundary-{index}"));
            let state_root = host.join("state");
            let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
            let store = PendingSessionStore::new(&mount);
            let session_id = 100 + index as u64;
            let mut journal = PendingSession::new(session_id, TEST_DEVICE_ID, Vec::new());
            let mutation_session = mutation_session(&mount);
            let coordinator = CheckpointCoordinator {
                mount: &mount,
                serial: TEST_DEVICE_ID,
                mutation_session: &mutation_session,
                manifest_store: &manifest_store,
                artwork_cache: cache,
            };
            let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
            let result = coordinator.publish_with_options(
                &mut journal,
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                    playlist_failure_point: Some(failure),
                    rockbox_compat: false,
                },
            );

            match expected_phase {
                Some(expected) => {
                    assert!(result.is_err(), "{failure:?} unexpectedly succeeded");
                    assert_eq!(
                        store.load(session_id).unwrap().phase,
                        expected,
                        "{failure:?}"
                    );
                    progress.finish(false).unwrap();
                }
                None => {
                    let result = result.expect("host-cache failure must be warning-only");
                    assert!(result.host_cache_warning.is_some());
                    assert!(!store.path(session_id).exists());
                    progress.finish(true).unwrap();
                }
            }
        }
    }

    #[test]
    fn pre_6b_prepared_projection_journal_without_plan_marker_fails_closed() {
        use crate::ipod::playlist_ownership::{
            ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership,
            RockboxProjectionRecord, MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        };
        use std::collections::BTreeMap;

        let (mount, _host, _manifest_store, _cache, _manifest) =
            coordinator_fixture("pre-6b-recorded-projection");
        let ownership = ManagedPlaylistOwnership {
            schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
            device_serial: TEST_DEVICE_ID.into(),
            playlists: BTreeMap::from([(
                "mix".into(),
                ManagedPlaylistEntry {
                    apple_playlist_id: 41,
                    expected_kind: ManagedPlaylistKind::Normal,
                    rockbox: Some(RockboxProjectionRecord {
                        relative_filename: "Mix--0123456789.m3u8".into(),
                        content_hash: "a".repeat(64),
                    }),
                },
            )]),
        };
        let store = PendingSessionStore::new(&mount);
        let mut journal = PendingSession::new(16, TEST_DEVICE_ID, Vec::new());
        journal.phase = PendingPhase::RockboxProjectionsPrepared;
        journal.candidate_playlist_ownership = Some(ownership.clone());
        journal
            .desired_playlist_memberships
            .insert("mix".into(), Vec::new());
        let error = store.save(&journal).unwrap_err();

        assert!(format!("{error:#}").contains("predates recorded operation planning"));
        assert!(!store.path(16).exists());
    }

    #[test]
    fn recovery_playlist_mismatch_preserves_the_unknown_generation() {
        let (mount, host, manifest_store, cache, mut manifest) =
            coordinator_fixture("playlist-verify-rollback");
        let state_root = host.join("state");
        let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
        let store = PendingSessionStore::new(&mount);
        let mut journal = PendingSession::new(17, TEST_DEVICE_ID, Vec::new());
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
            manifest_store: &manifest_store,
            artwork_cache: cache,
        };
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        coordinator
            .publish_with_options(
                &mut journal,
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                    playlist_failure_point: Some(PlaylistFailurePoint::AfterDatabaseVerified),
                    rockbox_compat: false,
                },
            )
            .unwrap_err();
        progress.finish(false).unwrap();
        let playlist_id = store
            .load(17)
            .unwrap()
            .candidate_playlist_ownership
            .as_ref()
            .unwrap()
            .playlists["mix"]
            .apple_playlist_id;
        let db = crate::ipod::db::OwnedDb::open(&mount).unwrap();
        crate::ipod::db::remove_playlist_by_id(&db, playlist_id).unwrap();
        db.write().unwrap();
        drop(db);
        let unknown_db = std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap();

        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let error = coordinator
            .recover_pending_with_options(
                &mut manifest,
                &progress,
                PublishOptions {
                    desired_playlists: Some(&desired),
                    playlist_state_root: Some(&state_root),
                    device_identity: None,
                    playlist_failure_point: None,
                    rockbox_compat: false,
                },
            )
            .unwrap_err();
        progress.finish(false).unwrap();

        assert!(format!("{error:#}").contains("external_generation_changed"));
        assert_eq!(
            store.load(17).unwrap().phase,
            PendingPhase::DatabaseVerified
        );
        assert_eq!(
            std::fs::read(crate::ipod::layout::itunes_db_path(&mount)).unwrap(),
            unknown_db
        );
        RollbackSnapshot::open(&store.snapshot_dir(17))
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn playlist_record_is_restored_when_checkpoint_rolls_back() {
        let (mount, host, store, cache, mut manifest) = coordinator_fixture("playlist-rollback");
        let state_root = host.join("state");
        let record =
            crate::device_state::managed_playlists_path_in(&state_root, TEST_DEVICE_ID).unwrap();
        let original = br#"{
  "names": [
    { "slug": "old", "name": "Old", "id": 123 }
  ]
}"#;
        std::fs::write(&record, original).unwrap();
        let manifest_path = crate::device_state::portable_manifest_path(&mount);
        std::fs::create_dir_all(&manifest_path).unwrap();

        let mut journal = PendingSession::new(14, TEST_DEVICE_ID, Vec::new());
        let desired = Vec::<DesiredPlaylist>::new();
        let (progress, _decisions) = crate::progress::Progress::start(false, false).unwrap();
        let mutation_session = mutation_session(&mount);
        let coordinator = CheckpointCoordinator {
            mount: &mount,
            serial: TEST_DEVICE_ID,
            mutation_session: &mutation_session,
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
                    playlist_failure_point: None,
                    rockbox_compat: false,
                },
            )
            .is_err());
        progress.finish(false).unwrap();

        assert_eq!(std::fs::read(record).unwrap(), original);
        assert_eq!(journal.phase, PendingPhase::ReadyToPublish);
        assert!(journal.managed_playlist_record_snapshot.is_none());
    }
}
