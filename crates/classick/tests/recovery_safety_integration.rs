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
    DeviceManifestPreimage, PendingPhase, PendingRockboxOp, PendingSession, PendingSessionStore,
};
use classick::progress::Progress;
use classick::sync_transaction::{
    CheckpointCoordinator, PlaylistFailurePoint, PublishOptions, RollbackSnapshot,
};
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
    journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
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
                rockbox_compat: false,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("projection publisher"));
    assert_eq!(store.load(1).unwrap().pending_rockbox_ops, expected_ops);
    assert_eq!(std::fs::read(store.path(1)).unwrap(), before);
}

#[test]
fn database_verified_mismatch_blocks_reconcile_on_every_restart() {
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
    journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
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
    journal.verified_playlist_memberships = vec![VerifiedPlaylistMembership {
        slug: "mix".into(),
        apple_playlist_id: original_candidate_id,
        ordered_dbids: Vec::new(),
        ordered_ipod_paths: Vec::new(),
    }];
    let expected_candidate = journal.candidate_manifest.clone();
    let expected_ownership = journal.candidate_playlist_ownership.clone();
    let expected_desired = journal.desired_playlist_memberships.clone();
    let expected_verified = journal.verified_playlist_memberships.clone();
    store.save(&journal).unwrap();
    let database = classick::ipod::layout::itunes_db_path(&fixture.mount);
    let restored_database = std::fs::read(&database).unwrap();
    let restored_playlist_ids = classick::ipod::playlist_audit::snapshot_playlists(
        &classick::ipod::db::OwnedDb::open(&fixture.mount).unwrap(),
    )
    .into_iter()
    .map(|playlist| playlist.id)
    .collect::<Vec<_>>();
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
                rockbox_compat: false,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("database verification failed"));
    let restarted_store = PendingSessionStore::new(&fixture.mount);
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let restarted_coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };
    let second = restarted_coordinator.recover_pending_with_options(
        &mut fixture.manifest,
        &progress,
        PublishOptions {
            desired_playlists: Some(&desired),
            playlist_state_root: Some(&state_root),
            device_identity: None,
            playlist_failure_point: None,
            rockbox_compat: false,
        },
    );
    progress.finish(second.is_ok()).unwrap();

    assert!(
        second.is_err(),
        "blocked recovery must fail on every restart"
    );
    let retained = restarted_store.load(2).unwrap();
    assert_eq!(
        serde_json::to_value(retained.phase).unwrap(),
        serde_json::json!("rollback_complete")
    );
    assert_eq!(
        retained
            .candidate_playlist_ownership
            .as_ref()
            .unwrap()
            .playlists["mix"]
            .apple_playlist_id,
        original_candidate_id
    );
    assert_eq!(retained.candidate_manifest, expected_candidate);
    assert_eq!(retained.candidate_playlist_ownership, expected_ownership);
    assert_eq!(retained.desired_playlist_memberships, expected_desired);
    assert_eq!(retained.verified_playlist_memberships, expected_verified);
    assert_eq!(std::fs::read(database).unwrap(), restored_database);
    let reopened = classick::ipod::db::OwnedDb::open(&fixture.mount).unwrap();
    let playlist_ids = classick::ipod::playlist_audit::snapshot_playlists(&reopened)
        .into_iter()
        .map(|playlist| playlist.id)
        .collect::<Vec<_>>();
    assert_eq!(playlist_ids, restored_playlist_ids);
    let replacement_count = classick::ipod::db::list_playlists(&reopened)
        .into_iter()
        .filter(|(_, is_master)| !is_master)
        .count();
    assert_eq!(replacement_count, 0, "neither recovery may call reconcile");
    assert!(
        !classick::ipod::playlist_audit::snapshot_playlists(&reopened)
            .iter()
            .any(|playlist| playlist.id == original_candidate_id)
    );
}

#[test]
fn published_mismatch_restores_exact_preexisting_device_manifest_on_every_restart() {
    assert_published_mismatch_restores_manifest("manifest-present", Some(b"exact old manifest\n"));
}

#[test]
fn published_mismatch_restores_originally_absent_device_manifest_on_every_restart() {
    assert_published_mismatch_restores_manifest("manifest-absent", None);
}

