use anyhow::{Context, Result};
use classick::artwork_cache::ArtworkCache;
use classick::atomic_file::AtomicFileWriter;
use classick::ffi;
use classick::ipod::device_playlists::VerifiedPlaylistMembership;
use classick::ipod::playlist_ownership::{
    DeviceOwnershipStore, ManagedPlaylistOwnership, RockboxProjectionRecord,
};
use classick::manifest::{Manifest, ManifestEntry};
use classick::manifest_store::ManifestStore;
use classick::pending_session::{PendingSession, PendingSessionStore};
use classick::progress::Progress;
use classick::rockbox_projection_fs::{DeviceProjectionFs, ProjectionFailurePoint};
use classick::sync_transaction::{CheckpointCoordinator, PlaylistFailurePoint, PublishOptions};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

const SERIAL: &str = "SERIAL";

#[derive(Clone)]
pub struct TestPlaylist {
    slug: String,
    name: String,
    ordered_track_indexes: Vec<usize>,
}

pub fn playlist(slug: &str, name: &str, ordered_track_indexes: &[usize]) -> TestPlaylist {
    TestPlaylist {
        slug: slug.into(),
        name: name.into(),
        ordered_track_indexes: ordered_track_indexes.to_vec(),
    }
}

#[derive(Clone, Copy)]
pub enum FailurePoint {
    BeforeOwnershipPublish,
    ProjectionWrite,
    ProjectionRename,
    ProjectionDelete,
    ProjectionDeleteCleanup,
}

pub struct SyncResult {
    pub verified: Vec<VerifiedPlaylistMembership>,
    pub ownership: ManagedPlaylistOwnership,
    pub completed: bool,
}

pub struct Harness {
    pub root: PathBuf,
    pub mount: PathBuf,
    pub serial: String,
    pub ownership_store: DeviceOwnershipStore,
    pub journal_store: PendingSessionStore,
    pub apple_write_count: Arc<AtomicUsize>,
    host: PathBuf,
    manifest_store: ManifestStore,
    artwork_cache: ArtworkCache,
    manifest: Manifest,
    source_paths: Vec<PathBuf>,
    next_session: u64,
    failure: Option<FailurePoint>,
    last_desired: Vec<TestPlaylist>,
    last_enabled: bool,
}

