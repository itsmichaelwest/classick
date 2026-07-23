use classick::artwork_cache::ArtworkCache;
use classick::atomic_file::AtomicFileWriter;
use classick::manifest::{Manifest, ManifestEntry};
use classick::manifest_store::ManifestStore;
use classick::pending_session::{
    DeviceManifestPreimage, ManagedPlaylistRecordSnapshot, PendingAlbum, PendingPhase,
    PendingSession, PendingSessionStore, StagedFile,
};
use classick::progress::Progress;
use classick::sync_transaction::{CheckpointCoordinator, PublishOptions, RollbackSnapshot};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const SERIAL: &str = "000A27002138B0A8";

struct Fixture {
    mount: PathBuf,
    host: PathBuf,
    store: ManifestStore,
    cache: ArtworkCache,
    manifest: Manifest,
    mutation_session: classick::device_coordination::DeviceMutationSession,
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
    let mutation_session = classick::device_coordination::DeviceMutationSession::acquire(
        &mount,
        classick::device::DeviceId::parse(SERIAL).unwrap(),
    )
    .unwrap();
    Fixture {
        mount,
        cache: ArtworkCache::new(host.join("artwork")),
        host,
        store,
        manifest,
        mutation_session,
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
    mutation_session: &'a classick::device_coordination::DeviceMutationSession,
) -> CheckpointCoordinator<'a> {
    CheckpointCoordinator {
        mount,
        serial: SERIAL,
        mutation_session,
        manifest_store: store,
        artwork_cache: cache.clone(),
    }
}

fn save_phase(fixture: &Fixture, id: u64, phase: PendingPhase) -> PendingSessionStore {
    let store = PendingSessionStore::new(&fixture.mount);
    let mut journal = PendingSession::new(id, SERIAL, Vec::new());
    journal.phase = phase;
    let generation = fixture.mutation_session.current_generation().unwrap();
    journal.generation_before = Some(generation.clone());
    if phase >= PendingPhase::DatabaseVerified {
        journal.verified_generation = Some(generation);
        let mut candidate = fixture.manifest.clone();
        candidate.version = 7;
        journal.candidate_manifest = Some(candidate);
        journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
    }
    store.save(&journal).unwrap();
    if phase >= PendingPhase::ReadyToPublish {
        RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(id)).unwrap();
    }
    store
}

fn lost_ready_journal(
    fixture: &Fixture,
    id: u64,
    staged_count: usize,
) -> (PendingSessionStore, PendingSession, PathBuf, Vec<PathBuf>) {
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(id).with_file_name(format!("{id}.staged"));
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    let mut journal = PendingSession::new(id, SERIAL, Vec::new());
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    let mut pending = Vec::new();
    for index in 0..staged_count {
        let path = staged_dir.join(format!("{index}.m4a"));
        album.staged_file_indices.push(index);
        journal.staged_files.push(StagedFile::minimal(
            fixture.host.join(format!("source/{index}.flac")),
            path.clone(),
            None,
            0,
        ));
        pending.push(path);
    }
    journal.albums = vec![album];
    (store, journal, staged_dir, pending)
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

    coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
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

    let results = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(results.len(), 1);
    assert!(!store.path(102).exists());
    assert!(classick::device_state::portable_manifest_path(&fixture.mount).exists());
}

#[test]
fn restart_abandons_ready_to_publish_when_rollback_lost_all_staged_inputs() {
    let mut fixture = fixture("ready-lost-staged");
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(109).with_file_name("109.staged");
    let pending = staged_dir.join("track.m4a");
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(109, SERIAL, vec![album]);
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
    let source = fixture.host.join("source/track.flac");
    fixture.cache.record_no_art(&source).unwrap();
    let mut staged = StagedFile::minimal(source.clone(), pending.clone(), None, 0);
    staged.candidate_entry = Some(ManifestEntry {
        source_path: source,
        source_mtime: 1,
        source_size: 2,
        source_fingerprint: "source".into(),
        ipod_dbid: 0,
        ipod_relpath: String::new(),
        source_known: true,
        audio_fingerprint: "audio".into(),
        encoder: "afconvert".into(),
        encoder_version: String::new(),
        source_format: "flac".into(),
    });
    journal.staged_files.push(staged);
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(109)).unwrap();
    let generation_before = fixture.mutation_session.current_generation().unwrap();
    let database = classick::ipod::layout::itunes_db_path(&fixture.mount);
    let database_before = std::fs::read(&database).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let results = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].published_albums, 0);
    assert_eq!(results[0].published_tracks, 0);
    assert!(!pending.exists());
    assert!(!staged_dir.exists());
    assert!(!store.snapshot_dir(109).exists());
    assert!(!store.path(109).exists());
    assert_eq!(std::fs::read(database).unwrap(), database_before);
    assert_eq!(
        fixture.mutation_session.current_generation().unwrap(),
        generation_before
    );
    assert!(
        std::fs::read_dir(fixture.mount.join("iPod_Control/Music/F00"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn restart_preserves_lost_ready_to_publish_when_generation_changed() {
    let mut fixture = fixture("ready-lost-generation-changed");
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(110).with_file_name("110.staged");
    let pending = staged_dir.join("track.m4a");
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(110, SERIAL, vec![album]);
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    journal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending,
        None,
        0,
    ));
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(110)).unwrap();
    let journal_before = std::fs::read(store.path(110)).unwrap();
    let external = fixture.mount.join("iPod_Control/classick/profile.json");
    std::fs::write(&external, b"external generation").unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("external_generation_changed"));
    assert_eq!(std::fs::read(store.path(110)).unwrap(), journal_before);
    assert!(store.snapshot_dir(110).exists());
    assert!(staged_dir.exists());
    assert_eq!(std::fs::read(external).unwrap(), b"external generation");
}

