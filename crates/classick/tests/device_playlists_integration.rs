use classick::ffi;
use classick::ipod::db::{OwnedDb, PlaylistStructuralKind};
use classick::ipod::device_playlists::{
    reconcile_candidate, DesiredPlaylist, PlaylistDiagnostic, ReconcileStats,
};
use classick::ipod::playlist_ownership::{
    ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership, RockboxProjectionRecord,
};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

struct Fixture {
    db: OwnedDb,
    serial: String,
    track_ids: Vec<u64>,
    _mount: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let mount = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "playlist-reconcile-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        write_empty_db(&mount);
        let db = OwnedDb::open(&mount).unwrap();
        let track_ids = (0..3).map(|_| add_track(&db)).collect();
        Self {
            db,
            serial: "0xTEST-RAW-SERIAL".into(),
            track_ids,
            _mount: mount,
        }
    }

    fn desired(&self, slug: &str, name: &str, members: &[usize]) -> DesiredPlaylist {
        DesiredPlaylist {
            slug: slug.into(),
            display_name: name.into(),
            ordered_dbids: members.iter().map(|index| self.track_ids[*index]).collect(),
        }
    }

    fn ownership(&self, entries: &[(&str, u64)]) -> ManagedPlaylistOwnership {
        let playlists = entries
            .iter()
            .map(|(slug, id)| {
                (
                    (*slug).to_string(),
                    ManagedPlaylistEntry {
                        apple_playlist_id: *id,
                        expected_kind: ManagedPlaylistKind::Normal,
                        rockbox: None,
                    },
                )
            })
            .collect();
        ManagedPlaylistOwnership {
            schema_version: 1,
            device_serial: self.serial.clone(),
            playlists,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._mount);
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Normal,
    Podcast,
    Smart,
}

#[test]
fn playlist_kind_by_id_classifies_every_structural_kind() {
    let f = Fixture::new();
    let master = master_id(&f.db);
    let normal = add_playlist(&f.db, "Normal", Kind::Normal);
    let podcast = add_playlist(&f.db, "Podcasts", Kind::Podcast);
    let smart = add_playlist(&f.db, "Smart", Kind::Smart);

    assert_eq!(
        f.db.playlist_kind_by_id(master),
        Some(PlaylistStructuralKind::Master)
    );
    assert_eq!(
        f.db.playlist_kind_by_id(normal),
        Some(PlaylistStructuralKind::Normal)
    );
    assert_eq!(
        f.db.playlist_kind_by_id(podcast),
        Some(PlaylistStructuralKind::Podcast)
    );
    assert_eq!(
        f.db.playlist_kind_by_id(smart),
        Some(PlaylistStructuralKind::Smart)
    );
    assert_eq!(f.db.playlist_kind_by_id(u64::MAX), None);
}

#[test]
fn stale_managed_ids_never_grant_ownership_to_special_playlists() {
    for kind in [Kind::Podcast, Kind::Smart] {
        let f = Fixture::new();
        let suspect = add_playlist(&f.db, "Suspect", kind);
        let prior = f.ownership(&[("mix", suspect)]);
        let outcome = reconcile_candidate(&f.db, &[f.desired("mix", "Mix", &[0])], &prior).unwrap();
        let replacement = &outcome.candidate_ownership.playlists["mix"];
        assert_ne!(replacement.apple_playlist_id, suspect);
        assert_eq!(replacement.expected_kind, ManagedPlaylistKind::Normal);
        assert!(playlist_exists(&f.db, suspect));
        assert!(outcome.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            PlaylistDiagnostic::InvalidManagedAssociation { slug, playlist_id, .. }
                if slug == "mix" && *playlist_id == suspect
        )));
    }

    let f = Fixture::new();
    let master = master_id(&f.db);
    let outcome = reconcile_candidate(
        &f.db,
        &[f.desired("mix", "Mix", &[0])],
        &f.ownership(&[("mix", master)]),
    )
    .unwrap();
    assert_ne!(
        outcome.candidate_ownership.playlists["mix"].apple_playlist_id,
        master
    );
    assert!(playlist_exists(&f.db, master));
}

#[test]
fn unsubscription_removes_only_the_exact_recorded_normal_playlist() {
    let f = Fixture::new();
    let managed = add_playlist(&f.db, "Mix", Kind::Normal);
    let foreign_collision = add_playlist(&f.db, "Mix", Kind::Normal);
    let on_the_go = add_playlist(&f.db, "On-The-Go", Kind::Normal);
    let outcome = reconcile_candidate(&f.db, &[], &f.ownership(&[("mix", managed)])).unwrap();

    assert!(!playlist_exists(&f.db, managed));
    assert!(playlist_exists(&f.db, foreign_collision));
    assert!(playlist_exists(&f.db, on_the_go));
    assert!(outcome.candidate_ownership.playlists.is_empty());
    assert_eq!(outcome.stats.removed, 1);
}

#[test]
fn invalid_or_missing_unsubscribed_targets_are_preserved_and_forgotten() {
    for kind in [Kind::Podcast, Kind::Smart] {
        let f = Fixture::new();
        let suspect = add_playlist(&f.db, "Special", kind);
        let outcome = reconcile_candidate(&f.db, &[], &f.ownership(&[("old", suspect)])).unwrap();
        assert!(playlist_exists(&f.db, suspect));
        assert!(outcome.candidate_ownership.playlists.is_empty());
        assert_eq!(outcome.stats.removed, 0);
    }

    let f = Fixture::new();
    let outcome = reconcile_candidate(&f.db, &[], &f.ownership(&[("old", u64::MAX)])).unwrap();
    assert!(outcome.candidate_ownership.playlists.is_empty());
    assert!(outcome.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic,
        PlaylistDiagnostic::InvalidManagedAssociation {
            actual_kind: None,
            ..
        }
    )));
}

