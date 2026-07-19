use classick::ffi;
use classick::ipod::playlist_profile::{firmware_profile, FirmwareProfileId};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::UNIX_EPOCH;

pub struct AuditFixture {
    pub mount: PathBuf,
    pub serial: String,
}

impl AuditFixture {
    pub fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let mount = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "playlist-audit-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
        let serial = "0xAudit-Raw-Serial".to_string();
        std::fs::write(
            mount.join("iPod_Control/Device/SysInfo"),
            format!("FirewireGuid: {serial}\n"),
        )
        .unwrap();
        write_audit_db(&mount);
        Self { mount, serial }
    }

    pub fn tree_digest(&self) -> Vec<TreeEntry> {
        let mut files = Vec::new();
        collect_files(&self.mount, &self.mount, &mut files);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    path: String,
    is_directory: bool,
    size: u64,
    modified_ns: u128,
    content_hash: Option<String>,
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<TreeEntry>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = entry.metadata().unwrap();
        if metadata.is_dir() {
            files.push(TreeEntry {
                path: relative_path(root, &path),
                is_directory: true,
                size: metadata.len(),
                modified_ns: modified_ns(&metadata),
                content_hash: None,
            });
            collect_files(root, &path, files);
        } else if metadata.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            files.push(TreeEntry {
                path: relative_path(root, &path),
                is_directory: false,
                size: metadata.len(),
                modified_ns: modified_ns(&metadata),
                content_hash: Some(blake3::hash(&bytes).to_hex().to_string()),
            });
        }
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn modified_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn write_audit_db(mount: &Path) {
    unsafe {
        let db = ffi::itdb_new();
        assert!(!db.is_null());
        let mount_c = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(db, mount_c.as_ptr());

        let master = new_playlist("iPod", false);
        ffi::itdb_playlist_set_mpl(master);
        ffi::itdb_playlist_add(db, master, -1);
        ffi::itdb_playlist_add(db, new_playlist("Foreign", false), -1);
        ffi::itdb_playlist_add(db, new_playlist("Arbitrary Smart", true), -1);

        let exact = new_playlist("Vidéos", true);
        apply_exact_profile(exact);
        ffi::itdb_playlist_add(db, exact, -1);

        let mut error = ptr::null_mut();
        assert_ne!(ffi::itdb_write(db, &mut error), 0);
        ffi::itdb_free(db);
    }
}

unsafe fn new_playlist(name: &str, smart: bool) -> *mut ffi::Itdb_Playlist {
    let name = CString::new(name).unwrap();
    let playlist = unsafe { ffi::itdb_playlist_new(name.as_ptr(), i32::from(smart)) };
    assert!(!playlist.is_null());
    playlist
}

unsafe fn apply_exact_profile(playlist: *mut ffi::Itdb_Playlist) {
    let profile = firmware_profile(FirmwareProfileId::IpodClassicVideoKindV1);
    unsafe {
        while !(*playlist).splrules.rules.is_null() {
            let rule = (*(*playlist).splrules.rules).data as *mut ffi::Itdb_SPLRule;
            assert!(!rule.is_null());
            ffi::itdb_splr_remove(playlist, rule);
        }
        (*playlist).is_spl = i32::from(profile.is_smart);
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
        for snapshot in &profile.rules {
            let rule = ffi::itdb_splr_add_new(playlist, -1);
            assert!(!rule.is_null());
            (*rule).field = snapshot.field;
            (*rule).action = snapshot.action;
            (*rule).fromvalue = snapshot.fromvalue;
            (*rule).fromdate = snapshot.fromdate;
            (*rule).fromunits = snapshot.fromunits;
            (*rule).tovalue = snapshot.tovalue;
            (*rule).todate = snapshot.todate;
            (*rule).tounits = snapshot.tounits;
            (*rule).unk052 = snapshot.unk052;
            (*rule).unk056 = snapshot.unk056;
            (*rule).unk060 = snapshot.unk060;
            (*rule).unk064 = snapshot.unk064;
            (*rule).unk068 = snapshot.unk068;
            (*rule).reserved_int1 = snapshot.reserved_int1;
            (*rule).reserved_int2 = snapshot.reserved_int2;
        }
    }
}
