use classick::atomic_file::AtomicFileWriter;
use classick::ipod::playlist_ownership::{
    DeviceOwnershipStore, ManagedPlaylistEntry, ManagedPlaylistKind, ManagedPlaylistOwnership,
    OwnershipOrigin, MANAGED_PLAYLIST_OWNERSHIP_VERSION,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct OwnershipFixture {
    root: PathBuf,
    device_path: PathBuf,
    host_path: PathBuf,
    serial: String,
}

impl OwnershipFixture {
    fn new(serial: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "playlist-ownership-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self {
            device_path: root.join("iPod/iPod_Control/classick/managed_playlists.json"),
            host_path: root.join("state/devices/RAW-SERIAL/managed_playlists.json"),
            root,
            serial: serial.to_string(),
        }
    }

    fn store(&self) -> DeviceOwnershipStore {
        DeviceOwnershipStore::new(
            self.root.join("iPod"),
            self.serial.clone(),
            self.host_path.clone(),
            AtomicFileWriter::new(),
        )
    }

    fn store_with_writer(&self, writer: AtomicFileWriter) -> DeviceOwnershipStore {
        DeviceOwnershipStore::new(
            self.root.join("iPod"),
            self.serial.clone(),
            self.host_path.clone(),
            writer,
        )
    }

    fn write_device_bytes(&self, bytes: &[u8]) {
        std::fs::create_dir_all(self.device_path.parent().unwrap()).unwrap();
        std::fs::write(&self.device_path, bytes).unwrap();
    }

    fn write_host_cache(&self, value: &ManagedPlaylistOwnership) {
        std::fs::create_dir_all(self.host_path.parent().unwrap()).unwrap();
        std::fs::write(&self.host_path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
}

fn ownership(serial: &str, slug: &str, id: u64) -> ManagedPlaylistOwnership {
    ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: serial.to_string(),
        playlists: BTreeMap::from([(
            slug.to_string(),
            ManagedPlaylistEntry {
                apple_playlist_id: id,
                expected_kind: ManagedPlaylistKind::Normal,
                rockbox: None,
            },
        )]),
    }
}

#[test]
fn present_invalid_device_record_never_falls_back_to_host_cache() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    fixture.write_host_cache(&ownership("RAW-Serial", "mix", 7));
    fixture.write_device_bytes(b"{broken");

    let error = fixture.store().load_device().unwrap_err();

    assert!(format!("{error:#}").contains("invalid device playlist ownership"));
}

#[test]
fn missing_device_record_is_empty_authority_not_host_permission() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    fixture.write_host_cache(&ownership("RAW-Serial", "mix", 7));

    let loaded = fixture.store().load_device_with_origin().unwrap();

    assert_eq!(loaded.origin, OwnershipOrigin::Missing);
    assert!(loaded.value.playlists.is_empty());
    assert_eq!(loaded.value.device_serial, "RAW-Serial");
}

#[test]
fn read_only_loads_create_no_directories() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let device_parent = fixture.device_path.parent().unwrap().to_path_buf();
    let host_parent = fixture.host_path.parent().unwrap().to_path_buf();

    let loaded = fixture.store().load_device_read_only().unwrap();

    assert!(loaded.playlists.is_empty());
    assert!(!device_parent.exists());
    assert!(!host_parent.exists());
}

#[test]
fn publish_device_is_replace_atomic_and_validates_exact_raw_serial() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let wrong = ownership("RAW-SERIAL", "mix", 7);
    assert!(fixture.store().publish_device(&wrong).is_err());
    assert!(!fixture.device_path.exists());

    let right = ownership("RAW-Serial", "mix", 7);
    fixture.store().publish_device(&right).unwrap();
    assert_eq!(fixture.store().load_device().unwrap(), right);
}

#[test]
fn rejects_wrong_schema_zero_id_unsafe_slug_and_unknown_kind() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let mut wrong_schema = ownership("RAW-Serial", "mix", 7);
    wrong_schema.schema_version += 1;
    assert!(fixture.store().publish_device(&wrong_schema).is_err());

    let zero = ownership("RAW-Serial", "mix", 0);
    assert!(fixture.store().publish_device(&zero).is_err());

    for slug in ["", "../mix", "mix/name", "mix\\name", ".", "has space"] {
        let unsafe_record = ownership("RAW-Serial", slug, 7);
        assert!(
            fixture.store().publish_device(&unsafe_record).is_err(),
            "{slug:?}"
        );
    }

    fixture.write_device_bytes(
        br#"{"schema_version":1,"device_serial":"RAW-Serial","playlists":{"mix":{"apple_playlist_id":7,"expected_kind":"smart"}}}"#,
    );
    assert!(fixture.store().load_device().is_err());
}