#[test]
fn restart_preserves_lost_ready_to_publish_with_publication_evidence() {
    let mut fixture = fixture("ready-lost-publication-evidence");
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(111).with_file_name("111.staged");
    let pending = staged_dir.join("track.m4a");
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(111, SERIAL, vec![album]);
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    journal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending,
        Some(fixture.mount.join("iPod_Control/Music/F00/ambiguous.m4a")),
        41,
    ));
    journal.candidate_manifest = Some(fixture.manifest.clone());
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(111)).unwrap();
    let journal_before = std::fs::read(store.path(111)).unwrap();
    let generation_before = fixture.mutation_session.current_generation().unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("ambiguous publication evidence"));
    assert_eq!(std::fs::read(store.path(111)).unwrap(), journal_before);
    assert!(store.snapshot_dir(111).exists());
    assert!(staged_dir.exists());
    assert_eq!(
        fixture.mutation_session.current_generation().unwrap(),
        generation_before
    );
}

#[cfg(unix)]
#[test]
fn restart_preserves_lost_ready_to_publish_under_redirected_pending_root() {
    let mut fixture = fixture("ready-lost-redirected-root");
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(112).with_file_name("112.staged");
    let pending = staged_dir.join("track.m4a");
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(112, SERIAL, vec![album]);
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    journal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending,
        None,
        0,
    ));
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(112)).unwrap();

    let pending_root = classick::device_state::pending_sessions_dir(&fixture.mount);
    let redirected = fixture.host.join("redirected-pending");
    std::fs::rename(&pending_root, &redirected).unwrap();
    std::os::unix::fs::symlink(&redirected, &pending_root).unwrap();
    let journal_target = redirected.join("112.json");
    let snapshot_target = redirected.join("112.snapshot");
    let staged_target = redirected.join("112.staged");
    let journal_before = std::fs::read(&journal_target).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("unsafe pending-session"));
    assert_eq!(std::fs::read(journal_target).unwrap(), journal_before);
    assert!(snapshot_target.exists());
    assert!(staged_target.exists());
}

#[cfg(unix)]
#[test]
fn restart_preserves_lost_ready_to_publish_with_redirected_journal() {
    let mut fixture = fixture("ready-lost-redirected-journal");
    let (store, journal, staged_dir, _pending) = lost_ready_journal(&fixture, 117, 1);
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(117)).unwrap();
    let journal_path = store.path(117);
    let redirected = fixture.host.join("redirected-journal.json");
    std::fs::rename(&journal_path, &redirected).unwrap();
    std::os::unix::fs::symlink(&redirected, &journal_path).unwrap();
    let journal_before = std::fs::read(&redirected).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("pending-session journal"));
    assert!(std::fs::symlink_metadata(journal_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(redirected).unwrap(), journal_before);
    assert!(staged_dir.exists());
    assert!(store.snapshot_dir(117).exists());
}

#[test]
fn restart_preserves_lost_ready_to_publish_with_residual_snapshot_file() {
    let mut fixture = fixture("ready-lost-residual-snapshot");
    let store = PendingSessionStore::new(&fixture.mount);
    let staged_dir = store.path(113).with_file_name("113.staged");
    let pending = staged_dir.join("track.m4a");
    std::fs::create_dir_all(&staged_dir).unwrap();
    let mut album = PendingAlbum::new("album", 0);
    album.staged_file_indices.push(0);
    let mut journal = PendingSession::new(113, SERIAL, vec![album]);
    journal.phase = PendingPhase::ReadyToPublish;
    journal.generation_before = Some(fixture.mutation_session.current_generation().unwrap());
    journal.staged_files.push(StagedFile::minimal(
        fixture.host.join("source/track.flac"),
        pending,
        None,
        0,
    ));
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(113)).unwrap();
    let residual = store.snapshot_dir(113).join("residual.bin");
    std::fs::write(&residual, b"unindexed").unwrap();
    let journal_before = std::fs::read(store.path(113)).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("rollback snapshot"));
    assert_eq!(std::fs::read(store.path(113)).unwrap(), journal_before);
    assert!(staged_dir.exists());
    assert_eq!(std::fs::read(residual).unwrap(), b"unindexed");
}