#[test]
fn corrupt_zero_id_authority_fails_closed_before_any_mutation() {
    let f = Fixture::new();
    let foreign = add_playlist(&f.db, "Foreign", Kind::Normal);
    let prior = f.ownership(&[("mix", 0)]);
    let error = reconcile_candidate(&f.db, &[f.desired("mix", "Mix", &[0])], &prior).unwrap_err();
    assert!(format!("{error:#}").contains("zero Apple playlist ID"));
    assert!(playlist_exists(&f.db, foreign));
    assert_eq!(playlist_name(&f.db, foreign), "Foreign");
}

#[test]
fn rename_by_slug_reuses_the_recorded_normal_id() {
    let f = Fixture::new();
    let id = add_playlist(&f.db, "Old Name", Kind::Normal);
    let outcome = reconcile_candidate(
        &f.db,
        &[f.desired("stable-slug", "New Name", &[2, 0])],
        &f.ownership(&[("stable-slug", id)]),
    )
    .unwrap();
    assert_eq!(
        outcome.candidate_ownership.playlists["stable-slug"].apple_playlist_id,
        id
    );
    assert_eq!(playlist_name(&f.db, id), "New Name");
    assert_eq!(
        playlist_members(&f.db, id),
        vec![f.track_ids[2], f.track_ids[0]]
    );
    assert_eq!(
        outcome.stats,
        ReconcileStats {
            created: 0,
            updated: 1,
            removed: 0
        }
    );
}

#[test]
fn duplicate_display_names_remain_distinct_by_slug() {
    let f = Fixture::new();
    let desired = [
        f.desired("gym", "Gym", &[0]),
        f.desired("gym-2", "Gym", &[1]),
    ];
    let outcome = reconcile_candidate(
        &f.db,
        &desired,
        &ManagedPlaylistOwnership::empty_for_serial(&f.serial),
    )
    .unwrap();
    let first = outcome.candidate_ownership.playlists["gym"].apple_playlist_id;
    let second = outcome.candidate_ownership.playlists["gym-2"].apple_playlist_id;
    assert_ne!(first, second);
    assert_eq!(playlist_members(&f.db, first), vec![f.track_ids[0]]);
    assert_eq!(playlist_members(&f.db, second), vec![f.track_ids[1]]);
}

#[test]
fn candidate_preserves_projection_and_exact_desired_order() {
    let f = Fixture::new();
    let id = add_playlist(&f.db, "Mix", Kind::Normal);
    let mut prior = f.ownership(&[("mix", id)]);
    let projection = RockboxProjectionRecord {
        relative_filename: "Playlists/mix.m3u8".into(),
        content_hash: "abc".into(),
    };
    prior.playlists.get_mut("mix").unwrap().rockbox = Some(projection.clone());
    let desired = f.desired("mix", "Mix", &[2, 0, 2]);
    let outcome = reconcile_candidate(&f.db, &[desired], &prior).unwrap();
    assert_eq!(
        outcome.candidate_ownership.playlists["mix"].rockbox,
        Some(projection)
    );
    assert_eq!(
        outcome.desired_memberships["mix"],
        vec![f.track_ids[2], f.track_ids[0], f.track_ids[2]]
    );
    assert_eq!(
        playlist_members(&f.db, id),
        outcome.desired_memberships["mix"]
    );
}

#[test]
fn mutation_errors_surface_without_a_publishable_candidate() {
    let f = Fixture::new();
    let error = reconcile_candidate(
        &f.db,
        &[f.desired("bad", "bad\0name", &[0])],
        &ManagedPlaylistOwnership::empty_for_serial(&f.serial),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("interior NUL"));
}

fn write_empty_db(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
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

fn add_track(db: &OwnedDb) -> u64 {
    unsafe {
        let track = ffi::itdb_track_new();
        ffi::itdb_track_add(db.as_ptr(), track, -1);
        ffi::itdb_playlist_add_track(ffi::itdb_playlist_mpl(db.as_ptr()), track, -1);
        (*track).dbid as u64
    }
}

fn add_playlist(db: &OwnedDb, name: &str, kind: Kind) -> u64 {
    unsafe {
        let name = CString::new(name).unwrap();
        let playlist = ffi::itdb_playlist_new(name.as_ptr(), 0);
        match kind {
            Kind::Normal => {}
            Kind::Podcast => ffi::itdb_playlist_set_podcasts(playlist),
            Kind::Smart => (*playlist).is_spl = 1,
        }
        ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
        (*playlist).id as u64
    }
}

fn master_id(db: &OwnedDb) -> u64 {
    unsafe { (*ffi::itdb_playlist_mpl(db.as_ptr())).id as u64 }
}

fn playlist_exists(db: &OwnedDb, id: u64) -> bool {
    unsafe { !ffi::itdb_playlist_by_id(db.as_ptr(), id).is_null() }
}

fn playlist_name(db: &OwnedDb, id: u64) -> String {
    unsafe {
        let playlist = ffi::itdb_playlist_by_id(db.as_ptr(), id);
        CStr::from_ptr((*playlist).name)
            .to_string_lossy()
            .into_owned()
    }
}

fn playlist_members(db: &OwnedDb, id: u64) -> Vec<u64> {
    unsafe {
        let playlist = ffi::itdb_playlist_by_id(db.as_ptr(), id);
        let mut members = Vec::new();
        let mut node = (*playlist).members;
        while !node.is_null() {
            members.push((*((*node).data as *mut ffi::Itdb_Track)).dbid as u64);
            node = (*node).next;
        }
        members
    }
}
