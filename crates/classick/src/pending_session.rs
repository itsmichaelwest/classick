use crate::atomic_file::AtomicFileWriter;
use crate::ipc_device::SessionId;
use crate::ipod::db::Tags;
use crate::ipod::device_playlists::VerifiedPlaylistMembership;
use crate::ipod::playlist_ownership::{ManagedPlaylistOwnership, RockboxProjectionRecord};
use crate::manifest::{Manifest, ManifestEntry};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const PENDING_SESSION_VERSION: u32 = 1;
pub const ROCKBOX_PROJECTION_PLAN_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingPhase {
    Staging,
    ReadyToPublish,
    DatabaseVerified,
    DeviceManifestPublished,
    RockboxProjectionsPrepared,
    PlaylistOwnershipPublished,
    RockboxProjectionsPublished,
    CleanupComplete,
    RollbackComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRockboxOp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<RockboxProjectionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired: Option<RockboxProjectionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingAlbum {
    pub key: String,
    pub ordinal: usize,
    #[serde(default)]
    pub staged_file_indices: Vec<usize>,
}

impl PendingAlbum {
    pub fn new(key: impl Into<String>, ordinal: usize) -> Self {
        Self {
            key: key.into(),
            ordinal,
            staged_file_indices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedFile {
    pub source: PathBuf,
    pub pending_path: PathBuf,
    pub final_ipod_path: Option<PathBuf>,
    pub dbid: u64,
    pub tags: Tags,
    pub artwork_hash: Option<String>,
    pub candidate_entry: Option<ManifestEntry>,
}

impl StagedFile {
    pub fn minimal(
        source: PathBuf,
        pending_path: PathBuf,
        final_ipod_path: Option<PathBuf>,
        dbid: u64,
    ) -> Self {
        Self {
            source,
            pending_path,
            final_ipod_path,
            dbid,
            tags: Tags::default(),
            artwork_hash: None,
            candidate_entry: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingMetadataUpdate {
    pub tags: Tags,
    pub artwork_hash: Option<String>,
    pub candidate_entry: ManifestEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObsoleteFile {
    pub path: PathBuf,
    pub prior_dbid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedPlaylistRecordSnapshot {
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceManifestPreimage {
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSession {
    pub version: u32,
    pub session_id: SessionId,
    pub serial: String,
    pub phase: PendingPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_before: Option<crate::device_coordination::DeviceGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_generation: Option<crate::device_coordination::DeviceGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_generation: Option<crate::device_coordination::DeviceGeneration>,
    pub albums: Vec<PendingAlbum>,
    pub staged_files: Vec<StagedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_updates: Vec<PendingMetadataUpdate>,
    pub obsolete_files: Vec<ObsoleteFile>,
    pub candidate_manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_manifest_preimage: Option<DeviceManifestPreimage>,
    #[serde(default)]
    pub managed_playlist_record_snapshot: Option<ManagedPlaylistRecordSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_playlist_ownership: Option<ManagedPlaylistOwnership>,
    #[serde(default)]
    pub desired_playlist_memberships: BTreeMap<String, Vec<u64>>,
    #[serde(default)]
    pub verified_playlist_memberships: Vec<VerifiedPlaylistMembership>,
    #[serde(default)]
    pub pending_rockbox_ops: BTreeMap<String, PendingRockboxOp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rockbox_projection_plan_version: Option<u32>,
}

impl PendingSession {
    pub fn new(
        session_id: SessionId,
        serial: impl Into<String>,
        albums: Vec<PendingAlbum>,
    ) -> Self {
        Self {
            version: PENDING_SESSION_VERSION,
            session_id,
            serial: serial.into(),
            phase: PendingPhase::Staging,
            generation_before: None,
            published_generation: None,
            verified_generation: None,
            albums,
            staged_files: Vec::new(),
            metadata_updates: Vec::new(),
            obsolete_files: Vec::new(),
            candidate_manifest: None,
            device_manifest_preimage: None,
            managed_playlist_record_snapshot: None,
            candidate_playlist_ownership: None,
            desired_playlist_memberships: BTreeMap::new(),
            verified_playlist_memberships: Vec::new(),
            pending_rockbox_ops: BTreeMap::new(),
            rockbox_projection_plan_version: None,
        }
    }

    pub fn ordered_album_keys(&self) -> Vec<&str> {
        let mut albums = self.albums.iter().collect::<Vec<_>>();
        albums.sort_by_key(|album| album.ordinal);
        albums.into_iter().map(|album| album.key.as_str()).collect()
    }

    pub fn publication_indices(&self) -> Result<Vec<usize>> {
        let mut albums = self.albums.iter().collect::<Vec<_>>();
        albums.sort_by_key(|album| album.ordinal);
        let indices = albums
            .into_iter()
            .flat_map(|album| album.staged_file_indices.iter().copied())
            .collect::<Vec<_>>();
        if indices.len() != self.staged_files.len() {
            bail!("every staged file must belong to exactly one pending album");
        }
        let unique = indices.iter().copied().collect::<HashSet<_>>();
        if unique.len() != indices.len()
            || unique.iter().any(|index| *index >= self.staged_files.len())
        {
            bail!("pending album staged-file membership is invalid");
        }
        Ok(indices)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != PENDING_SESSION_VERSION {
            bail!("unsupported pending-session version {}", self.version);
        }
        if self.serial.trim().is_empty() {
            bail!("pending-session serial is empty");
        }
        if self.generation_before.is_none()
            && (self.published_generation.is_some() || self.verified_generation.is_some())
        {
            bail!("pending session has a published generation without its predecessor");
        }
        if self.phase < PendingPhase::DatabaseVerified && self.verified_generation.is_some() {
            bail!("pending session records a verified generation before publication");
        }
        if self.generation_before.is_some()
            && self.phase >= PendingPhase::DatabaseVerified
            && self.verified_generation.is_none()
        {
            bail!("coordinated pending session has no verified published generation");
        }
        let mut ordinals = HashSet::new();
        for album in &self.albums {
            if !ordinals.insert(album.ordinal) {
                bail!("duplicate pending album ordinal {}", album.ordinal);
            }
            if album
                .staged_file_indices
                .iter()
                .any(|index| *index >= self.staged_files.len())
            {
                bail!("pending album references an unknown staged file");
            }
        }
        self.publication_indices()?;
        let mut metadata_dbids = HashSet::new();
        for update in &self.metadata_updates {
            if update.candidate_entry.ipod_dbid == 0
                || !metadata_dbids.insert(update.candidate_entry.ipod_dbid)
                || !update.candidate_entry.source_known
                || update.candidate_entry.ipod_relpath.is_empty()
            {
                bail!("pending metadata update is invalid");
            }
        }
        if let Some(candidate) = &self.candidate_playlist_ownership {
            candidate
                .validate_for_serial(&self.serial)
                .context("validate pending candidate playlist ownership")?;
            if candidate.playlists.len() != self.desired_playlist_memberships.len()
                || candidate
                    .playlists
                    .keys()
                    .any(|slug| !self.desired_playlist_memberships.contains_key(slug))
            {
                bail!("pending candidate ownership and desired memberships differ");
            }
        } else if !self.desired_playlist_memberships.is_empty()
            || !self.verified_playlist_memberships.is_empty()
        {
            bail!("pending playlist membership exists without candidate ownership");
        }
        if let Some(version) = self.rockbox_projection_plan_version {
            if version != ROCKBOX_PROJECTION_PLAN_VERSION {
                bail!("unsupported Rockbox projection plan version {version}");
            }
            if self.phase < PendingPhase::RockboxProjectionsPrepared {
                bail!("Rockbox projection plan is marked before its prepared phase");
            }
        } else if (self.phase >= PendingPhase::RockboxProjectionsPrepared
            && self.phase <= PendingPhase::RockboxProjectionsPublished)
            || (self.phase == PendingPhase::CleanupComplete
                && self.candidate_playlist_ownership.is_some())
        {
            bail!("prepared Rockbox projection journal predates recorded operation planning");
        }
        for (slug, operation) in &self.pending_rockbox_ops {
            for record in [&operation.previous, &operation.desired]
                .into_iter()
                .flatten()
            {
                crate::rockbox_playlist::validate_projection_record(record).with_context(|| {
                    format!("validate pending Rockbox projection operation {slug:?}")
                })?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PendingSessionStore {
    mount: PathBuf,
    root: PathBuf,
    writer: AtomicFileWriter,
}

#[derive(Debug)]
pub struct PendingSessionDiscovery {
    pub sessions: Vec<PendingSession>,
    pub rejected: Vec<RejectedPendingSession>,
}

#[derive(Debug)]
pub struct RejectedPendingSession {
    pub path: PathBuf,
    pub reason: String,
}

pub fn has_sync_transaction_material(mount: &Path) -> Result<bool> {
    let root = crate::device_state::pending_sessions_dir(mount);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read pending-session dir {}", root.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", root.display()))?;
        let name = entry.file_name();
        if name == "portable-config" || is_appledouble_sidecar(Path::new(&name)) {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

/// macOS writes AppleDouble sidecars beside anything it touches on the iPod's
/// FAT volume, including directories Classick never rewrites. They are never
/// transaction material, so every pending-material check must skip them or a
/// sync wedges on journal-less material that recovery cannot resolve.
fn is_appledouble_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("._"))
}

impl PendingSessionStore {
    pub fn new(mount: impl AsRef<Path>) -> Self {
        let mount = mount.as_ref().to_path_buf();
        Self {
            root: crate::device_state::pending_sessions_dir(&mount),
            mount,
            writer: AtomicFileWriter::new(),
        }
    }

    pub fn path(&self, session_id: SessionId) -> PathBuf {
        self.root.join(format!("{session_id}.json"))
    }

    pub fn snapshot_dir(&self, session_id: SessionId) -> PathBuf {
        self.root.join(format!("{session_id}.snapshot"))
    }

    pub fn staged_dir(&self, session_id: SessionId) -> PathBuf {
        self.root.join(format!("{session_id}.staged"))
    }

    pub fn save(&self, session: &PendingSession) -> Result<()> {
        session.validate()?;
        let bytes = serde_json::to_vec_pretty(session).context("encode pending-session journal")?;
        self.writer
            .write(&self.path(session.session_id), &bytes)
            .context("write pending-session journal")
    }

    pub fn load(&self, session_id: SessionId) -> Result<PendingSession> {
        let path = self.path(session_id);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read pending-session journal {}", path.display()))?;
        let session: PendingSession = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode pending-session journal {}", path.display()))?;
        session.validate()?;
        Ok(session)
    }

    pub fn discover(&self, serial: &str) -> Result<PendingSessionDiscovery> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PendingSessionDiscovery {
                    sessions: Vec::new(),
                    rejected: Vec::new(),
                });
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read pending-session dir {}", self.root.display()));
            }
        };
        let mut journal_paths = Vec::new();
        for entry in entries {
            let path = entry
                .with_context(|| format!("read entry in {}", self.root.display()))?
                .path();
            if is_appledouble_sidecar(&path) {
                continue;
            }
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                journal_paths.push(path);
            }
        }
        journal_paths.sort();

        let mut discovery = PendingSessionDiscovery {
            sessions: Vec::new(),
            rejected: Vec::new(),
        };
        for path in journal_paths {
            let loaded = (|| -> Result<PendingSession> {
                let file_session_id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .context("pending-session filename is not valid UTF-8")?
                    .parse::<SessionId>()
                    .context("pending-session filename is not a session id")?;
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("read pending-session journal {}", path.display()))?;
                let session: PendingSession =
                    serde_json::from_slice(&bytes).with_context(|| {
                        format!("decode pending-session journal {}", path.display())
                    })?;
                session.validate()?;
                if session.session_id != file_session_id {
                    bail!(
                        "pending-session filename id {} does not match journal id {}",
                        file_session_id,
                        session.session_id
                    );
                }
                if session.serial != serial {
                    bail!(
                        "pending-session serial {:?} does not exactly match connected device {:?}",
                        session.serial,
                        serial
                    );
                }
                self.validate_discovered_paths(&session)?;
                Ok(session)
            })();
            match loaded {
                Ok(session) => discovery.sessions.push(session),
                Err(error) => discovery.rejected.push(RejectedPendingSession {
                    path,
                    reason: format!("{error:#}"),
                }),
            }
        }
        discovery.sessions.sort_by_key(|session| session.session_id);
        discovery
            .rejected
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(discovery)
    }

    fn validate_discovered_paths(&self, session: &PendingSession) -> Result<()> {
        let staged_root = self.root.join(format!("{}.staged", session.session_id));
        let music_root = self.mount.join("iPod_Control").join("Music");
        for staged in &session.staged_files {
            if !is_descendant(&staged.pending_path, &staged_root) {
                bail!(
                    "pending-session staged path {} is outside {}",
                    staged.pending_path.display(),
                    staged_root.display()
                );
            }
            if let Some(path) = &staged.final_ipod_path {
                if !is_descendant(path, &music_root) {
                    bail!(
                        "pending-session published path {} is outside {}",
                        path.display(),
                        music_root.display()
                    );
                }
            }
        }
        for obsolete in &session.obsolete_files {
            if !is_descendant(&obsolete.path, &music_root) {
                bail!(
                    "pending-session obsolete path {} is outside {}",
                    obsolete.path.display(),
                    music_root.display()
                );
            }
        }
        Ok(())
    }

    pub fn remove(&self, session_id: SessionId) -> Result<()> {
        let path = self.path(session_id);
        remove_file_if_present(&path)
            .with_context(|| format!("remove journal {}", path.display()))?;
        remove_file_if_present(&appledouble_sibling(&path))
            .with_context(|| format!("remove journal metadata {}", path.display()))
    }
}

#[derive(Debug, Default)]
pub struct ReferencedPaths(HashSet<PathBuf>);

impl<const N: usize> From<[PathBuf; N]> for ReferencedPaths {
    fn from(paths: [PathBuf; N]) -> Self {
        Self(paths.into_iter().map(normalize_path).collect())
    }
}

impl FromIterator<PathBuf> for ReferencedPaths {
    fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
        Self(iter.into_iter().map(normalize_path).collect())
    }
}

impl ReferencedPaths {
    pub fn contains(&self, path: &Path) -> bool {
        self.0.contains(&normalize_path(path.to_path_buf()))
    }
}

pub fn cleanup_unreferenced_staged_files(
    journal: &PendingSession,
    referenced: &ReferencedPaths,
) -> Result<()> {
    for staged in &journal.staged_files {
        remove_if_unreferenced(&staged.pending_path, referenced)?;
        if let Some(path) = &staged.final_ipod_path {
            remove_if_unreferenced(path, referenced)?;
        }
    }
    Ok(())
}

fn remove_if_unreferenced(path: &Path, referenced: &ReferencedPaths) -> Result<()> {
    if referenced.contains(path) {
        return Ok(());
    }
    remove_file_if_present(path)
        .with_context(|| format!("remove staged file {}", path.display()))?;
    remove_file_if_present(&appledouble_sibling(path))
        .with_context(|| format!("remove staged file metadata {}", path.display()))
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove file {}", path.display())),
    }
}

fn appledouble_sibling(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    path.with_file_name(format!("._{}", name.to_string_lossy()))
}

fn normalize_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn is_descendant(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    !components.is_empty()
        && components
            .iter()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
#[path = "pending_session/tests.rs"]
mod tests;
