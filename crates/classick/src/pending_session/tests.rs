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
fn discovery_ignores_macos_appledouble_sidecars() {
    let mount = tempdir("appledouble");
    let store = PendingSessionStore::new(&mount);
    let journal = PendingSession::new(41, "SERIAL", Vec::new());
    store.save(&journal).unwrap();
    std::fs::write(
        store.path(41).with_file_name("._41.json"),
        b"AppleDouble metadata",
    )
    .unwrap();

    let discovery = store.discover("SERIAL").unwrap();

    assert_eq!(discovery.sessions, vec![journal]);
    assert!(discovery.rejected.is_empty());
}

#[test]
fn appledouble_sidecars_are_not_sync_transaction_material() {
    let mount = tempdir("material");
    let pending = crate::device_state::pending_sessions_dir(&mount);
    std::fs::create_dir_all(pending.join("portable-config")).unwrap();
    std::fs::write(pending.join("._portable-config"), b"AppleDouble metadata").unwrap();

    assert!(!has_sync_transaction_material(&mount).unwrap());

    PendingSessionStore::new(&mount)
        .save(&PendingSession::new(41, "SERIAL", Vec::new()))
        .unwrap();

    assert!(has_sync_transaction_material(&mount).unwrap());
}

/// The gate that decides "is there pending work?" and the discovery that acts
/// on it must agree about what counts. Anything the gate counts but discovery
/// cannot load wedges every future sync on "pending sync transaction material
/// could not be recovered".
#[test]
fn only_loadable_journals_count_as_sync_transaction_material() {
    let mount = tempdir("gate");
    let pending = crate::device_state::pending_sessions_dir(&mount);
    std::fs::create_dir_all(&pending).unwrap();

    // Debris discovery skips: an AtomicFileWriter temp orphaned by a hard kill,
    // an AppleDouble sidecar, and a leftover staged directory.
    std::fs::write(pending.join("41.json.tmp-23375-278"), b"interrupted").unwrap();
    std::fs::write(pending.join("._41.json"), b"AppleDouble metadata").unwrap();
    std::fs::create_dir_all(pending.join("41.staged")).unwrap();
    std::fs::create_dir_all(pending.join("portable-config")).unwrap();

    assert!(!has_sync_transaction_material(&mount).unwrap());

    PendingSessionStore::new(&mount)
        .save(&PendingSession::new(41, "SERIAL", Vec::new()))
        .unwrap();

    assert!(has_sync_transaction_material(&mount).unwrap());
}

#[test]
fn removing_a_journal_clears_its_interrupted_write_temporaries() {
    let mount = tempdir("remove-temps");
    let store = PendingSessionStore::new(&mount);
    store
        .save(&PendingSession::new(41, "SERIAL", Vec::new()))
        .unwrap();
    let pending = crate::device_state::pending_sessions_dir(&mount);
    let stale = pending.join("41.json.tmp-23375-278");
    let other = pending.join("42.json.tmp-1-0");
    std::fs::write(&stale, b"interrupted").unwrap();
    std::fs::write(&other, b"another session's").unwrap();

    store.remove(41).unwrap();

    assert!(!store.path(41).exists());
    assert!(!stale.exists(), "this session's write temp must be cleared");
    assert!(other.exists(), "another session's files must be left alone");
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
    let pending_appledouble = mount.join("._pending.m4a");
    std::fs::write(&pending_appledouble, b"AppleDouble metadata").unwrap();
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
    assert!(!pending_appledouble.exists());
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
