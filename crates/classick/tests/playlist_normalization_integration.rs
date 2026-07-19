use classick::ffi;
use classick::ipod::playlist_audit::{snapshot_playlists, PlaylistSnapshot};
use classick::ipod::playlist_normalize::normalize_firmware_playlists;
use classick::ipod::playlist_profile::{
    firmware_profile, match_firmware_profile, FirmwarePlaylistProfile, FirmwareProfileId,
};
use classick::ipod::OwnedDb;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

struct NormalizationFixture {
    mount: PathBuf,
    db: OwnedDb,
}

impl NormalizationFixture {
    fn new() -> Self {
        let mount = fake_mount();
        write_empty_db(&mount);
        let db = OwnedDb::open(&mount).unwrap();
        Self { mount, db }
    }

    fn add_exact(&self, name: &str, timestamp: i64, id: u64) -> u64 {
        unsafe {
            add_profile(&self.db, name, timestamp, id);
        }
        id
    }

    fn snapshots(&self) -> Vec<PlaylistSnapshot> {
        snapshot_playlists(&self.db)
    }

    fn exists(&self, id: u64) -> bool {
        self.snapshots().iter().any(|playlist| playlist.id == id)
    }
}

#[test]
fn zero_exact_instances_are_byte_stable_and_create_nothing() {
    let fixture = NormalizationFixture::new();
    unsafe {
        add_normal(&fixture.db, "Foreign", 11);
    }
    let before = fixture.snapshots();
    let report = normalize_firmware_playlists(&fixture.db).unwrap();
    assert!(report.kept.is_empty());
    assert!(report.removed.is_empty());
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn one_exact_instance_is_byte_stable() {
    let fixture = NormalizationFixture::new();
    let only = fixture.add_exact("Localized Videos", 100, 10);
    let before = fixture.snapshots();
    let report = normalize_firmware_playlists(&fixture.db).unwrap();
    assert_eq!(report.kept, vec![only]);
    assert!(report.removed.is_empty());
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn many_exact_instances_keep_newest_then_highest_id() {
    let fixture = NormalizationFixture::new();
    let old = fixture.add_exact("Alt", 100, 10);
    let tied_low = fixture.add_exact("Videos", 200, 20);
    let tied_high = fixture.add_exact("Vidéos", 200, 30);
    let report = normalize_firmware_playlists(&fixture.db).unwrap();
    assert_eq!(report.kept, vec![tied_high]);
    assert_eq!(report.removed, vec![old, tied_low]);
    assert!(fixture.exists(tied_high));
    assert!(!fixture.exists(old));
    assert!(!fixture.exists(tied_low));
}

#[test]
fn preservation_matrix_is_untouched() {
    let fixture = NormalizationFixture::new();
    unsafe {
        let master = ffi::itdb_playlist_mpl(fixture.db.as_ptr());
        (*master).id = 1;
        add_normal(&fixture.db, "Foreign normal", 2);
        let podcast = add_profile(&fixture.db, "Podcasts", 10, 3);
        (*podcast).podcastflag = 1;
        let on_the_go = add_profile(&fixture.db, "On-The-Go 1", 11, 4);
        (*on_the_go).is_spl = 0;
        add_empty_smart(&fixture.db, "Arbitrary empty smart", 5);
        let unknown_smart = add_profile(&fixture.db, "Unknown", 13, 6);
        (*unknown_smart).splrules.match_operator += 1;

        for (index, mutate) in semantic_mutations().into_iter().enumerate() {
            let playlist = add_profile(
                &fixture.db,
                &format!("Near {index}"),
                100 + index as i64,
                100 + index as u64,
            );
            mutate(playlist);
        }
    }
    let before = fixture.snapshots();
    let report = normalize_firmware_playlists(&fixture.db).unwrap();
    assert!(report.kept.is_empty());
    assert!(report.removed.is_empty());
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn normalization_persists_only_duplicate_removal() {
    let fixture = NormalizationFixture::new();
    let removed = fixture.add_exact("Videos", 100, 10);
    let kept = fixture.add_exact("Vídeos", 200, 20);
    normalize_firmware_playlists(&fixture.db).unwrap();
    fixture.db.write().unwrap();
    let mount = fixture.mount.clone();
    drop(fixture);
    let reopened = OwnedDb::open(&mount).unwrap();
    let ids: Vec<_> = snapshot_playlists(&reopened)
        .into_iter()
        .map(|playlist| playlist.id)
        .collect();
    assert!(ids.contains(&kept));
    assert!(!ids.contains(&removed));
}

#[test]
fn libgpod_write_reparse_maps_captured_encoding_to_registered_post_write_encoding() {
    let fixture = NormalizationFixture::new();
    let id = fixture.add_exact("Localized Videos", 100, 10);
    fixture.db.write().unwrap();
    let mount = fixture.mount.clone();
    drop(fixture);

    let reopened = OwnedDb::open(&mount).unwrap();
    let snapshot = snapshot_playlists(&reopened)
        .into_iter()
        .find(|playlist| playlist.id == id)
        .unwrap();
    assert_profile_encoding(&snapshot, &post_write_profile());
    assert_eq!(
        match_firmware_profile(&snapshot),
        Some(FirmwareProfileId::IpodClassicVideoKindV1)
    );
}

#[test]
fn normalization_keeps_one_across_two_coordinated_writes_and_preserves_foreign_near_match() {
    let fixture = NormalizationFixture::new();
    let removed = fixture.add_exact("Videos", 100, 10);
    let kept = fixture.add_exact("Vidéos", 200, 20);
    let foreign = unsafe {
        let playlist = add_profile(&fixture.db, "Near", 300, 30);
        (*playlist).splrules.match_operator += 1;
        (*playlist).id
    };

    let first = normalize_firmware_playlists(&fixture.db).unwrap();
    assert_eq!(first.kept, vec![kept]);
    assert_eq!(first.removed, vec![removed]);
    fixture.db.write().unwrap();
    let mount = fixture.mount.clone();
    drop(fixture);

    let reopened = OwnedDb::open(&mount).unwrap();
    assert_exact_and_foreign_counts(&reopened, kept, foreign);
    let second = normalize_firmware_playlists(&reopened).unwrap();
    assert_eq!(second.kept, vec![kept]);
    assert!(second.removed.is_empty());
    reopened.write().unwrap();
    drop(reopened);

    let reopened_again = OwnedDb::open(&mount).unwrap();
    assert_exact_and_foreign_counts(&reopened_again, kept, foreign);
}

#[test]
fn replace_library_database_write_normalizes_exact_firmware_duplicates() {
    let fixture = NormalizationFixture::new();
    let removed = fixture.add_exact("Videos", 100, 10);
    let kept = fixture.add_exact("Vidéos", 200, 20);

    classick::apply_loop::write_replace_library_database(&fixture.db).unwrap();
    let mount = fixture.mount.clone();
    drop(fixture);

    let reopened = OwnedDb::open(&mount).unwrap();
    assert!(reopened.track_count() == 0);
    let ids = snapshot_playlists(&reopened)
        .into_iter()
        .map(|playlist| playlist.id)
        .collect::<Vec<_>>();
    assert!(!ids.contains(&removed));
    assert!(ids.contains(&kept));
}

#[test]
fn rebuilt_artwork_database_write_normalizes_exact_firmware_duplicates() {
    let fixture = NormalizationFixture::new();
    let removed = fixture.add_exact("Videos", 100, 10);
    let kept = fixture.add_exact("Vidéos", 200, 20);

    classick::apply_loop::write_rebuilt_artwork_database(&fixture.db).unwrap();
    let mount = fixture.mount.clone();
    drop(fixture);

    let reopened = OwnedDb::open(&mount).unwrap();
    let ids = snapshot_playlists(&reopened)
        .into_iter()
        .map(|playlist| playlist.id)
        .collect::<Vec<_>>();
    assert!(!ids.contains(&removed));
    assert!(ids.contains(&kept));
}

fn post_write_profile() -> FirmwarePlaylistProfile {
    serde_json::from_str(include_str!(
        "fixtures/ipod-classic-video-kind-v1-libgpod-post-write.json"
    ))
    .unwrap()
}

fn assert_profile_encoding(snapshot: &PlaylistSnapshot, profile: &FirmwarePlaylistProfile) {
    assert_eq!(snapshot.is_master, profile.is_master);
    assert_eq!(snapshot.is_podcast, profile.is_podcast);
    assert_eq!(snapshot.is_smart, profile.is_smart);
    assert_eq!(snapshot.member_count, profile.member_count);
    assert_eq!(snapshot.preferences, profile.preferences);
    assert_eq!(snapshot.rules_header, profile.rules_header);
    assert_eq!(snapshot.rules, profile.rules);
}

fn assert_exact_and_foreign_counts(db: &OwnedDb, exact_id: u64, foreign_id: u64) {
    let snapshots = snapshot_playlists(db);
    let exact: Vec<_> = snapshots
        .iter()
        .filter(|playlist| match_firmware_profile(playlist).is_some())
        .collect();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].id, exact_id);
    let foreign = snapshots
        .iter()
        .find(|playlist| playlist.id == foreign_id)
        .unwrap();
    assert_eq!(match_firmware_profile(foreign), None);
}

type PlaylistMutation = unsafe fn(*mut ffi::Itdb_Playlist);

fn semantic_mutations() -> Vec<PlaylistMutation> {
    vec![
        |p| unsafe { (*p).is_spl = 0 },
        |p| unsafe { (*p).splpref.liveupdate ^= 1 },
        |p| unsafe { (*p).splpref.checkrules ^= 1 },
        |p| unsafe { (*p).splpref.checklimits ^= 1 },
        |p| unsafe { (*p).splpref.limittype += 1 },
        |p| unsafe { (*p).splpref.limitsort += 1 },
        |p| unsafe { (*p).splpref.limitvalue += 1 },
        |p| unsafe { (*p).splpref.matchcheckedonly ^= 1 },
        |p| unsafe { (*p).splpref.reserved_int1 += 1 },
        |p| unsafe { (*p).splpref.reserved_int2 += 1 },
        |p| unsafe { (*p).splpref.reserved1 = duplicated_pointer("preference-reserved-1") },
        |p| unsafe { (*p).splpref.reserved2 = duplicated_pointer("preference-reserved-2") },
        |p| unsafe { (*p).splrules.unk004 += 1 },
        |p| unsafe { (*p).splrules.match_operator += 1 },
        |p| unsafe { (*p).splrules.reserved_int1 += 1 },
        |p| unsafe { (*p).splrules.reserved_int2 += 1 },
        |p| unsafe { (*p).splrules.reserved1 = duplicated_pointer("rules-reserved-1") },
        |p| unsafe { (*p).splrules.reserved2 = duplicated_pointer("rules-reserved-2") },
        |p| unsafe { first_rule(p).field += 1 },
        |p| unsafe {
            first_rule(p).action += 1;
        },
        |p| unsafe { first_rule(p).string = duplicated_pointer("near-rule") as *mut _ },
        |p| unsafe { first_rule(p).fromvalue += 1 },
        |p| unsafe { first_rule(p).fromdate += 1 },
        |p| unsafe { first_rule(p).fromunits += 1 },
        |p| unsafe { first_rule(p).tovalue += 1 },
        |p| unsafe { first_rule(p).todate += 1 },
        |p| unsafe { first_rule(p).tounits += 1 },
        |p| unsafe { first_rule(p).unk052 += 1 },
        |p| unsafe { first_rule(p).unk056 += 1 },
        |p| unsafe { first_rule(p).unk060 += 1 },
        |p| unsafe { first_rule(p).unk064 += 1 },
        |p| unsafe { first_rule(p).unk068 += 1 },
        |p| unsafe { first_rule(p).reserved_int1 += 1 },
        |p| unsafe { first_rule(p).reserved_int2 += 1 },
        |p| unsafe { first_rule(p).reserved1 = duplicated_pointer("rule-reserved-1") },
        |p| unsafe { first_rule(p).reserved2 = duplicated_pointer("rule-reserved-2") },
        |p| unsafe {
            let extra = ffi::itdb_splr_new();
            ffi::itdb_splr_add(p, extra, -1);
        },
    ]
}

unsafe fn add_profile(
    db: &OwnedDb,
    name: &str,
    timestamp: i64,
    id: u64,
) -> *mut ffi::Itdb_Playlist {
    let profile = firmware_profile(FirmwareProfileId::IpodClassicVideoKindV1);
    let name = CString::new(name).unwrap();
    let playlist = unsafe { ffi::itdb_playlist_new(name.as_ptr(), 1) };
    assert!(!playlist.is_null());
    unsafe {
        (*playlist).id = id;
        (*playlist).timestamp = timestamp as _;
        apply_profile(playlist, profile);
        ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
    }
    playlist
}

unsafe fn apply_profile(playlist: *mut ffi::Itdb_Playlist, profile: &FirmwarePlaylistProfile) {
    unsafe {
        while !(*playlist).splrules.rules.is_null() {
            let rule = (*(*playlist).splrules.rules).data as *mut ffi::Itdb_SPLRule;
            ffi::itdb_splr_remove(playlist, rule);
        }
        (*playlist).is_spl = profile.is_smart as _;
        (*playlist).splpref.liveupdate = profile.preferences.liveupdate;
        (*playlist).splpref.checkrules = profile.preferences.checkrules;
        (*playlist).splpref.checklimits = profile.preferences.checklimits;
        (*playlist).splpref.limittype = profile.preferences.limittype;
        (*playlist).splpref.limitsort = profile.preferences.limitsort;
        (*playlist).splpref.limitvalue = profile.preferences.limitvalue;
        (*playlist).splpref.matchcheckedonly = profile.preferences.matchcheckedonly;
        (*playlist).splpref.reserved_int1 = profile.preferences.reserved_int1;
        (*playlist).splpref.reserved_int2 = profile.preferences.reserved_int2;
        (*playlist).splrules.unk004 = profile.rules_header.unk004;
        (*playlist).splrules.match_operator = profile.rules_header.match_operator;
        (*playlist).splrules.reserved_int1 = profile.rules_header.reserved_int1;
        (*playlist).splrules.reserved_int2 = profile.rules_header.reserved_int2;
        for expected in &profile.rules {
            let rule = ffi::itdb_splr_new();
            assert!(!rule.is_null());
            (*rule).field = expected.field;
            (*rule).action = expected.action;
            (*rule).fromvalue = expected.fromvalue;
            (*rule).fromdate = expected.fromdate;
            (*rule).fromunits = expected.fromunits;
            (*rule).tovalue = expected.tovalue;
            (*rule).todate = expected.todate;
            (*rule).tounits = expected.tounits;
            (*rule).unk052 = expected.unk052;
            (*rule).unk056 = expected.unk056;
            (*rule).unk060 = expected.unk060;
            (*rule).unk064 = expected.unk064;
            (*rule).unk068 = expected.unk068;
            (*rule).reserved_int1 = expected.reserved_int1;
            (*rule).reserved_int2 = expected.reserved_int2;
            ffi::itdb_splr_add(playlist, rule, -1);
        }
    }
}

unsafe fn add_normal(db: &OwnedDb, name: &str, id: u64) {
    let name = CString::new(name).unwrap();
    let playlist = unsafe { ffi::itdb_playlist_new(name.as_ptr(), 0) };
    assert!(!playlist.is_null());
    unsafe {
        (*playlist).id = id;
        ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
    }
}

unsafe fn add_empty_smart(db: &OwnedDb, name: &str, id: u64) {
    let name = CString::new(name).unwrap();
    let playlist = unsafe { ffi::itdb_playlist_new(name.as_ptr(), 1) };
    assert!(!playlist.is_null());
    unsafe {
        while !(*playlist).splrules.rules.is_null() {
            let rule = (*(*playlist).splrules.rules).data as *mut ffi::Itdb_SPLRule;
            ffi::itdb_splr_remove(playlist, rule);
        }
        (*playlist).id = id;
        ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
    }
}

unsafe fn first_rule(playlist: *mut ffi::Itdb_Playlist) -> &'static mut ffi::Itdb_SPLRule {
    unsafe { &mut *((*(*playlist).splrules.rules).data as *mut ffi::Itdb_SPLRule) }
}

unsafe fn duplicated_pointer(value: &str) -> *mut std::ffi::c_void {
    let value = CString::new(value).unwrap();
    unsafe { ffi::g_strdup(value.as_ptr()) as *mut _ }
}

fn fake_mount() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mount = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!("playlist-normalize-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&mount);
    std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    mount
}

fn write_empty_db(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
        assert!(!db.is_null());
        let mount = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(db, mount.as_ptr());
        let name = CString::new("iPod").unwrap();
        let master = ffi::itdb_playlist_new(name.as_ptr(), 0);
        ffi::itdb_playlist_set_mpl(master);
        ffi::itdb_playlist_add(db, master, -1);
        let mut error = ptr::null_mut();
        assert_ne!(ffi::itdb_write(db, &mut error), 0);
        ffi::itdb_free(db);
    }
}
