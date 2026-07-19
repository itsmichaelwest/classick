use classick::daemon::library_mutations::{LibraryMutationService, MutationFailureCode};
use classick::library_index::{IndexedTrack, LibraryIndex};
use classick::playlist::{ManualPlaylist, Playlist, PlaylistStore};
use classick::selection::{SelectionMode, SelectionRule};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const REQ: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740";

fn root(label: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "library-mutation-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn index(root: &Path) -> LibraryIndex {
    let source = root.join("music");
    let mut index = LibraryIndex::empty(source.clone());
    index.scanned_at_unix_secs = Some(1);
    for (relative, artist, genre) in [
        ("Birdy/Fire Within/01.flac", "Birdy", "Pop"),
        ("Birdy/Fire Within/02.flac", "Birdy", "Pop"),
        ("Other/Album/01.flac", "Other", "Rock"),
    ] {
        index.files.insert(
            source.join(relative),
            IndexedTrack {
                mtime: 1,
                size: 1,
                artist: artist.into(),
                album_artist: String::new(),
                album: if artist == "Birdy" {
                    "Fire Within"
                } else {
                    "Album"
                }
                .into(),
                genre: genre.into(),
                title: relative.into(),
                duration_ms: 1,
                year: None,
            },
        );
    }
    index
}

