use classick::artwork_cache::ArtworkCache;
use classick::atomic_file::AtomicFileWriter;
use classick::manifest::Manifest;
use classick::manifest_store::ManifestStore;
use classick::pending_session::{
    PendingAlbum, PendingPhase, PendingSession, PendingSessionStore, StagedFile,
};
use classick::progress::Progress;
use classick::sync_transaction::{CheckpointCoordinator, PublishOptions, RollbackSnapshot};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const SERIAL: &str = "RawSerial";

struct Fixture {
    mount: PathBuf,
    host: PathBuf,
    store: ManifestStore,
    cache: ArtworkCache,
    manifest: Manifest,
}

fn fixture(label: &str) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "pending-recovery-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    let mount = root.join("mount");
    let host = root.join("host");
    std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(mount.join("iPod_Control/Artwork")).unwrap();
    std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
    std::fs::create_dir_all(host.join("source")).unwrap();
    write_valid_itunesdb(&mount);

    let store = ManifestStore::new(
        mount.clone(),
        SERIAL.into(),
        host.join("manifest.json"),
        host.join("legacy.json"),
        AtomicFileWriter::new(),
    );
    let mut manifest = Manifest::empty();
    manifest.version = 2;
    manifest.ipod_serial = Some(SERIAL.into());
    manifest.last_source_root = Some(host.join("source"));
    Fixture {
        mount,
        cache: ArtworkCache::new(host.join("artwork")),
        host,
        store,
        manifest,
    }
}

fn write_valid_itunesdb(mount: &Path) {
    unsafe {
        let db = classick::ffi::itdb_new();
        assert!(!db.is_null());
        let mount = CString::new(mount.to_str().unwrap()).unwrap();
        classick::ffi::itdb_set_mountpoint(db, mount.as_ptr());
        let title = CString::new("iPod").unwrap();
        let playlist = classick::ffi::itdb_playlist_new(title.as_ptr(), 0);
        classick::ffi::itdb_playlist_set_mpl(playlist);
        classick::ffi::itdb_playlist_add(db, playlist, -1);
        let mut error: *mut classick::ffi::GError = ptr::null_mut();
        assert_ne!(classick::ffi::itdb_write(db, &mut error), 0);
        classick::ffi::itdb_free(db);
    }
}

fn coordinator<'a>(
    mount: &'a Path,
    store: &'a ManifestStore,
    cache: &ArtworkCache,
) -> CheckpointCoordinator<'a> {
    CheckpointCoordinator {
        mount,
        serial: SERIAL,
        manifest_store: store,
        artwork_cache: cache.clone(),
    }
}

fn save_phase(fixture: &Fixture, id: u64, phase: PendingPhase) -> PendingSessionStore {
    let store = PendingSessionStore::new(&fixture.mount);
    let mut journal = PendingSession::new(id, SERIAL, Vec::new());
    journal.phase = phase;
    if phase >= PendingPhase::DatabaseVerified {
        let mut candidate = fixture.manifest.clone();
        candidate.version = 7;
        journal.candidate_manifest = Some(candidate);
    }
    store.save(&journal).unwrap();
    if phase >= PendingPhase::ReadyToPublish {
        RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(id)).unwrap();
    }
    store
}

#[test]
fn restart_abandons_staging_but_preserves_a_live_db_reference() {
    let mut fixture = fixture("staging");
    let db = classick::ipod::db::OwnedDb::open(&fixture.mount).unwrap();
    let handle = db
        .add_track_with_file(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/bare.m4a")
                .as_path(),
            &classick::ipod::db::Tags::default(),
            None,
        )
        .unwrap();
    db.write().unwrap();
    drop(db);
    let published = fixture.mount.join(handle.ipod_relpath.replace('\\', "/"));
    let pending = fixture
        .mount
        .join("iPod_Control/classick/pending/101.staged/track.m4a");
    let foreign = fixture.host.join("foreign.m4a");
    std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
    std::fs::write(&pending, b"pending").unwrap();
    std::fs::write(&foreign, b"foreign").unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(101, SERIAL, vec![album]);
    journal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending.clone(),
        Some(published.clone()),
        handle.dbid,
    ));
    let journal_store = PendingSessionStore::new(&fixture.mount);
    journal_store.save(&journal).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert!(published.exists());
    assert!(!pending.exists());
    assert!(foreign.exists());
    assert!(!journal_store.path(101).exists());
}

#[test]
fn restart_recovers_ready_to_publish_before_returning() {
    let mut fixture = fixture("ready");
    let store = save_phase(&fixture, 102, PendingPhase::ReadyToPublish);
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let results = coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(results.len(), 1);
    assert!(!store.path(102).exists());
    assert!(classick::device_state::portable_manifest_path(&fixture.mount).exists());
}

