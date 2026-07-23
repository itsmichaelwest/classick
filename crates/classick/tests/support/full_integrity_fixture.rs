use classick::atomic_file::AtomicFileWriter;
use classick::ffi;
use classick::ipod::db::{OwnedDb, TrackFileDisposition};
use classick::ipod::device_playlists::{
    reconcile_candidate, DesiredPlaylist, PlaylistReconcileOutcome,
};
use classick::ipod::playlist_audit::{snapshot_playlists, PlaylistSnapshot};
use classick::ipod::playlist_normalize::normalize_firmware_playlists;
use classick::ipod::playlist_ownership::{
    DeviceOwnershipStore, ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership,
    MANAGED_PLAYLIST_OWNERSHIP_VERSION,
};
use classick::ipod::playlist_profile::{
    firmware_profile, match_firmware_profile, FirmwarePlaylistProfile, FirmwareProfileId,
};
use classick::sync_transaction::verify_managed_playlists;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

const SERIAL: &str = "RAW-INTEGRITY-SERIAL";
const DOOMED: u64 = 101;
const RETAINED: u64 = 202;
const ADDED: u64 = 303;
const FIXTURE_PLAYLIST_TIMESTAMP: i64 = 1_700_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistPayload {
    snapshot: PlaylistSnapshot,
    members: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignPayloads(BTreeMap<String, PlaylistPayload>);

impl ForeignPayloads {
    pub fn without_deleted_track(mut self, deleted: u64) -> Self {
        for payload in self.0.values_mut() {
            payload.members.retain(|dbid| *dbid != deleted);
            payload.snapshot.member_count = payload.members.len();
        }
        self
    }
}

pub struct FullIntegrityFixture {
    mount: PathBuf,
    state_root: PathBuf,
    db: Option<OwnedDb>,
    managed_id: u64,
}

impl FullIntegrityFixture {
    pub fn new() -> Self {
        let mount = fake_mount();
        write_empty_db(&mount);
        let db = OwnedDb::open(&mount).unwrap();
        let doomed = add_track(&db, DOOMED, "Doomed", ":iPod_Control:Music:F00:doomed.m4a");
        let retained = add_track(
            &db,
            RETAINED,
            "Retained",
            ":iPod_Control:Music:F00:retained.m4a",
        );
        let added = add_track(&db, ADDED, "Added", ":iPod_Control:Music:F00:added.m4a");

        unsafe {
            let master = ffi::itdb_playlist_mpl(db.as_ptr());
            (*master).id = 1;
            (*master).timestamp = 0;
            add_members(master, &[doomed, retained, added]);

            let podcast = add_playlist(&db, "Podcasts", 2, false);
            (*podcast).podcastflag = 1;
            add_members(podcast, &[doomed, retained]);

            let on_the_go = add_playlist(&db, "On-The-Go 1", 3, false);
            add_members(on_the_go, &[doomed, retained]);
            let foreign = add_playlist(&db, "Foreign", 4, false);
            add_members(foreign, &[doomed, retained]);

            let arbitrary = add_playlist(&db, "Arbitrary empty smart", 5, true);
            clear_rules(arbitrary);

            add_profile(&db, "Localized Videos", 0, 6);
            let near = add_profile(&db, "Near match", 0, 7);
            (*near).splrules.match_operator += 1;

            let managed = add_playlist(&db, "Old Mix", 8, false);
            add_members(managed, &[doomed, retained]);
            set_fixture_playlist_timestamps(&db);
        }

        db.write().unwrap();
        drop(db);
        let db = OwnedDb::open(&mount).unwrap();
        let state_root = mount.join("host-state");
        let fixture = Self {
            mount,
            state_root,
            db: Some(db),
            managed_id: 8,
        };
        fixture
            .ownership_store()
            .publish_device(&fixture.previous_ownership())
            .unwrap();
        fixture
    }

    pub fn doomed_dbid(&self) -> u64 {
        DOOMED
    }

    pub fn desired_mix_dbids(&self) -> Vec<u64> {
        vec![ADDED, RETAINED]
    }

    pub fn foreign_payloads(&self) -> ForeignPayloads {
        self.foreign_payloads_from(self.db.as_ref().unwrap())
    }

    pub fn exact_profile_payload(&self) -> PlaylistPayload {
        self.exact_profile_payload_from(self.db.as_ref().unwrap())
    }

    pub fn exact_profile_payload_from(&self, db: &OwnedDb) -> PlaylistPayload {
        playlist_payload(db, 6)
    }

    pub fn foreign_payloads_from(&self, db: &OwnedDb) -> ForeignPayloads {
        ForeignPayloads(
            [
                ("master", 1),
                ("podcast", 2),
                ("on-the-go", 3),
                ("foreign-normal", 4),
                ("arbitrary-smart", 5),
                ("near-match", 7),
            ]
            .into_iter()
            .map(|(label, id)| (label.to_string(), playlist_payload(db, id)))
            .collect(),
        )
    }

    pub fn stage_delete_and_playlist_update(&mut self) -> PlaylistReconcileOutcome {
        let db = self.db.as_ref().unwrap();
        assert!(db
            .remove_track(DOOMED, TrackFileDisposition::DeleteAfterCommit)
            .unwrap());
        let normalized = normalize_firmware_playlists(db).unwrap();
        assert_eq!(normalized.kept, vec![6]);
        assert!(normalized.removed.is_empty());
        reconcile_candidate(
            db,
            &[DesiredPlaylist {
                slug: "mix".into(),
                display_name: "Updated Mix".into(),
                ordered_dbids: self.desired_mix_dbids(),
            }],
            &self.previous_ownership(),
        )
        .unwrap()
    }

    pub fn publish(&mut self, publication: PlaylistReconcileOutcome) -> anyhow::Result<()> {
        unsafe { set_fixture_playlist_timestamps(self.db.as_ref().unwrap()) };
        self.db.as_ref().unwrap().write()?;
        drop(self.db.take());
        let reopened = OwnedDb::open(&self.mount)?;
        verify_managed_playlists(
            &reopened,
            &publication.candidate_ownership,
            &publication.desired_memberships,
        )?;
        drop(reopened);
        self.ownership_store()
            .publish_device(&publication.candidate_ownership)
    }

    pub fn reopen(&self) -> OwnedDb {
        OwnedDb::open(&self.mount).unwrap()
    }

    pub fn exact_profile_count(&self, db: &OwnedDb) -> usize {
        snapshot_playlists(db)
            .iter()
            .filter(|snapshot| {
                match_firmware_profile(snapshot) == Some(FirmwareProfileId::IpodClassicVideoKindV1)
            })
            .count()
    }

    pub fn verified_managed_order(&self, db: &OwnedDb, slug: &str) -> Vec<u64> {
        let ownership = self.device_ownership();
        let desired = BTreeMap::from([(slug.to_string(), self.desired_mix_dbids())]);
        verify_managed_playlists(db, &ownership, &desired)
            .unwrap()
            .into_iter()
            .find(|membership| membership.slug == slug)
            .unwrap()
            .ordered_dbids
    }

    pub fn device_ownership(&self) -> ManagedPlaylistOwnership {
        self.ownership_store().load_device().unwrap()
    }

    fn previous_ownership(&self) -> ManagedPlaylistOwnership {
        ManagedPlaylistOwnership {
            schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
            device_serial: SERIAL.into(),
            playlists: BTreeMap::from([(
                "mix".into(),
                ManagedPlaylistEntry {
                    apple_playlist_id: self.managed_id,
                    expected_kind: ManagedPlaylistKind::Normal,
                    rockbox: None,
                },
            )]),
        }
    }

    fn ownership_store(&self) -> DeviceOwnershipStore {
        DeviceOwnershipStore::new(
            self.mount.clone(),
            SERIAL.into(),
            self.state_root.join("managed_playlists.json"),
            AtomicFileWriter::new(),
        )
    }
}

impl Drop for FullIntegrityFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.mount);
    }
}