fn configured(root: &Path, serial: &str) {
    let path = root.join("devices/registry.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!(r#"{{"version":1,"records":[{{"serial":"{serial}","model_label":"Classic","configured":true,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0}}]}}"#)).unwrap();
}

fn artist(name: &str) -> SelectionRule {
    SelectionRule::Artist { name: name.into() }
}
fn genre(name: &str) -> SelectionRule {
    SelectionRule::Genre { name: name.into() }
}

#[test]
fn replay_is_acknowledged_without_second_revision_bump() {
    let root = root("device-replay");
    configured(&root, "A");
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    let first = service
        .add_selection_to_device(REQ, "A", &[artist("Birdy")])
        .unwrap();
    let replay = service
        .add_selection_to_device(REQ, "A", &[artist("birdy")])
        .unwrap();
    assert_eq!(replay.selection_revision, first.selection_revision);
    assert!(!replay.selection_changed);
    assert_eq!(replay.selection.mode, SelectionMode::All);
    assert_eq!(service.device_selection("A").rules.len(), 0);
}

#[test]
fn playlist_replay_does_not_append_twice() {
    let root = root("playlist-replay");
    let store = PlaylistStore::open(root.join("playlists")).unwrap();
    store
        .save(&Playlist::Manual(ManualPlaylist {
            slug: "favorites".into(),
            name: "Favorites".into(),
            tracks: vec!["old.flac".into()],
            skipped_unsafe: 0,
        }))
        .unwrap();
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    let first = service
        .append_selection_to_playlist(REQ, "favorites", &[genre("Pop")])
        .unwrap();
    let replay = service
        .append_selection_to_playlist(REQ, "favorites", &[genre("pop")])
        .unwrap();
    assert_eq!(first.playlist_revision, replay.playlist_revision);
    assert_eq!(replay.appended_tracks, 2);
    assert_eq!(
        service.manual_playlist("favorites").unwrap().tracks.len(),
        3
    );
}

#[test]
fn same_request_with_different_fingerprint_is_rejected() {
    let root = root("collision");
    configured(&root, "A");
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    service
        .add_selection_to_device(REQ, "A", &[artist("Birdy")])
        .unwrap();
    let error = service
        .add_selection_to_device(REQ, "A", &[artist("Other")])
        .unwrap_err();
    assert_eq!(error.code, MutationFailureCode::RequestIdCollision);
}

#[test]
fn invalid_requests_and_unknown_targets_do_not_mutate() {
    let root = root("invalid");
    configured(&root, "A");
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    assert_eq!(
        service
            .add_selection_to_device("BAD", "A", &[artist("Birdy")])
            .unwrap_err()
            .code,
        MutationFailureCode::InvalidRequestId
    );
    assert_eq!(
        service
            .add_selection_to_device(REQ, "B", &[artist("Birdy")])
            .unwrap_err()
            .code,
        MutationFailureCode::UnknownDevice
    );
    assert_eq!(
        service
            .append_selection_to_playlist(REQ, "missing", &[artist("Birdy")])
            .unwrap_err()
            .code,
        MutationFailureCode::MissingPlaylist
    );
}

#[test]
fn recovery_finishes_payload_without_reapplying_append() {
    let root = root("recover");
    let store = PlaylistStore::open(root.join("playlists")).unwrap();
    store
        .save(&Playlist::Manual(ManualPlaylist {
            slug: "favorites".into(),
            name: "Favorites".into(),
            tracks: vec!["old.flac".into()],
            skipped_unsafe: 0,
        }))
        .unwrap();
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    service.fail_after_phase_once("payload_published");
    assert!(service
        .append_selection_to_playlist(REQ, "favorites", &[genre("Pop")])
        .is_err());
    drop(service);
    let mut recovered = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    recovered.recover_pending().unwrap();
    assert_eq!(
        recovered.manual_playlist("favorites").unwrap().tracks.len(),
        3
    );
    assert_eq!(recovered.playlist_revision("favorites"), 1);
    let replay = recovered
        .append_selection_to_playlist(REQ, "favorites", &[genre("pop")])
        .unwrap();
    assert_eq!(replay.appended_tracks, 2);
    assert_eq!(
        recovered.manual_playlist("favorites").unwrap().tracks.len(),
        3
    );
}

#[test]
fn recovery_rolls_forward_every_durable_phase_without_duplicate_append() {
    for (n, phase) in [
        "prepared",
        "payload_published",
        "revision_published",
        "ledger_published",
    ]
    .into_iter()
    .enumerate()
    {
        let root = root(phase);
        let store = PlaylistStore::open(root.join("playlists")).unwrap();
        store
            .save(&Playlist::Manual(ManualPlaylist {
                slug: "favorites".into(),
                name: "Favorites".into(),
                tracks: vec!["old.flac".into()],
                skipped_unsafe: 0,
            }))
            .unwrap();
        let request = format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f874{n}");
        let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
        service.fail_after_phase_once(phase);
        assert!(service
            .append_selection_to_playlist(&request, "favorites", &[genre("Pop")])
            .is_err());
        assert!(
            root.join("devices/library-mutations")
                .read_dir()
                .unwrap()
                .next()
                .is_some(),
            "failure must be journaled before returning"
        );
        let mut recovered = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
        recovered.recover_pending().unwrap();
        let replay = recovered
            .append_selection_to_playlist(&request, "favorites", &[genre("pop")])
            .unwrap();
        assert_eq!(replay.playlist_revision, 1);
        assert_eq!(
            recovered.manual_playlist("favorites").unwrap().tracks.len(),
            3
        );
        assert!(root
            .join("devices/library-mutations")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
    }
}

#[test]
fn no_matches_and_smart_playlist_rejection_leave_authority_unchanged() {
    let root = root("rejections");
    configured(&root, "A");
    let store = PlaylistStore::open(root.join("playlists")).unwrap();
    store
        .save(&Playlist::Smart(classick::playlist::SmartPlaylist {
            slug: "smart".into(),
            name: "Smart".into(),
            rules: classick::playlist_rules::SmartRules {
                version: classick::playlist_rules::RULES_VERSION,
                matching: classick::playlist_rules::Match::All,
                rules: vec![],
                limit: None,
                order: classick::playlist_rules::Order::Alpha,
                seed: 0,
            },
        }))
        .unwrap();
    let mut service = LibraryMutationService::open(root.clone(), index(&root)).unwrap();
    assert_eq!(
        service
            .add_selection_to_device(REQ, "A", &[artist("Nobody")])
            .unwrap_err()
            .code,
        MutationFailureCode::NoLibraryMatches
    );
    assert_eq!(
        service
            .append_selection_to_playlist(REQ, "smart", &[artist("Birdy")])
            .unwrap_err()
            .code,
        MutationFailureCode::NonManualPlaylist
    );
    assert_eq!(service.playlist_revision("smart"), 0);
}