#[test]
fn restart_recovers_database_verified_and_assigns_candidate_after_device_publish() {
    let mut fixture = fixture("database-verified");
    let store = save_phase(&fixture, 103, PendingPhase::DatabaseVerified);
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(fixture.manifest.version, 7);
    assert!(!store.path(103).exists());
}

#[test]
fn restart_keeps_loaded_manifest_when_database_verified_publish_fails() {
    let mut fixture = fixture("database-verified-publish-failure");
    let store = save_phase(&fixture, 108, PendingPhase::DatabaseVerified);
    let device_manifest = classick::device_state::portable_manifest_path(&fixture.mount);
    std::fs::create_dir_all(&device_manifest).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("device manifest publication failed"));
    assert_eq!(fixture.manifest.version, 2);
    assert_eq!(store.load(108).unwrap().phase, PendingPhase::ReadyToPublish);
}

#[test]
fn restart_finishes_device_manifest_published_cleanup() {
    let mut fixture = fixture("device-manifest");
    let store = save_phase(&fixture, 104, PendingPhase::DeviceManifestPublished);
    fixture
        .store
        .publish_runtime(&store.load(104).unwrap().candidate_manifest.unwrap())
        .unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(fixture.manifest.version, 7);
    assert!(!store.path(104).exists());
    assert!(!store.snapshot_dir(104).exists());
}

#[test]
fn restart_removes_cleanup_complete_journal() {
    let mut fixture = fixture("cleanup-complete");
    let store = save_phase(&fixture, 105, PendingPhase::CleanupComplete);
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert!(!store.path(105).exists());
    assert!(!store.snapshot_dir(105).exists());
}

#[test]
fn restart_refuses_corrupt_and_foreign_serial_journals_without_mutation() {
    let mut fixture = fixture("rejected");
    let store = PendingSessionStore::new(&fixture.mount);
    let foreign = PendingSession::new(107, "RAWSERIAL", Vec::new());
    store.save(&foreign).unwrap();
    let corrupt = store.path(106);
    std::fs::write(&corrupt, b"{broken").unwrap();
    let foreign_bytes = std::fs::read(store.path(107)).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(&fixture.mount, &fixture.store, &fixture.cache)
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("unsafe pending-session journal"));
    assert_eq!(std::fs::read(&corrupt).unwrap(), b"{broken");
    assert_eq!(std::fs::read(store.path(107)).unwrap(), foreign_bytes);
}

#[test]
fn discovery_validates_identity_and_owned_paths_without_mutating_rejections() {
    let fixture = fixture("discovery");
    let store = PendingSessionStore::new(&fixture.mount);
    store
        .save(&PendingSession::new(201, SERIAL, Vec::new()))
        .unwrap();
    let corrupt = store.path(202);
    std::fs::write(&corrupt, b"{broken").unwrap();
    store
        .save(&PendingSession::new(203, "RAWSERIAL", Vec::new()))
        .unwrap();
    let mismatched_identity = store.path(204);
    std::fs::write(
        &mismatched_identity,
        serde_json::to_vec_pretty(&PendingSession::new(205, SERIAL, Vec::new())).unwrap(),
    )
    .unwrap();

    let outside = fixture.host.join("outside.m4a");
    std::fs::write(&outside, b"foreign audio").unwrap();
    let mut outside_album = PendingAlbum::new("album", 0);
    outside_album.staged_file_indices.push(0);
    let mut outside_paths = PendingSession::new(206, SERIAL, vec![outside_album]);
    outside_paths.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        outside.clone(),
        None,
        0,
    ));
    store.save(&outside_paths).unwrap();

    let pending_root = classick::device_state::pending_sessions_dir(&fixture.mount);
    let mut traversal_album = PendingAlbum::new("album", 0);
    traversal_album.staged_file_indices.push(0);
    let mut traversal = PendingSession::new(207, SERIAL, vec![traversal_album]);
    traversal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending_root.join("207.staged/../outside.m4a"),
        None,
        0,
    ));
    store.save(&traversal).unwrap();

    let discovery = store.discover(SERIAL).unwrap();

    assert_eq!(
        discovery
            .sessions
            .iter()
            .map(|session| session.session_id)
            .collect::<Vec<_>>(),
        vec![201]
    );
    assert_eq!(
        discovery
            .rejected
            .iter()
            .map(|rejected| rejected.path.clone())
            .collect::<Vec<_>>(),
        vec![
            corrupt.clone(),
            store.path(203),
            mismatched_identity.clone(),
            store.path(206),
            store.path(207),
        ]
    );
    for path in [
        corrupt,
        store.path(203),
        mismatched_identity,
        store.path(206),
        store.path(207),
        outside,
    ] {
        assert!(path.exists(), "discovery must preserve {}", path.display());
    }
}