fn playlist_payload(db: &OwnedDb, id: u64) -> PlaylistPayload {
    let snapshot = snapshot_playlists(db)
        .into_iter()
        .find(|playlist| playlist.id == id)
        .unwrap();
    PlaylistPayload {
        snapshot,
        members: playlist_members(db, id),
    }
}

fn playlist_members(db: &OwnedDb, id: u64) -> Vec<u64> {
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() && (*playlist).id == id {
                let mut result = Vec::new();
                let mut member = (*playlist).members;
                while !member.is_null() {
                    let track = (*member).data as *mut ffi::Itdb_Track;
                    result.push((*track).dbid);
                    member = (*member).next;
                }
                return result;
            }
            node = (*node).next;
        }
    }
    panic!("playlist {id} missing")
}

fn fake_mount() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mount = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "playlist-full-integrity-{}-{id}",
            std::process::id()
        ));
    let _ = std::fs::remove_dir_all(&mount);
    std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
    for name in ["doomed", "retained", "added"] {
        std::fs::write(
            mount.join(format!("iPod_Control/Music/F00/{name}.m4a")),
            format!("{name} audio"),
        )
        .unwrap();
    }
    mount
}

fn write_empty_db(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
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

fn add_track(db: &OwnedDb, dbid: u64, title: &str, ipod_path: &str) -> *mut ffi::Itdb_Track {
    unsafe {
        let track = ffi::itdb_track_new();
        (*track).dbid = dbid;
        (*track).title = ffi::g_strdup(CString::new(title).unwrap().as_ptr());
        (*track).ipod_path = ffi::g_strdup(CString::new(ipod_path).unwrap().as_ptr());
        ffi::itdb_track_add(db.as_ptr(), track, -1);
        track
    }
}

unsafe fn add_playlist(db: &OwnedDb, name: &str, id: u64, smart: bool) -> *mut ffi::Itdb_Playlist {
    unsafe {
        let playlist = ffi::itdb_playlist_new(CString::new(name).unwrap().as_ptr(), smart as i32);
        (*playlist).id = id;
        ffi::itdb_playlist_add(db.as_ptr(), playlist, -1);
        playlist
    }
}

unsafe fn set_fixture_playlist_timestamps(db: &OwnedDb) {
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() {
                (*playlist).timestamp = FIXTURE_PLAYLIST_TIMESTAMP as _;
            }
            node = (*node).next;
        }
    }
}

unsafe fn add_members(playlist: *mut ffi::Itdb_Playlist, tracks: &[*mut ffi::Itdb_Track]) {
    for track in tracks {
        unsafe { ffi::itdb_playlist_add_track(playlist, *track, -1) };
    }
}

unsafe fn clear_rules(playlist: *mut ffi::Itdb_Playlist) {
    unsafe {
        while !(*playlist).splrules.rules.is_null() {
            let rule = (*(*playlist).splrules.rules).data as *mut ffi::Itdb_SPLRule;
            ffi::itdb_splr_remove(playlist, rule);
        }
    }
}

unsafe fn add_profile(
    db: &OwnedDb,
    name: &str,
    timestamp: i64,
    id: u64,
) -> *mut ffi::Itdb_Playlist {
    unsafe {
        let playlist = add_playlist(db, name, id, true);
        (*playlist).timestamp = timestamp as _;
        apply_profile(
            playlist,
            firmware_profile(FirmwareProfileId::IpodClassicVideoKindV1),
        );
        playlist
    }
}

unsafe fn apply_profile(playlist: *mut ffi::Itdb_Playlist, profile: &FirmwarePlaylistProfile) {
    unsafe {
        clear_rules(playlist);
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