#[test]
fn present_device_record_rejects_unknown_fields() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    fixture.write_device_bytes(
        br#"{"schema_version":1,"device_serial":"RAW-Serial","playlists":{},"permission":true}"#,
    );

    assert!(fixture.store().load_device().is_err());
}

#[test]
fn btree_serialization_is_deterministic_and_reparsed_from_disk() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let mut value = ownership("RAW-Serial", "z-last", 9);
    value.playlists.insert(
        "a-first".into(),
        ManagedPlaylistEntry {
            apple_playlist_id: 1,
            expected_kind: ManagedPlaylistKind::Normal,
            rockbox: None,
        },
    );

    fixture.store().publish_device(&value).unwrap();
    let first = std::fs::read(&fixture.device_path).unwrap();
    fixture.store().publish_device(&value).unwrap();
    let second = std::fs::read(&fixture.device_path).unwrap();

    assert_eq!(first, second);
    let text = String::from_utf8(first).unwrap();
    assert!(text.find("a-first").unwrap() < text.find("z-last").unwrap());
    assert_eq!(fixture.store().load_device().unwrap(), value);
}

#[test]
fn temp_write_failure_preserves_previous_device_file() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let previous = ownership("RAW-Serial", "old", 1);
    fixture.store().publish_device(&previous).unwrap();
    let previous_bytes = std::fs::read(&fixture.device_path).unwrap();
    let writer = AtomicFileWriter::failing_before_replace(fixture.device_path.clone());

    let error = fixture
        .store_with_writer(writer)
        .publish_device(&ownership("RAW-Serial", "new", 2))
        .unwrap_err();

    assert!(format!("{error:#}").contains("injected failure before atomic replace"));
    assert_eq!(std::fs::read(&fixture.device_path).unwrap(), previous_bytes);
    assert_eq!(
        std::fs::read_dir(fixture.device_path.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn host_cache_refresh_requires_published_device_truth() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let value = ownership("RAW-Serial", "mix", 7);

    assert!(fixture.store().refresh_host_cache(&value).is_err());
    assert!(!fixture.host_path.exists());
}

#[test]
fn legacy_host_record_is_retained_but_never_grants_device_authority() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let legacy = br#"{"names":[{"slug":"mix","name":"Mix","id":99}]}"#;
    std::fs::create_dir_all(fixture.host_path.parent().unwrap()).unwrap();
    std::fs::write(&fixture.host_path, legacy).unwrap();
    assert!(fixture.store().load_device().unwrap().playlists.is_empty());

    let candidate = ownership("RAW-Serial", "mix", 7);
    fixture.store().publish_device(&candidate).unwrap();
    assert_eq!(
        fixture.store().refresh_host_cache(&candidate).unwrap(),
        None
    );

    let retained =
        classick::device_state::retained_legacy_managed_playlists_path(&fixture.host_path);
    assert_eq!(std::fs::read(retained).unwrap(), legacy);
    assert_eq!(
        serde_json::from_slice::<ManagedPlaylistOwnership>(
            &std::fs::read(&fixture.host_path).unwrap()
        )
        .unwrap(),
        candidate
    );
}

#[test]
fn host_cache_failure_is_warning_only_and_never_changes_device_bytes() {
    let fixture = OwnershipFixture::new("RAW-Serial");
    let value = ownership("RAW-Serial", "mix", 7);
    fixture.store().publish_device(&value).unwrap();
    let device_bytes = std::fs::read(&fixture.device_path).unwrap();
    let writer = AtomicFileWriter::failing_before_replace(fixture.host_path.clone());

    let warning = fixture
        .store_with_writer(writer)
        .refresh_host_cache(&value)
        .unwrap()
        .expect("host failure should be returned as a warning");

    assert!(warning.contains("refresh host playlist ownership cache"));
    assert_eq!(std::fs::read(&fixture.device_path).unwrap(), device_bytes);
}