impl Harness {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "rockbox-projection-integration-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        let mount = root.join("mount");
        let host = root.join("host");
        let source = host.join("source");
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
        std::fs::create_dir_all(mount.join("Playlists/Classick")).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        let source_paths = vec![source.join("zero.flac"), source.join("one.flac")];
        let manifest = write_database_and_tracks(&mount, &source_paths);
        let manifest_store = ManifestStore::new(
            mount.clone(),
            SERIAL.into(),
            host.join("manifest.json"),
            host.join("legacy.json"),
            AtomicFileWriter::new(),
        );
        let artwork_cache = ArtworkCache::new(host.join("artwork"));
        for source_path in &source_paths {
            artwork_cache.record_no_art(source_path).unwrap();
        }
        let ownership_store = DeviceOwnershipStore::new(
            mount.clone(),
            SERIAL.into(),
            host.join("managed-playlists.json"),
            AtomicFileWriter::new(),
        );
        Self {
            root,
            mount: mount.clone(),
            serial: SERIAL.into(),
            ownership_store,
            journal_store: PendingSessionStore::new(&mount),
            apple_write_count: Arc::new(AtomicUsize::new(0)),
            host: host.clone(),
            manifest_store,
            artwork_cache,
            manifest,
            source_paths,
            next_session: 1,
            failure: None,
            last_desired: Vec::new(),
            last_enabled: false,
        }
    }

    pub fn sync(&mut self, enabled: bool, playlists: Vec<TestPlaylist>) -> Result<SyncResult> {
        self.last_enabled = enabled;
        self.last_desired = playlists.clone();
        let desired = self.desired(&playlists);
        let mut journal = PendingSession::new(self.next_session, &self.serial, Vec::new());
        self.next_session += 1;
        self.apple_write_count.fetch_add(1, Ordering::SeqCst);
        let playlist_failure = self.install_failure();
        let coordinator = CheckpointCoordinator {
            mount: &self.mount,
            serial: &self.serial,
            manifest_store: &self.manifest_store,
            artwork_cache: self.artwork_cache.clone(),
        };
        let (progress, _decisions) = Progress::start(false, false)?;
        let result = coordinator.publish_with_options(
            &mut journal,
            &mut self.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&self.host),
                device_identity: None,
                playlist_failure_point: playlist_failure,
                rockbox_compat: enabled,
            },
        );
        progress.finish(result.is_ok())?;
        result?;
        Ok(SyncResult {
            verified: journal.verified_playlist_memberships.clone(),
            ownership: self.ownership(),
            completed: self.journal().is_none(),
        })
    }

    pub fn recover(&mut self) -> Result<SyncResult> {
        let pending = self.journal().context("no pending projection journal")?;
        let desired = self.desired(&self.last_desired);
        let coordinator = CheckpointCoordinator {
            mount: &self.mount,
            serial: &self.serial,
            manifest_store: &self.manifest_store,
            artwork_cache: self.artwork_cache.clone(),
        };
        let (progress, _decisions) = Progress::start(false, false)?;
        let result = coordinator.recover_pending_with_options(
            &mut self.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&self.host),
                device_identity: None,
                playlist_failure_point: None,
                rockbox_compat: self.last_enabled,
            },
        );
        progress.finish(result.is_ok())?;
        result?;
        Ok(SyncResult {
            verified: pending.verified_playlist_memberships,
            ownership: self.ownership(),
            completed: self.journal().is_none(),
        })
    }

    pub fn fail_once(&mut self, point: FailurePoint) {
        self.failure = Some(point);
    }

    fn install_failure(&mut self) -> Option<PlaylistFailurePoint> {
        match self.failure.take() {
            Some(FailurePoint::BeforeOwnershipPublish) => {
                Some(PlaylistFailurePoint::BeforeDeviceOwnershipRename)
            }
            Some(FailurePoint::ProjectionWrite) => {
                DeviceProjectionFs::fail_once_for_mount(
                    self.mount.clone(),
                    ProjectionFailurePoint::Write,
                );
                None
            }
            Some(FailurePoint::ProjectionRename) => {
                DeviceProjectionFs::fail_once_for_mount(
                    self.mount.clone(),
                    ProjectionFailurePoint::Rename,
                );
                None
            }
            Some(FailurePoint::ProjectionDelete) => {
                DeviceProjectionFs::fail_once_for_mount(
                    self.mount.clone(),
                    ProjectionFailurePoint::Delete,
                );
                None
            }
            Some(FailurePoint::ProjectionDeleteCleanup) => {
                DeviceProjectionFs::fail_once_for_mount(
                    self.mount.clone(),
                    ProjectionFailurePoint::DeleteCleanup,
                );
                None
            }
            None => None,
        }
    }

    fn desired(
        &self,
        playlists: &[TestPlaylist],
    ) -> Vec<classick::sync_transaction::DesiredPlaylist> {
        playlists
            .iter()
            .map(|playlist| {
                (
                    playlist.slug.clone(),
                    playlist.name.clone(),
                    playlist
                        .ordered_track_indexes
                        .iter()
                        .map(|index| self.source_paths[*index].clone())
                        .collect(),
                )
            })
            .collect()
    }

    pub fn write_foreign(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.mount.join("Playlists/Classick").join(name), bytes).unwrap();
    }

    pub fn write_raw_device_ownership(&self, bytes: &[u8]) {
        let path = classick::ipod::layout::managed_playlists_path(&self.mount);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    pub fn projection_path(&self, record: &RockboxProjectionRecord) -> PathBuf {
        self.mount
            .join("Playlists/Classick")
            .join(&record.relative_filename)
    }

    pub fn read_projection(&self, record: &RockboxProjectionRecord) -> Vec<u8> {
        std::fs::read(self.projection_path(record)).unwrap()
    }

    pub fn projection_exists(&self, record: &RockboxProjectionRecord) -> bool {
        self.projection_path(record).exists()
    }

    pub fn ownership(&self) -> ManagedPlaylistOwnership {
        self.ownership_store.load_device().unwrap()
    }

    pub fn journal(&self) -> Option<PendingSession> {
        self.journal_store
            .discover(&self.serial)
            .unwrap()
            .sessions
            .into_iter()
            .next()
    }

    pub fn foreign_hash(&self, name: &str) -> blake3::Hash {
        blake3::hash(&std::fs::read(self.mount.join("Playlists/Classick").join(name)).unwrap())
    }

    #[cfg(unix)]
    pub fn replace_managed_root_with_symlink(&self, outside: &Path) {
        let root = self.mount.join("Playlists/Classick");
        std::fs::remove_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside, root).unwrap();
    }

    #[cfg(unix)]
    pub fn swap_managed_root_before_projection_mutation(&self, outside: &Path) {
        DeviceProjectionFs::swap_managed_root_before_mutation_once(
            self.mount.clone(),
            outside.to_path_buf(),
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn rendered_lines(bytes: &[u8]) -> Vec<String> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

pub fn normalized_paths(membership: &VerifiedPlaylistMembership) -> Vec<String> {
    membership
        .ordered_ipod_paths
        .iter()
        .map(|path| {
            let normalized = path.replace('\\', "/");
            format!("/{}", normalized.trim_start_matches('/'))
        })
        .collect()
}

fn write_database_and_tracks(mount: &Path, sources: &[PathBuf]) -> Manifest {
    unsafe {
        let raw = ffi::itdb_new();
        let mount_c = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(raw, mount_c.as_ptr());
        let master = ffi::itdb_playlist_new(CString::new("iPod").unwrap().as_ptr(), 0);
        ffi::itdb_playlist_set_mpl(master);
        ffi::itdb_playlist_add(raw, master, -1);
        let mut entries = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            std::fs::write(source, format!("source-{index}")).unwrap();
            let file = format!("track{index}.m4a");
            std::fs::write(mount.join("iPod_Control/Music/F00").join(&file), b"audio").unwrap();
            let dbid = 100 + index as u64;
            let track = ffi::itdb_track_new();
            (*track).dbid = dbid;
            (*track).title =
                ffi::g_strdup(CString::new(format!("Track {index}")).unwrap().as_ptr());
            (*track).ipod_path = ffi::g_strdup(
                CString::new(format!(":iPod_Control:Music:F00:{file}"))
                    .unwrap()
                    .as_ptr(),
            );
            ffi::itdb_track_add(raw, track, -1);
            ffi::itdb_playlist_add_track(master, track, -1);
            entries.push(ManifestEntry {
                source_path: source.clone(),
                source_mtime: 0,
                source_size: 0,
                source_fingerprint: format!("fingerprint-{index}"),
                ipod_dbid: dbid,
                ipod_relpath: format!("iPod_Control/Music/F00/{file}"),
                source_known: true,
                audio_fingerprint: String::new(),
                encoder: "unknown".into(),
                encoder_version: String::new(),
                source_format: "flac".into(),
            });
        }
        let mut error = ptr::null_mut();
        assert_ne!(ffi::itdb_write(raw, &mut error), 0);
        ffi::itdb_free(raw);
        Manifest {
            version: 2,
            ipod_serial: Some(SERIAL.into()),
            last_source_root: Some(sources[0].parent().unwrap().to_path_buf()),
            tracks: entries,
        }
    }
}
