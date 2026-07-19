use crate::atomic_file::AtomicFileWriter;
use crate::daemon::device_registry::DeviceRegistry;
use crate::daemon::library_drop::{add_rules_to_selection, append_rules_to_manual};
use crate::daemon::mutation_ledger::{
    fingerprint, missing_count, now, valid_request_id, validate_indexed_rules, JournalPhase,
    MutationJournal, MutationLedger, RuleValidationError, StoredOutcome,
};
use crate::library_index::LibraryIndex;
use crate::playlist::{ManualPlaylist, Playlist, PlaylistStore};
use crate::selection::{Selection, SelectionRule};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

pub use crate::daemon::mutation_ledger::{
    DeviceDropOutcome, MutationFailure, MutationFailureCode, MutationRequestId, MutationTarget,
    PlaylistDropOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailurePoint {
    None,
    AfterPrepared,
    AfterPayload,
    AfterRevision,
    AfterLedger,
}

pub struct LibraryMutationService {
    config_root: PathBuf,
    index: LibraryIndex,
    registry: DeviceRegistry,
    playlists: PlaylistStore,
    ledger: MutationLedger,
    connected_mounts: BTreeMap<String, PathBuf>,
    failure_point: FailurePoint,
}

impl LibraryMutationService {
    pub fn open(config_root: PathBuf, index: LibraryIndex) -> Result<Self> {
        let registry =
            DeviceRegistry::load_or_migrate(config_root.join("devices/registry.json"), None)?;
        let playlists = PlaylistStore::open(config_root.join("playlists"))?;
        let ledger = MutationLedger::load(config_root.join("devices/library-mutation-acks.json"))?;
        Ok(Self {
            config_root,
            index,
            registry,
            playlists,
            ledger,
            connected_mounts: BTreeMap::new(),
            failure_point: FailurePoint::None,
        })
    }

    pub fn set_connected_mount(&mut self, serial: &str, mount: Option<PathBuf>) {
        let key = crate::daemon::device_registry::canonical_serial_key(serial);
        match mount {
            Some(mount) => {
                self.connected_mounts.insert(key, mount);
            }
            None => {
                self.connected_mounts.remove(&key);
            }
        }
    }

    #[doc(hidden)]
    pub fn fail_after_phase_once(&mut self, phase: &str) {
        self.failure_point = match phase {
            "prepared" => FailurePoint::AfterPrepared,
            "payload_published" => FailurePoint::AfterPayload,
            "revision_published" => FailurePoint::AfterRevision,
            "ledger_published" => FailurePoint::AfterLedger,
            _ => FailurePoint::None,
        };
    }

    pub fn add_selection_to_device(
        &mut self,
        request_id: &str,
        serial: &str,
        rules: &[SelectionRule],
    ) -> std::result::Result<DeviceDropOutcome, MutationFailure> {
        let target = MutationTarget::DeviceSelection {
            serial: serial.to_string(),
        };
        let canonical = self.validate(request_id, &target, rules)?;
        let fingerprint = fingerprint(&target, &canonical);
        if let Some(outcome) = self.replay(request_id, &target, &fingerprint)? {
            return match outcome {
                StoredOutcome::Device(mut value) => {
                    value.selection_changed = false;
                    value.selection_revision = self
                        .registry
                        .record(serial)
                        .map(|r| r.selection_revision)
                        .unwrap_or(value.selection_revision);
                    value.selection = self.device_selection(serial);
                    Ok(value)
                }
                _ => Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::RequestIdCollision,
                    "request ID was acknowledged for another mutation kind",
                )),
            };
        }
        let record = self.registry.record(serial).cloned().ok_or_else(|| {
            self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::UnknownDevice,
                "unknown device",
            )
        })?;
        if !record.configured {
            return Err(self.failure(
                request_id,
                target,
                MutationFailureCode::UnconfiguredDevice,
                "device is not configured",
            ));
        }
        let payload_path =
            crate::device_state::device_selection_path_in(&self.config_root, &record.serial)
                .map_err(|e| self.persistence(request_id, target.clone(), e))?;
        let current = crate::selection::load_or_all(&payload_path);
        let mutation = add_rules_to_selection(&current, &canonical, &self.index).map_err(|e| {
            self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::InvalidRules,
                e.to_string(),
            )
        })?;
        if mutation.matched_paths.is_empty() {
            return Err(self.failure(
                request_id,
                target,
                MutationFailureCode::NoLibraryMatches,
                "drop rules match no indexed library tracks",
            ));
        }
        let new_revision = record
            .selection_revision
            .checked_add(u64::from(mutation.selection_changed))
            .ok_or_else(|| {
                self.failure(
                    request_id,
                    target.clone(),
                    MutationFailureCode::PersistenceFailed,
                    "selection revision overflow",
                )
            })?;
        let outcome = DeviceDropOutcome {
            request_id: request_id.into(),
            serial: record.serial.clone(),
            matched_tracks: mutation.matched_paths.len(),
            missing_tracks: missing_count(
                &self.config_root,
                &self.index,
                &self.connected_mounts,
                &record.serial,
                &mutation.matched_paths,
            ),
            selection_changed: mutation.selection_changed,
            selection_revision: new_revision,
            selection: mutation.selection.clone(),
        };
        let new_payload = serde_json::to_vec_pretty(&mutation.selection)
            .map_err(|e| self.persistence(request_id, target.clone(), e))?;
        let old_payload = std::fs::read(&payload_path).ok();
        let journal = MutationJournal::prepared(
            request_id.into(),
            fingerprint,
            target.clone(),
            payload_path,
            old_payload,
            new_payload,
            record.selection_revision,
            new_revision,
            StoredOutcome::Device(outcome.clone()),
            now(),
        );
        self.execute(journal)
            .map_err(|e| self.persistence(request_id, target, e))?;
        Ok(outcome)
    }

    pub fn append_selection_to_playlist(
        &mut self,
        request_id: &str,
        slug: &str,
        rules: &[SelectionRule],
    ) -> std::result::Result<PlaylistDropOutcome, MutationFailure> {
        let target = MutationTarget::ManualPlaylist {
            slug: slug.to_string(),
        };
        let canonical = self.validate(request_id, &target, rules)?;
        if crate::playlist::slugify(slug) != slug {
            return Err(self.failure(
                request_id,
                target,
                MutationFailureCode::MissingPlaylist,
                "playlist slug is not canonical",
            ));
        }
        let fingerprint = fingerprint(&target, &canonical);
        if let Some(outcome) = self.replay(request_id, &target, &fingerprint)? {
            return match outcome {
                StoredOutcome::Playlist(mut value) => {
                    value.playlist_revision = self
                        .playlists
                        .playlist_revision(slug)
                        .unwrap_or(value.playlist_revision);
                    if let Some(current) = self.manual_playlist(slug) {
                        value.playlist = current;
                    }
                    Ok(value)
                }
                _ => Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::RequestIdCollision,
                    "request ID was acknowledged for another mutation kind",
                )),
            };
        }
        let current = match self.playlists.load(slug) {
            Ok(Some(Playlist::Manual(value))) if value.skipped_unsafe == 0 => value,
            Ok(Some(Playlist::Manual(_))) => {
                return Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::CorruptPlaylist,
                    "manual playlist contains unsafe entries",
                ))
            }
            Ok(Some(Playlist::Smart(_))) => {
                return Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::NonManualPlaylist,
                    "smart playlists cannot be appended",
                ))
            }
            Ok(None) => {
                return Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::MissingPlaylist,
                    "playlist does not exist",
                ))
            }
            Err(error) => {
                return Err(self.failure(
                    request_id,
                    target,
                    MutationFailureCode::CorruptPlaylist,
                    error.to_string(),
                ))
            }
        };
        let (next, appended) =
            append_rules_to_manual(&current, &canonical, &self.index).map_err(|e| {
                self.failure(
                    request_id,
                    target.clone(),
                    MutationFailureCode::InvalidRules,
                    e.to_string(),
                )
            })?;
        if appended.is_empty() {
            return Err(self.failure(
                request_id,
                target,
                MutationFailureCode::NoLibraryMatches,
                "drop rules add no indexed library tracks",
            ));
        }
        let prior_revision = self
            .playlists
            .playlist_revision(slug)
            .map_err(|e| self.persistence(request_id, target.clone(), e))?;
        let new_revision = prior_revision.checked_add(1).ok_or_else(|| {
            self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::PersistenceFailed,
                "playlist revision overflow",
            )
        })?;
        let outcome = PlaylistDropOutcome {
            request_id: request_id.into(),
            slug: slug.into(),
            appended_tracks: appended.len(),
            playlist_revision: new_revision,
            playlist: next.clone(),
        };
        let path = self.playlists.manual_path(slug);
        let journal = MutationJournal::prepared(
            request_id.into(),
            fingerprint,
            target.clone(),
            path.clone(),
            std::fs::read(&path).ok(),
            self.playlists.encode_manual(&next),
            prior_revision,
            new_revision,
            StoredOutcome::Playlist(outcome.clone()),
            now(),
        );
        self.execute(journal)
            .map_err(|e| self.persistence(request_id, target, e))?;
        Ok(outcome)
    }

    pub fn recover_pending(&mut self) -> Result<()> {
        let dir = self.journal_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut paths = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().and_then(|v| v.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            self.roll_forward(path)?;
        }
        Ok(())
    }

    pub fn device_selection(&self, serial: &str) -> Selection {
        crate::device_state::device_selection_path_in(&self.config_root, serial)
            .map(|p| crate::selection::load_or_all(&p))
            .unwrap_or_else(|_| Selection::all())
    }
    pub fn manual_playlist(&self, slug: &str) -> Option<ManualPlaylist> {
        match self.playlists.load(slug).ok().flatten()? {
            Playlist::Manual(p) => Some(p),
            _ => None,
        }
    }
    pub fn playlist_revision(&self, slug: &str) -> u64 {
        self.playlists.playlist_revision(slug).unwrap_or(0)
    }

    fn validate(
        &self,
        request_id: &str,
        target: &MutationTarget,
        rules: &[SelectionRule],
    ) -> std::result::Result<Vec<SelectionRule>, MutationFailure> {
        if !valid_request_id(request_id) {
            return Err(self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::InvalidRequestId,
                "request ID must be a lowercase UUID",
            ));
        }
        validate_indexed_rules(&self.index, rules).map_err(|error| match error {
            RuleValidationError::StaleIndex => self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::PersistenceFailed,
                "library index is not an authoritative completed scan",
            ),
            RuleValidationError::Invalid(message) => self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::InvalidRules,
                message,
            ),
        })
    }

    fn replay(
        &self,
        request_id: &str,
        target: &MutationTarget,
        fingerprint: &str,
    ) -> std::result::Result<Option<StoredOutcome>, MutationFailure> {
        let Some(entry) = self.ledger.find(request_id) else {
            return Ok(None);
        };
        if entry.target != *target || entry.fingerprint != fingerprint {
            return Err(self.failure(
                request_id,
                target.clone(),
                MutationFailureCode::RequestIdCollision,
                "request ID was already used with a different target or rules",
            ));
        }
        Ok(Some(entry.outcome.clone()))
    }

    fn execute(&mut self, journal: MutationJournal) -> Result<()> {
        std::fs::create_dir_all(self.journal_dir())?;
        let path = self.journal_path(&journal.request_id);
        journal.publish(&path)?;
        if self.failure_point == FailurePoint::AfterPrepared {
            self.failure_point = FailurePoint::None;
            anyhow::bail!("injected failure after journal preparation");
        }
        self.roll_forward(path)
    }

    fn roll_forward(&mut self, path: PathBuf) -> Result<()> {
        let mut journal = MutationJournal::load(&path)?;
        if journal.phase == JournalPhase::Prepared {
            match &journal.target {
                MutationTarget::ManualPlaylist { slug } => self
                    .playlists
                    .publish_manual_bytes(slug, &journal.new_payload)?,
                MutationTarget::DeviceSelection { .. } => {
                    AtomicFileWriter::new().write(&journal.payload_path, &journal.new_payload)?
                }
            }
            if self.failure_point == FailurePoint::AfterPayload {
                self.failure_point = FailurePoint::None;
                anyhow::bail!("injected failure after payload publication");
            }
            journal.phase = JournalPhase::PayloadPublished;
            journal.publish(&path)?;
        }
        if journal.phase == JournalPhase::PayloadPublished {
            match &journal.target {
                MutationTarget::DeviceSelection { serial } => self
                    .registry
                    .publish_selection_revision(serial, journal.new_revision)?,
                MutationTarget::ManualPlaylist { slug } => self
                    .playlists
                    .publish_playlist_revision(slug, journal.new_revision)?,
            }
            journal.phase = JournalPhase::RevisionPublished;
            journal.publish(&path)?;
            if self.failure_point == FailurePoint::AfterRevision {
                self.failure_point = FailurePoint::None;
                anyhow::bail!("injected failure after revision publication");
            }
        }
        if journal.phase == JournalPhase::RevisionPublished {
            self.ledger.publish(journal.acknowledgement())?;
            journal.phase = JournalPhase::LedgerPublished;
            journal.publish(&path)?;
            if self.failure_point == FailurePoint::AfterLedger {
                self.failure_point = FailurePoint::None;
                anyhow::bail!("injected failure after ledger publication");
            }
        }
        if journal.phase == JournalPhase::LedgerPublished {
            std::fs::remove_file(&path)
                .with_context(|| format!("remove completed mutation journal {}", path.display()))?;
        }
        Ok(())
    }

    fn journal_dir(&self) -> PathBuf {
        self.config_root.join("devices/library-mutations")
    }
    fn journal_path(&self, request_id: &str) -> PathBuf {
        self.journal_dir().join(format!("{request_id}.json"))
    }
    fn failure(
        &self,
        request_id: &str,
        target: MutationTarget,
        code: MutationFailureCode,
        message: impl Into<String>,
    ) -> MutationFailure {
        MutationFailure {
            request_id: request_id.into(),
            target,
            code,
            message: message.into(),
        }
    }
    fn persistence(
        &self,
        request_id: &str,
        target: MutationTarget,
        error: impl fmt::Display,
    ) -> MutationFailure {
        self.failure(
            request_id,
            target,
            MutationFailureCode::PersistenceFailed,
            error.to_string(),
        )
    }
}
