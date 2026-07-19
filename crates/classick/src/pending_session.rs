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
pub struct PendingSession {
    pub version: u32,
    pub session_id: SessionId,
    pub serial: String,
    pub phase: PendingPhase,
    pub albums: Vec<PendingAlbum>,
    pub staged_files: Vec<StagedFile>,
    pub obsolete_files: Vec<ObsoleteFile>,
    pub candidate_manifest: Option<Manifest>,
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
            albums,
            staged_files: Vec::new(),
            obsolete_files: Vec::new(),
            candidate_manifest: None,
            managed_playlist_record_snapshot: None,
            candidate_playlist_ownership: None,
            desired_playlist_memberships: BTreeMap::new(),
            verified_playlist_memberships: Vec::new(),
            pending_rockbox_ops: BTreeMap::new(),
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
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove journal {}", path.display())),
        }
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
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove staged file {}", path.display())),
    }
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
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn tempdir(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("pending-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn save_load_is_atomic_and_rejects_corruption() {
        let mount = tempdir("atomic");
        let store = PendingSessionStore::new(&mount);
        let journal = PendingSession::new(41, "SERIAL", Vec::new());
        store.save(&journal).unwrap();
        assert_eq!(store.load(41).unwrap(), journal);

        std::fs::write(store.path(41), b"{broken").unwrap();
        assert!(store.load(41).unwrap_err().to_string().contains("decode"));
    }

    #[test]
    fn recovery_deletes_only_unreferenced_journal_files() {
        let mount = tempdir("foreign");
        let pending = mount.join("pending.m4a");
        let published = mount.join("published.m4a");
        let foreign = mount.join("foreign.m4a");
        for path in [&pending, &published, &foreign] {
            std::fs::write(path, b"audio").unwrap();
        }
        let mut journal = PendingSession::new(42, "SERIAL", Vec::new());
        journal.staged_files.push(StagedFile::minimal(
            PathBuf::from("source.flac"),
            pending.clone(),
            Some(published.clone()),
            7,
        ));
        cleanup_unreferenced_staged_files(&journal, &ReferencedPaths::from([published.clone()]))
            .unwrap();
        assert!(!pending.exists());
        assert!(published.exists());
        assert!(foreign.exists());
    }

    #[test]
    fn albums_are_journaled_in_admission_order() {
        let mut journal = PendingSession::new(
            43,
            "SERIAL",
            vec![
                PendingAlbum::new("second", 1),
                PendingAlbum::new("first", 0),
            ],
        );
        journal.staged_files = vec![
            StagedFile::minimal("second.flac".into(), "second.m4a".into(), None, 0),
            StagedFile::minimal("first.flac".into(), "first.m4a".into(), None, 0),
        ];
        journal.albums[0].staged_file_indices.push(0);
        journal.albums[1].staged_file_indices.push(1);
        assert_eq!(journal.ordered_album_keys(), vec!["first", "second"]);
        assert_eq!(journal.publication_indices().unwrap(), vec![1, 0]);
    }
}
