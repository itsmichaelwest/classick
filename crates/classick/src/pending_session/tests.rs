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
