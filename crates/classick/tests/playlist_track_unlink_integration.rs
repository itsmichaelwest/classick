use classick::ffi;
use classick::ipod::db::{OwnedDb, TrackFileDisposition};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

#[path = "support/full_integrity_fixture.rs"]
mod full_integrity_fixture;
use full_integrity_fixture::FullIntegrityFixture;

struct UnlinkFixture {
    mount: PathBuf,
    db: OwnedDb,
    doomed_dbid: u64,
    retained_dbid: u64,
    doomed_file: PathBuf,
}

impl UnlinkFixture {
    fn with_shared_track() -> Self {
        let mount = fake_mount();
        write_empty_db(&mount);
        let db = OwnedDb::open(&mount).unwrap();
        let doomed_file = mount.join("iPod_Control/Music/F00/doomed.m4a");
        let retained_file = mount.join("iPod_Control/Music/F00/retained.m4a");
        std::fs::write(&doomed_file, b"doomed audio").unwrap();
        std::fs::write(&retained_file, b"retained audio").unwrap();

        let doomed = add_track(&db, 101, "Doomed", ":iPod_Control:Music:F00:doomed.m4a");
        let retained = add_track(&db, 202, "Retained", ":iPod_Control:Music:F00:retained.m4a");
        unsafe {
            let master = ffi::itdb_playlist_mpl(db.as_ptr());
            ffi::itdb_playlist_add_track(master, doomed, -1);
            ffi::itdb_playlist_add_track(master, retained, -1);
            for (name, smart) in [("Foreign", 0), ("Foreign Smart", 1), ("Managed", 0)] {
                let name = CString::new(name).unwrap();
                let playlist = ffi::itdb_playlist_new(name.as_ptr(), smart);
                assert!(!playlist.is_null());
                ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
                ffi::itdb_playlist_add_track(playlist, doomed, -1);
                ffi::itdb_playlist_add_track(playlist, retained, -1);
            }
        }
        Self {
            mount,
            db,
            doomed_dbid: 101,
            retained_dbid: 202,
            doomed_file,
        }
    }

    fn memberships_for(&self, dbid: u64) -> Vec<String> {
        memberships_for(&self.db, dbid)
    }
}

#[test]
fn delete_unlinks_every_playlist_before_free_and_reparses_cleanly() {
    let fixture = UnlinkFixture::with_shared_track();
    assert_eq!(
        fixture.memberships_for(fixture.doomed_dbid),
        vec!["iPod", "Foreign", "Foreign Smart", "Managed"]
    );
    assert!(fixture
        .db
        .remove_track(fixture.doomed_dbid, TrackFileDisposition::DeleteAfterCommit,)
        .unwrap());
    assert!(!fixture.doomed_file.exists());
    fixture.db.write().unwrap();
    drop(fixture.db);

    let reopened = OwnedDb::open(&fixture.mount).unwrap();
    assert!(!reopened
        .list_tracks_for_rebuild()
        .iter()
        .any(|track| track.dbid == fixture.doomed_dbid));
    for name in ["Foreign", "Foreign Smart", "Managed"] {
        assert_eq!(
            playlist_dbids(&reopened, name),
            vec![fixture.retained_dbid],
            "{name} retained a freed track"
        );
    }
}

#[test]
fn keep_unlinks_every_membership_but_preserves_audio() {
    let fixture = UnlinkFixture::with_shared_track();
    assert!(fixture
        .db
        .remove_track(fixture.doomed_dbid, TrackFileDisposition::Keep)
        .unwrap());
    assert!(fixture.doomed_file.exists());
    fixture.db.write().unwrap();
    drop(fixture.db);

    let reopened = OwnedDb::open(&fixture.mount).unwrap();
    assert!(memberships_for(&reopened, fixture.doomed_dbid).is_empty());
    for name in ["Foreign", "Foreign Smart", "Managed"] {
        assert_eq!(playlist_dbids(&reopened, name), vec![fixture.retained_dbid]);
    }
}

#[test]
fn coordinated_publication_preserves_every_non_owned_non_exact_playlist() {
    let mut fixture = FullIntegrityFixture::new();
    let preserved_before = fixture.foreign_payloads();
    let exact_before = fixture.exact_profile_payload();

    let publication = fixture.stage_delete_and_playlist_update();
    let expected_ownership = publication.candidate_ownership.clone();
    fixture.publish(publication).unwrap();
    let reopened = fixture.reopen();

    assert_eq!(
        fixture.foreign_payloads_from(&reopened),
        preserved_before.without_deleted_track(fixture.doomed_dbid())
    );
    assert_eq!(fixture.exact_profile_count(&reopened), 1);
    assert_eq!(fixture.exact_profile_payload_from(&reopened), exact_before);
    assert_eq!(
        fixture.verified_managed_order(&reopened, "mix"),
        fixture.desired_mix_dbids()
    );
    assert_eq!(fixture.device_ownership(), expected_ownership);
    assert_eq!(
        expected_ownership.playlists["mix"].expected_kind,
        classick::ipod::playlist_ownership::ManagedPlaylistKind::Normal
    );
}

fn fake_mount() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mount = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!("playlist-unlink-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&mount);
    std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
    mount
}

fn write_empty_db(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
        assert!(!db.is_null());
        let mount_c = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(db, mount_c.as_ptr());
        let name = CString::new("iPod").unwrap();
        let master = ffi::itdb_playlist_new(name.as_ptr(), 0);
        ffi::itdb_playlist_set_mpl(master);
        ffi::itdb_playlist_add(db, master, -1);
        let mut error = ptr::null_mut();
        assert_ne!(ffi::itdb_write(db, &mut error), 0);
        ffi::itdb_free(db);
    }
}

fn add_track(db: &OwnedDb, dbid: u64, title: &str, ipod_path: &str) -> *mut ffi::Itdb_Track {
    unsafe {
        let track = ffi::itdb_track_new();
        assert!(!track.is_null());
        (*track).dbid = dbid;
        let title = CString::new(title).unwrap();
        (*track).title = ffi::g_strdup(title.as_ptr());
        let path = CString::new(ipod_path).unwrap();
        (*track).ipod_path = ffi::g_strdup(path.as_ptr());
        ffi::itdb_track_add(db.as_ptr(), track, -1);
        track
    }
}

fn memberships_for(db: &OwnedDb, dbid: u64) -> Vec<String> {
    let mut memberships = Vec::new();
    unsafe {
        let track = find_track(db, dbid);
        if track.is_null() {
            return memberships;
        }
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() && ffi::itdb_playlist_contains_track(playlist, track) != 0 {
                memberships.push(
                    CStr::from_ptr((*playlist).name)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            node = (*node).next;
        }
    }
    memberships
}

fn playlist_dbids(db: &OwnedDb, name: &str) -> Vec<u64> {
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() && CStr::from_ptr((*playlist).name).to_string_lossy() == name {
                let mut members = Vec::new();
                let mut member = (*playlist).members;
                while !member.is_null() {
                    let track = (*member).data as *mut ffi::Itdb_Track;
                    if !track.is_null() {
                        members.push((*track).dbid);
                    }
                    member = (*member).next;
                }
                return members;
            }
            node = (*node).next;
        }
    }
    panic!("playlist {name:?} not found")
}

unsafe fn find_track(db: &OwnedDb, dbid: u64) -> *mut ffi::Itdb_Track {
    let mut node = (*db.as_ptr()).tracks;
    while !node.is_null() {
        let track = (*node).data as *mut ffi::Itdb_Track;
        if !track.is_null() && (*track).dbid == dbid {
            return track;
        }
        node = (*node).next;
    }
    ptr::null_mut()
}
