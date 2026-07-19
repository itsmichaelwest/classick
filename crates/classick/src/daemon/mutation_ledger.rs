use crate::atomic_file::AtomicFileWriter;
use crate::library_index::{LibraryIndex, INDEX_VERSION};
use crate::manifest::Manifest;
use crate::playlist::ManualPlaylist;
use crate::selection::Selection;
use crate::selection::SelectionRule;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;
const MAX_PER_TARGET: usize = 256;

pub(crate) fn fingerprint(target: &MutationTarget, rules: &[SelectionRule]) -> String {
    let target_key = match target {
        MutationTarget::DeviceSelection { serial } => format!(
            "device:{}",
            crate::daemon::device_registry::canonical_serial_key(serial)
        ),
        MutationTarget::ManualPlaylist { slug } => format!("playlist:{slug}"),
    };
    let rule_keys = rules
        .iter()
        .map(|rule| match rule {
            SelectionRule::Artist { name } => format!("artist:{}", name.to_lowercase()),
            SelectionRule::Album { artist, album } => {
                format!("album:{}:{}", artist.to_lowercase(), album.to_lowercase())
            }
            SelectionRule::Genre { name } => format!("genre:{}", name.to_lowercase()),
        })
        .collect::<Vec<_>>();
    blake3::hash(&serde_json::to_vec(&(target_key, rule_keys)).expect("fingerprint serializes"))
        .to_hex()
        .to_string()
}

pub(crate) fn valid_request_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

pub(crate) enum RuleValidationError {
    StaleIndex,
    Invalid(String),
}

pub(crate) fn validate_indexed_rules(
    index: &LibraryIndex,
    rules: &[SelectionRule],
) -> std::result::Result<Vec<SelectionRule>, RuleValidationError> {
    if index.version != INDEX_VERSION
        || index.scanned_at_unix_secs.is_none()
        || index.source_root.as_os_str().is_empty()
    {
        return Err(RuleValidationError::StaleIndex);
    }
    crate::daemon::library_drop::validate_drop_rules(rules)
        .map_err(|error| RuleValidationError::Invalid(error.to_string()))
}