#[test]
fn restart_preserves_partially_lost_ready_to_publish_inputs() {
    let mut fixture = fixture("ready-partially-lost");
    let (store, journal, staged_dir, pending) = lost_ready_journal(&fixture, 114, 2);
    std::fs::write(&pending[0], b"surviving staged input").unwrap();
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(114)).unwrap();
    let journal_before = std::fs::read(store.path(114)).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("some staged inputs"));
    assert_eq!(std::fs::read(store.path(114)).unwrap(), journal_before);
    assert_eq!(
        std::fs::read(&pending[0]).unwrap(),
        b"surviving staged input"
    );
    assert!(staged_dir.exists());
    assert!(store.snapshot_dir(114).exists());
}

#[cfg(unix)]
#[test]
fn restart_preserves_nonregular_or_redirected_pending_input() {
    for (offset, kind) in ["directory", "symlink"].into_iter().enumerate() {
        let mut fixture = fixture(&format!("ready-{kind}-pending-input"));
        let id = 127 + offset as u64;
        let (store, journal, staged_dir, pending) = lost_ready_journal(&fixture, id, 1);
        let outside = fixture.host.join(format!("{kind}-outside.m4a"));
        match kind {
            "directory" => std::fs::create_dir(&pending[0]).unwrap(),
            "symlink" => {
                std::fs::write(&outside, b"outside").unwrap();
                std::os::unix::fs::symlink(&outside, &pending[0]).unwrap();
            }
            _ => unreachable!(),
        }
        store.save(&journal).unwrap();
        RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(id)).unwrap();
        let journal_before = std::fs::read(store.path(id)).unwrap();
        let (progress, _decisions) = Progress::start(false, false).unwrap();

        let error = coordinator(
            &fixture.mount,
            &fixture.store,
            &fixture.cache,
            &fixture.mutation_session,
        )
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap_err();
        progress.finish(false).unwrap();

        assert!(
            format!("{error:#}").contains("not a regular file"),
            "{kind}: {error:#}"
        );
        assert_eq!(std::fs::read(store.path(id)).unwrap(), journal_before);
        assert!(staged_dir.exists());
        assert!(store.snapshot_dir(id).exists());
        if kind == "directory" {
            assert!(pending[0].is_dir());
        } else {
            assert!(std::fs::symlink_metadata(&pending[0])
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read(outside).unwrap(), b"outside");
        }
    }
}

#[test]
fn restart_preserves_lost_ready_to_publish_with_nonempty_staged_directory() {
    let mut fixture = fixture("ready-lost-nonempty-staged");
    let (store, journal, staged_dir, _pending) = lost_ready_journal(&fixture, 115, 1);
    let residual = staged_dir.join("residual.partial");
    std::fs::write(&residual, b"partial").unwrap();
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(115)).unwrap();
    let journal_before = std::fs::read(store.path(115)).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("staged transaction is not empty"));
    assert_eq!(std::fs::read(store.path(115)).unwrap(), journal_before);
    assert_eq!(std::fs::read(residual).unwrap(), b"partial");
    assert!(store.snapshot_dir(115).exists());
}

#[test]
fn restart_abandons_lost_ready_to_publish_without_snapshot() {
    let mut fixture = fixture("ready-lost-no-snapshot");
    let (store, journal, staged_dir, _pending) = lost_ready_journal(&fixture, 116, 1);
    store.save(&journal).unwrap();
    let generation_before = fixture.mutation_session.current_generation().unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let results = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(
        results,
        vec![classick::sync_transaction::CheckpointResult::default()]
    );
    assert!(!store.path(116).exists());
    assert!(!staged_dir.exists());
    assert!(!store.snapshot_dir(116).exists());
    assert_eq!(
        fixture.mutation_session.current_generation().unwrap(),
        generation_before
    );
}

