use classick::artwork_cache::ArtworkCache;
use classick::atomic_file::AtomicFileWriter;
use classick::ipod::device_playlists::VerifiedPlaylistMembership;
use classick::ipod::playlist_ownership::{
    ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership,
    MANAGED_PLAYLIST_OWNERSHIP_VERSION,
};
use classick::manifest::{Manifest, ManifestEntry};
use classick::manifest_store::ManifestStore;
use classick::pending_session::{
    PendingPhase, PendingRockboxOp, PendingSession, PendingSessionStore,
};
use classick::progress::Progress;
use classick::sync_transaction::{CheckpointCoordinator, PublishOptions, RollbackSnapshot};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const SERIAL: &str = "SERIAL";

struct Fixture {
    mount: PathBuf,
    host: PathBuf,
    manifest_store: ManifestStore,
    cache: ArtworkCache,
    manifest: Manifest,
}

fn fixture(label: &str) -> Fixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "recovery-safety-{label}-{}-{}",
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

    let manifest_store = ManifestStore::new(
        mount.clone(),
        SERIAL.into(),
        host.join("manifest.json"),
        host.join("legacy.json"),
        AtomicFileWriter::new(),
    );
    let mut manifest = Manifest::empty();
    manifest.ipod_serial = Some(SERIAL.into());
    manifest.last_source_root = Some(host.join("source"));
    Fixture {
        mount,
        cache: ArtworkCache::new(host.join("artwork")),
        host,
        manifest_store,
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

fn empty_ownership() -> ManagedPlaylistOwnership {
    ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: SERIAL.into(),
        playlists: BTreeMap::new(),
    }
}

#[test]
fn device_manifest_published_future_ops_remain_byte_exact_when_recovery_rejects_them() {
    let mut fixture = fixture("future-ops");
    let store = PendingSessionStore::new(&fixture.mount);
    let mut journal = PendingSession::new(1, SERIAL, Vec::new());
    journal.phase = PendingPhase::DeviceManifestPublished;
    journal.candidate_manifest = Some(fixture.manifest.clone());
    journal.candidate_playlist_ownership = Some(empty_ownership());
    journal.pending_rockbox_ops.insert(
        "future".into(),
        PendingRockboxOp {
            previous: None,
            desired: None,
        },
    );
    store.save(&journal).unwrap();
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(1)).unwrap();
    let before = std::fs::read(store.path(1)).unwrap();
    let expected_ops = journal.pending_rockbox_ops.clone();
    let state_root = fixture.host.join("state");
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };

    let error = coordinator
        .recover_pending_with_options(
            &mut fixture.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&[]),
                playlist_state_root: Some(&state_root),
                device_identity: None,
                playlist_failure_point: None,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("projection publisher"));
    assert_eq!(store.load(1).unwrap().pending_rockbox_ops, expected_ops);
    assert_eq!(std::fs::read(store.path(1)).unwrap(), before);
}

#[test]
fn database_verified_mismatch_restores_without_reconciling_replacement_ids() {
    let mut fixture = fixture("verified-mismatch");
    let store = PendingSessionStore::new(&fixture.mount);
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(2)).unwrap();
    let mut candidate = fixture.manifest.clone();
    candidate.tracks.push(ManifestEntry {
        source_path: PathBuf::new(),
        source_mtime: 0,
        source_size: 0,
        source_fingerprint: String::new(),
        ipod_dbid: 999,
        ipod_relpath: "iPod_Control\\Music\\F00\\missing.m4a".into(),
        source_known: false,
        audio_fingerprint: String::new(),
        encoder: "unknown".into(),
        encoder_version: String::new(),
        source_format: "flac".into(),
    });
    let original_candidate_id = 41;
    let mut journal = PendingSession::new(2, SERIAL, Vec::new());
    journal.phase = PendingPhase::DatabaseVerified;
    journal.candidate_manifest = Some(candidate);
    journal.candidate_playlist_ownership = Some(ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: SERIAL.into(),
        playlists: BTreeMap::from([(
            "mix".into(),
            ManagedPlaylistEntry {
                apple_playlist_id: original_candidate_id,
                expected_kind: ManagedPlaylistKind::Normal,
                rockbox: None,
            },
        )]),
    });
    journal
        .desired_playlist_memberships
        .insert("mix".into(), Vec::new());
    store.save(&journal).unwrap();
    let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
    let state_root = fixture.host.join("state");
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };

    let error = coordinator
        .recover_pending_with_options(
            &mut fixture.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&state_root),
                device_identity: None,
                playlist_failure_point: None,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("database verification failed"));
    assert_eq!(store.load(2).unwrap().phase, PendingPhase::ReadyToPublish);
    let reopened = classick::ipod::db::OwnedDb::open(&fixture.mount).unwrap();
    let replacement_count = classick::ipod::db::list_playlists(&reopened)
        .into_iter()
        .filter(|(_, is_master)| !is_master)
        .count();
    assert_eq!(replacement_count, 0, "recovery must not call reconcile");
    assert!(
        !classick::ipod::playlist_audit::snapshot_playlists(&reopened)
            .iter()
            .any(|playlist| playlist.id == original_candidate_id)
    );
}

#[test]
fn device_manifest_published_playlist_mismatch_restores_and_keeps_journal() {
    let mut fixture = fixture("published-mismatch");
    let store = PendingSessionStore::new(&fixture.mount);
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(3)).unwrap();
    let mut journal = PendingSession::new(3, SERIAL, Vec::new());
    journal.phase = PendingPhase::DeviceManifestPublished;
    journal.candidate_manifest = Some(fixture.manifest.clone());
    journal.candidate_playlist_ownership = Some(ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: SERIAL.into(),
        playlists: BTreeMap::from([(
            "mix".into(),
            ManagedPlaylistEntry {
                apple_playlist_id: 41,
                expected_kind: ManagedPlaylistKind::Normal,
                rockbox: None,
            },
        )]),
    });
    journal
        .desired_playlist_memberships
        .insert("mix".into(), Vec::new());
    journal.verified_playlist_memberships = vec![VerifiedPlaylistMembership {
        slug: "mix".into(),
        apple_playlist_id: 41,
        ordered_dbids: Vec::new(),
        ordered_ipod_paths: Vec::new(),
    }];
    store.save(&journal).unwrap();
    let original_db =
        std::fs::read(classick::ipod::layout::itunes_db_path(&fixture.mount)).unwrap();
    let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
    let state_root = fixture.host.join("state");
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };

    let error = coordinator
        .recover_pending_with_options(
            &mut fixture.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&state_root),
                device_identity: None,
                playlist_failure_point: None,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("database verification failed"));
    assert_eq!(store.load(3).unwrap().phase, PendingPhase::ReadyToPublish);
    assert_eq!(
        std::fs::read(classick::ipod::layout::itunes_db_path(&fixture.mount)).unwrap(),
        original_db
    );
}

#[test]
fn database_verified_recovery_is_database_byte_stable_when_verification_succeeds() {
    let mut fixture = fixture("verified-byte-stable");
    let store = PendingSessionStore::new(&fixture.mount);
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(4)).unwrap();
    let mut journal = PendingSession::new(4, SERIAL, Vec::new());
    journal.phase = PendingPhase::DatabaseVerified;
    journal.candidate_manifest = Some(fixture.manifest.clone());
    store.save(&journal).unwrap();
    let database = classick::ipod::layout::itunes_db_path(&fixture.mount);
    let before = std::fs::read(&database).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };

    coordinator
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap();
    progress.finish(true).unwrap();

    assert_eq!(std::fs::read(database).unwrap(), before);
}