pub(crate) fn missing_count(
    config_root: &Path,
    index: &LibraryIndex,
    connected_mounts: &BTreeMap<String, PathBuf>,
    serial: &str,
    matched: &[String],
) -> usize {
    let key = crate::daemon::device_registry::canonical_serial_key(serial);
    let path = connected_mounts
        .get(&key)
        .map(|mount| crate::device_state::portable_manifest_path(mount))
        .unwrap_or_else(|| {
            crate::device_state::device_manifest_path_in(config_root, serial).unwrap_or_default()
        });
    let Ok(bytes) = std::fs::read(path) else {
        return matched.len();
    };
    let manifest =
        Manifest::decode_v2(&bytes, &index.source_root).or_else(|_| serde_json::from_slice(&bytes));
    let Ok(manifest) = manifest else {
        return matched.len();
    };
    let present = manifest
        .tracks
        .iter()
        .filter_map(|track| track.source_path.strip_prefix(&index.source_root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/").to_lowercase())
        .collect::<HashSet<_>>();
    matched
        .iter()
        .filter(|path| !present.contains(&path.to_lowercase()))
        .count()
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub type MutationRequestId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MutationTarget {
    DeviceSelection { serial: String },
    ManualPlaylist { slug: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceDropOutcome {
    pub request_id: MutationRequestId,
    pub serial: String,
    pub matched_tracks: usize,
    pub missing_tracks: usize,
    pub selection_changed: bool,
    pub selection_revision: u64,
    pub selection: Selection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistDropOutcome {
    pub request_id: MutationRequestId,
    pub slug: String,
    pub appended_tracks: usize,
    pub playlist_revision: u64,
    pub playlist: ManualPlaylist,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationFailureCode {
    InvalidRequestId,
    InvalidRules,
    UnknownDevice,
    UnconfiguredDevice,
    NoLibraryMatches,
    MissingPlaylist,
    NonManualPlaylist,
    CorruptPlaylist,
    RequestIdCollision,
    PersistenceFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MutationFailure {
    pub request_id: String,
    pub target: MutationTarget,
    pub code: MutationFailureCode,
    pub message: String,
}

impl fmt::Display for MutationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}
impl std::error::Error for MutationFailure {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum StoredOutcome {
    Device(DeviceDropOutcome),
    Playlist(PlaylistDropOutcome),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Acknowledgement {
    pub request_id: String,
    pub fingerprint: String,
    pub target: MutationTarget,
    pub acknowledged_at: u64,
    pub outcome: StoredOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerFile {
    version: u32,
    entries: Vec<Acknowledgement>,
}

#[derive(Debug)]
pub(crate) struct MutationLedger {
    path: PathBuf,
    entries: Vec<Acknowledgement>,
}

impl MutationLedger {
    pub(crate) fn load(path: PathBuf) -> Result<Self> {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => {
                let file: LedgerFile = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse mutation ledger {}", path.display()))?;
                anyhow::ensure!(
                    file.version == SCHEMA_VERSION,
                    "unsupported mutation ledger version {}",
                    file.version
                );
                file.entries
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read mutation ledger {}", path.display()))
            }
        };
        Ok(Self { path, entries })
    }

    pub(crate) fn find(&self, request_id: &str) -> Option<&Acknowledgement> {
        self.entries
            .iter()
            .find(|entry| entry.request_id == request_id)
    }

    pub(crate) fn publish(&mut self, acknowledgement: Acknowledgement) -> Result<()> {
        let mut entries = self.entries.clone();
        entries.retain(|entry| entry.request_id != acknowledgement.request_id);
        entries.push(acknowledgement);
        evict(&mut entries);
        let bytes = serde_json::to_vec_pretty(&LedgerFile {
            version: SCHEMA_VERSION,
            entries: entries.clone(),
        })?;
        AtomicFileWriter::new()
            .write(&self.path, &bytes)
            .context("publish library mutation acknowledgement ledger")?;
        self.entries = entries;
        Ok(())
    }
}

fn evict(entries: &mut Vec<Acknowledgement>) {
    let targets = entries
        .iter()
        .map(|entry| entry.target.clone())
        .collect::<std::collections::HashSet<_>>();
    for target in targets {
        let mut positions = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.target == target)
            .map(|(index, entry)| (index, entry.acknowledged_at, entry.request_id.clone()))
            .collect::<Vec<_>>();
        if positions.len() <= MAX_PER_TARGET {
            continue;
        }
        positions.sort_by(|a, b| (a.1, &a.2).cmp(&(b.1, &b.2)));
        let remove = positions.len() - MAX_PER_TARGET;
        let doomed = positions
            .into_iter()
            .take(remove)
            .map(|item| item.0)
            .collect::<std::collections::HashSet<_>>();
        *entries = entries
            .drain(..)
            .enumerate()
            .filter_map(|(index, entry)| (!doomed.contains(&index)).then_some(entry))
            .collect();
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JournalPhase {
    Prepared,
    PayloadPublished,
    RevisionPublished,
    LedgerPublished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MutationJournal {
    pub version: u32,
    pub request_id: String,
    pub fingerprint: String,
    pub target: MutationTarget,
    pub phase: JournalPhase,
    pub payload_path: PathBuf,
    pub old_payload: Option<Vec<u8>>,
    pub new_payload: Vec<u8>,
    pub prior_revision: u64,
    pub new_revision: u64,
    pub outcome: StoredOutcome,
    pub acknowledged_at: u64,
}

impl MutationJournal {
    pub(crate) fn prepared(
        request_id: String,
        fingerprint: String,
        target: MutationTarget,
        payload_path: PathBuf,
        old_payload: Option<Vec<u8>>,
        new_payload: Vec<u8>,
        prior_revision: u64,
        new_revision: u64,
        outcome: StoredOutcome,
        acknowledged_at: u64,
    ) -> Self {
        Self {
            version: SCHEMA_VERSION,
            request_id,
            fingerprint,
            target,
            phase: JournalPhase::Prepared,
            payload_path,
            old_payload,
            new_payload,
            prior_revision,
            new_revision,
            outcome,
            acknowledged_at,
        }
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let journal: Self = serde_json::from_slice(
            &std::fs::read(path)
                .with_context(|| format!("read mutation journal {}", path.display()))?,
        )
        .with_context(|| format!("parse mutation journal {}", path.display()))?;
        anyhow::ensure!(
            journal.version == SCHEMA_VERSION,
            "unsupported mutation journal version {}",
            journal.version
        );
        Ok(journal)
    }

    pub(crate) fn publish(&self, path: &Path) -> Result<()> {
        AtomicFileWriter::new()
            .write(path, &serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("publish mutation journal {}", path.display()))
    }

    pub(crate) fn acknowledgement(&self) -> Acknowledgement {
        Acknowledgement {
            request_id: self.request_id.clone(),
            fingerprint: self.fingerprint.clone(),
            target: self.target.clone(),
            acknowledged_at: self.acknowledged_at,
            outcome: self.outcome.clone(),
        }
    }
}