fn assert_published_mismatch_restores_manifest(label: &str, previous: Option<&[u8]>) {
    let mut fixture = fixture(label);
    let manifest_path = classick::device_state::portable_manifest_path(&fixture.mount);
    if let Some(bytes) = previous {
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(&manifest_path, bytes).unwrap();
    }
    let original_db =
        std::fs::read(classick::ipod::layout::itunes_db_path(&fixture.mount)).unwrap();
    let desired = vec![("mix".to_string(), "Mix".to_string(), Vec::new())];
    let state_root = fixture.host.join("state");
    let store = PendingSessionStore::new(&fixture.mount);
    let mut journal = PendingSession::new(3, SERIAL, Vec::new());
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };
    let (progress, _decisions) = Progress::start(false, false).unwrap();

    coordinator
        .publish_with_options(
            &mut journal,
            &mut fixture.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&state_root),
                device_identity: None,
                playlist_failure_point: Some(PlaylistFailurePoint::BeforeProjectionPlanPersist),
                rockbox_compat: false,
            },
        )
        .unwrap_err();
    progress.finish(false).unwrap();
    let published = store.load(3).unwrap();
    assert_eq!(published.phase, PendingPhase::DeviceManifestPublished);
    assert_eq!(
        published
            .device_manifest_preimage
            .as_ref()
            .unwrap()
            .contents
            .as_deref(),
        previous
    );
    assert!(manifest_path.exists());
    if let Some(bytes) = previous {
        assert_ne!(std::fs::read(&manifest_path).unwrap(), bytes);
    }
    let candidate_id = published
        .candidate_playlist_ownership
        .as_ref()
        .unwrap()
        .playlists["mix"]
        .apple_playlist_id;
    let db = classick::ipod::db::OwnedDb::open(&fixture.mount).unwrap();
    classick::ipod::db::remove_playlist_by_id(&db, candidate_id).unwrap();
    db.write().unwrap();
    drop(db);

    for _restart in 0..2 {
        let (progress, _decisions) = Progress::start(false, false).unwrap();
        let result = coordinator.recover_pending_with_options(
            &mut fixture.manifest,
            &progress,
            PublishOptions {
                desired_playlists: Some(&desired),
                playlist_state_root: Some(&state_root),
                device_identity: None,
                playlist_failure_point: None,
                rockbox_compat: false,
            },
        );
        progress.finish(result.is_ok()).unwrap();
        assert!(result.is_err());
        let retained = store.load(3).unwrap();
        assert_eq!(retained.phase, PendingPhase::RollbackComplete);
        assert_eq!(
            retained
                .candidate_playlist_ownership
                .as_ref()
                .unwrap()
                .playlists["mix"]
                .apple_playlist_id,
            candidate_id
        );
        assert_eq!(
            std::fs::read(classick::ipod::layout::itunes_db_path(&fixture.mount)).unwrap(),
            original_db
        );
        match previous {
            Some(bytes) => assert_eq!(std::fs::read(&manifest_path).unwrap(), bytes),
            None => assert!(!manifest_path.exists()),
        }
    }
}

#[test]
fn database_verified_recovery_is_database_byte_stable_when_verification_succeeds() {
    let mut fixture = fixture("verified-byte-stable");
    let store = PendingSessionStore::new(&fixture.mount);
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(4)).unwrap();
    let mut journal = PendingSession::new(4, SERIAL, Vec::new());
    journal.phase = PendingPhase::DatabaseVerified;
    journal.candidate_manifest = Some(fixture.manifest.clone());
    journal.device_manifest_preimage = Some(DeviceManifestPreimage { contents: None });
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

#[test]
fn legacy_verified_journal_without_manifest_preimage_fails_closed_unchanged() {
    let mut fixture = fixture("legacy-missing-preimage");
    let store = PendingSessionStore::new(&fixture.mount);
    RollbackSnapshot::create(&fixture.mount, &store.snapshot_dir(5)).unwrap();
    let mut journal = PendingSession::new(5, SERIAL, Vec::new());
    journal.phase = PendingPhase::DatabaseVerified;
    journal.candidate_manifest = Some(fixture.manifest.clone());
    store.save(&journal).unwrap();
    let journal_before = std::fs::read(store.path(5)).unwrap();
    let database = classick::ipod::layout::itunes_db_path(&fixture.mount);
    let database_before = std::fs::read(&database).unwrap();
    let (progress, _decisions) = Progress::start(false, false).unwrap();
    let coordinator = CheckpointCoordinator {
        mount: &fixture.mount,
        serial: SERIAL,
        manifest_store: &fixture.manifest_store,
        artwork_cache: fixture.cache.clone(),
    };

    let error = coordinator
        .recover_pending_with_options(&mut fixture.manifest, &progress, PublishOptions::default())
        .unwrap_err();
    progress.finish(false).unwrap();

    assert!(format!("{error:#}").contains("has no safe preimage"));
    assert_eq!(std::fs::read(store.path(5)).unwrap(), journal_before);
    assert_eq!(std::fs::read(database).unwrap(), database_before);
    assert!(store.load(5).unwrap().device_manifest_preimage.is_none());
}