#[test]
fn restart_preserves_each_independent_publication_evidence_field() {
    for (offset, field) in [
        "published_generation",
        "staged_dbid",
        "final_path",
        "candidate_manifest",
        "playlist_snapshot",
        "playlist_ownership",
        "rockbox_operation",
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = fixture(&format!("ready-lost-evidence-{field}"));
        let id = 120 + offset as u64;
        let (store, mut journal, staged_dir, _pending) = lost_ready_journal(&fixture, id, 1);
        match field {
            "published_generation" => {
                journal.published_generation = journal.generation_before.clone()
            }
            "staged_dbid" => journal.staged_files[0].dbid = 41,
            "final_path" => {
                journal.staged_files[0].final_ipod_path =
                    Some(fixture.mount.join("iPod_Control/Music/F00/candidate.m4a"))
            }
            "candidate_manifest" => journal.candidate_manifest = Some(fixture.manifest.clone()),
            "playlist_snapshot" => {
                journal.managed_playlist_record_snapshot =
                    Some(ManagedPlaylistRecordSnapshot { contents: None })
            }
            "playlist_ownership" => {
                journal.candidate_playlist_ownership = Some(
                    classick::ipod::playlist_ownership::ManagedPlaylistOwnership::empty_for_serial(
                        SERIAL,
                    ),
                )
            }
            "rockbox_operation" => {
                journal.pending_rockbox_ops.insert(
                    "mix".into(),
                    classick::pending_session::PendingRockboxOp {
                        previous: None,
                        desired: None,
                    },
                );
            }
            _ => unreachable!(),
        }
        store.save(&journal).unwrap();
        RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(id)).unwrap();
        let journal_before = std::fs::read(store.path(id)).unwrap();
        let (progress, _decisions) = Progress::start(false, false).unwrap();

        let error = coordinator(
            &fixture.mount,
            &fixture.store,
            &fixture.cache,
            &fixture.mutation_session,
        )
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap_err();
        progress.finish(false).unwrap();

        assert!(
            format!("{error:#}").contains("ambiguous publication evidence"),
            "{field}: {error:#}"
        );
        assert_eq!(
            std::fs::read(store.path(id)).unwrap(),
            journal_before,
            "{field}"
        );
        assert!(staged_dir.exists(), "{field}");
        assert!(store.snapshot_dir(id).exists(), "{field}");
    }
}

#[test]
fn restart_recovers_database_verified_and_assigns_candidate_after_device_publish() {
    let mut fixture = fixture("database-verified");
    let store = save_phase(&fixture, 103, PendingPhase::DatabaseVerified);
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
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
    let failing_manifest_store = ManifestStore::new(
        fixture.mount.clone(),
        SERIAL.into(),
        fixture.host.join("manifest.json"),
        fixture.host.join("legacy.json"),
        AtomicFileWriter::failing_before_replace(device_manifest),
    );
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &failing_manifest_store,
        &fixture.cache,
        &fixture.mutation_session,
    )
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
        .mutation_session
        .publish_verified(|| {
            fixture
                .store
                .publish_runtime(&store.load(104).unwrap().candidate_manifest.unwrap())?;
            Ok(())
        })
        .unwrap();
    let mut published = store.load(104).unwrap();
    published.verified_generation = Some(fixture.mutation_session.current_generation().unwrap());
    store.save(&published).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
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
    std::fs::create_dir_all(store.staged_dir(105)).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap();
    progress.finish(true).unwrap();

    assert!(!store.path(105).exists());
    assert!(!store.snapshot_dir(105).exists());
    assert!(!store.staged_dir(105).exists());
}

#[test]
fn rejected_journal_blocks_before_a_valid_session_is_cleaned() {
    let mut fixture = fixture("rejected-before-valid");
    let store = save_phase(&fixture, 108, PendingPhase::CleanupComplete);
    std::fs::create_dir_all(store.staged_dir(108)).unwrap();
    let corrupt = store.path(109);
    std::fs::write(&corrupt, b"{broken").unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("unsafe pending-session journal"));
    assert!(store.path(108).exists());
    assert!(store.staged_dir(108).exists());
    assert_eq!(std::fs::read(corrupt).unwrap(), b"{broken");
}

#[test]
fn cleanup_complete_preserves_an_unvalidated_snapshot_and_journal() {
    let mut fixture = fixture("cleanup-unvalidated-snapshot");
    let store = save_phase(&fixture, 110, PendingPhase::CleanupComplete);
    let snapshot = RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(110)).unwrap();
    drop(snapshot);
    let unexpected = store.snapshot_dir(110).join("unexpected.bin");
    std::fs::write(&unexpected, b"foreign").unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
    .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
    .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("rollback snapshot"));
    assert!(store.path(110).exists());
    assert_eq!(std::fs::read(unexpected).unwrap(), b"foreign");
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

    let error = coordinator(
        &fixture.mount,
        &fixture.store,
        &fixture.cache,
        &fixture.mutation_session,
    )
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
