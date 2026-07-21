use super::readiness::{
    classify_device_readiness, classify_device_readiness_with, DeviceReadiness,
};
use crate::ffi;
#[cfg(unix)]
use crate::ipod::OwnedDb;
use std::ffi::CString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const FACTORY_LAYOUT: &str = "factory-restored.paths";
const INITIALIZED_LAYOUT: &str = "finder-initialized.paths";

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    kind: EntryKind,
    bytes: Option<Vec<u8>>,
    symlink_target: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

struct TestMount(PathBuf);

impl TestMount {
    fn new(label: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "device-readiness-{label}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestMount {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn readiness_serde_uses_snake_case_for_every_state() {
    let cases = [
        (DeviceReadiness::Ready, "ready"),
        (
            DeviceReadiness::NeedsAppleInitialization,
            "needs_apple_initialization",
        ),
        (DeviceReadiness::InvalidDatabase, "invalid_database"),
        (DeviceReadiness::IdentityUnavailable, "identity_unavailable"),
    ];

    for (state, spelling) in cases {
        let json = format!("\"{spelling}\"");
        assert_eq!(serde_json::to_string(&state).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<DeviceReadiness>(&json).unwrap(),
            state
        );
    }
}

#[test]
fn factory_restored_fixture_needs_apple_initialization_without_writes() {
    let mount = materialize_fixture(FACTORY_LAYOUT);

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness_with(mount.path(), |_| panic!("validator must not run")),
        Some(DeviceReadiness::NeedsAppleInitialization),
    );
}

#[test]
fn initialized_fixture_is_ready_when_injected_validator_succeeds_without_writes() {
    let mount = materialize_fixture(INITIALIZED_LAYOUT);
    let mut validator_called = false;

    assert_classification_is_read_only(
        &mount,
        || {
            classify_device_readiness_with(mount.path(), |_| {
                validator_called = true;
                true
            })
        },
        Some(DeviceReadiness::Ready),
    );
    assert!(validator_called);
}

#[test]
fn initialized_fixture_is_invalid_when_injected_validator_fails_without_writes() {
    let mount = materialize_fixture(INITIALIZED_LAYOUT);

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness_with(mount.path(), |_| false),
        Some(DeviceReadiness::InvalidDatabase),
    );
}

#[test]
fn production_classifier_rejects_the_empty_redacted_database_marker_without_writes() {
    let mount = materialize_fixture(INITIALIZED_LAYOUT);

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness(mount.path()),
        Some(DeviceReadiness::InvalidDatabase),
    );
}

#[test]
fn production_classifier_accepts_a_generated_valid_database_without_writes() {
    let mount = TestMount::new("valid-production-database");
    write_valid_itunesdb(mount.path());

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness(mount.path()),
        Some(DeviceReadiness::Ready),
    );
}

#[test]
fn non_ipod_and_partial_layouts_are_not_recognized() {
    let empty = TestMount::new("empty");
    assert_classification_is_read_only(
        &empty,
        || classify_device_readiness_with(empty.path(), |_| true),
        None,
    );

    for missing in ["control", "sysinfo", "itunes"] {
        let mount = TestMount::new(missing);
        create_recognizable_layout(mount.path());
        match missing {
            "control" => fs::remove_dir_all(mount.path().join("iPod_Control")).unwrap(),
            "sysinfo" => fs::remove_file(mount.path().join("iPod_Control/Device/SysInfo")).unwrap(),
            "itunes" => fs::remove_dir_all(mount.path().join("iPod_Control/iTunes")).unwrap(),
            _ => unreachable!(),
        }

        assert_classification_is_read_only(
            &mount,
            || classify_device_readiness_with(mount.path(), |_| true),
            None,
        );
    }
}

#[test]
fn database_directory_is_invalid_and_never_validated() {
    let mount = TestMount::new("database-directory");
    create_recognizable_layout(mount.path());
    fs::create_dir(mount.path().join("iPod_Control/iTunes/iTunesDB")).unwrap();

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness_with(mount.path(), |_| panic!("validator must not run")),
        Some(DeviceReadiness::InvalidDatabase),
    );
}

#[test]
fn database_symlink_is_invalid_without_following_its_target() {
    let mount = TestMount::new("database-symlink");
    create_recognizable_layout(mount.path());
    let outside = TestMount::new("outside-authority");
    let outside_database = outside.path().join("foreign-iTunesDB");
    fs::write(&outside_database, b"foreign database bytes").unwrap();
    let before = fs::read(&outside_database).unwrap();

    if !create_file_symlink(
        &outside_database,
        &mount.path().join("iPod_Control/iTunes/iTunesDB"),
    ) {
        return;
    }

    assert_classification_is_read_only(
        &mount,
        || classify_device_readiness_with(mount.path(), |_| panic!("validator must not run")),
        Some(DeviceReadiness::InvalidDatabase),
    );
    assert_eq!(fs::read(&outside_database).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn database_swapped_to_symlink_during_validation_is_not_ready() {
    let mount = TestMount::new("database-swap");
    create_recognizable_layout(mount.path());
    let database = mount.path().join("iPod_Control/iTunes/iTunesDB");
    fs::write(&database, b"checked database bytes").unwrap();
    let outside = TestMount::new("database-swap-outside");
    let outside_database = outside.path().join("foreign-iTunesDB");
    fs::write(&outside_database, b"foreign database bytes").unwrap();
    let outside_before = fs::read(&outside_database).unwrap();

    let readiness = classify_device_readiness_with(mount.path(), |_| {
        fs::remove_file(&database).unwrap();
        std::os::unix::fs::symlink(&outside_database, &database).unwrap();
        true
    });

    assert_eq!(readiness, Some(DeviceReadiness::InvalidDatabase));
    assert_eq!(fs::read(&outside_database).unwrap(), outside_before);
}

#[cfg(unix)]
#[test]
fn handle_bound_production_parser_does_not_follow_a_swapped_database_symlink() {
    let mount = TestMount::new("parser-database-swap");
    create_recognizable_layout(mount.path());
    let database_path = mount.path().join("iPod_Control/iTunes/iTunesDB");
    fs::write(&database_path, b"not a database").unwrap();

    let outside = TestMount::new("parser-database-swap-outside");
    write_valid_itunesdb(outside.path());
    let outside_database = outside.path().join("iPod_Control/iTunes/iTunesDB");
    assert!(OwnedDb::open(outside.path()).is_ok());
    let mut authority_parse_result = None;

    let readiness = classify_device_readiness_with(mount.path(), |database_authority| {
        fs::remove_file(&database_path).unwrap();
        std::os::unix::fs::symlink(&outside_database, &database_path).unwrap();
        let result = database_authority.is_structurally_valid();
        authority_parse_result = Some(result);
        result
    });

    assert_eq!(authority_parse_result, Some(false));
    assert_eq!(readiness, Some(DeviceReadiness::InvalidDatabase));
}

#[test]
fn committed_path_lists_are_relative_and_exclude_private_identifier_fields() {
    for fixture in [FACTORY_LAYOUT, INITIALIZED_LAYOUT] {
        let contents = fs::read_to_string(fixture_path(fixture)).unwrap();
        for (line_number, line) in contents.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (_, relative) = line.split_once(' ').unwrap();
            let path = Path::new(relative);
            assert!(
                path.components()
                    .all(|component| matches!(component, Component::Normal(_))),
                "unsafe path in {fixture}:{}: {relative}",
                line_number + 1
            );
            let lowered = relative.to_ascii_lowercase();
            for forbidden in [
                "guid",
                "serial",
                "owner",
                "hostname",
                "volumeuuid",
                "rentalclockbias",
                "rbsync",
            ] {
                assert!(
                    !lowered.contains(forbidden),
                    "private field in {fixture}:{}: {relative}",
                    line_number + 1
                );
            }
            assert!(
                !relative
                    .split(['/', '\\'])
                    .any(|component| component.len() == 16
                        && component.bytes().all(|byte| byte.is_ascii_hexdigit())),
                "possible device ID in {fixture}:{}: {relative}",
                line_number + 1
            );
        }
    }
}

fn materialize_fixture(name: &str) -> TestMount {
    let mount = TestMount::new(name.trim_end_matches(".paths"));
    let contents = fs::read_to_string(fixture_path(name)).unwrap();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, relative) = line.split_once(' ').unwrap();
        let path = mount.path().join(relative);
        match kind {
            "D" => fs::create_dir_all(path).unwrap(),
            "F" => {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, []).unwrap();
            }
            _ => panic!("unknown path-list kind {kind:?}"),
        }
    }

    mount
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/device-readiness")
        .join(name)
}

fn create_recognizable_layout(mount: &Path) {
    fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
    fs::write(mount.join("iPod_Control/Device/SysInfo"), []).unwrap();
    fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
}

fn write_valid_itunesdb(mount: &Path) {
    create_recognizable_layout(mount);
    fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();

    unsafe {
        let database = ffi::itdb_new();
        assert!(!database.is_null(), "itdb_new returned null");
        let mount = CString::new(mount.to_str().unwrap()).unwrap();
        ffi::itdb_set_mountpoint(database, mount.as_ptr());
        let title = CString::new("iPod").unwrap();
        let master = ffi::itdb_playlist_new(title.as_ptr(), 0);
        assert!(!master.is_null(), "itdb_playlist_new returned null");
        ffi::itdb_playlist_set_mpl(master);
        ffi::itdb_playlist_add(database, master, -1);
        let mut error: *mut ffi::GError = ptr::null_mut();
        let written = ffi::itdb_write(database, &mut error);
        ffi::itdb_free(database);
        assert_ne!(written, 0, "itdb_write failed generating test database");
    }
}

fn assert_classification_is_read_only(
    mount: &TestMount,
    classify: impl FnOnce() -> Option<DeviceReadiness>,
    expected: Option<DeviceReadiness>,
) {
    let before = snapshot(mount.path());
    assert_eq!(classify(), expected);
    assert_eq!(snapshot(mount.path()), before);
}

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    snapshot_path(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn snapshot_path(root: &Path, path: &Path, entries: &mut Vec<SnapshotEntry>) {
    let metadata = fs::symlink_metadata(path).unwrap();
    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    };
    let bytes = file_type.is_file().then(|| fs::read(path).unwrap());
    let symlink_target = file_type.is_symlink().then(|| fs::read_link(path).unwrap());
    entries.push(SnapshotEntry {
        path: path.strip_prefix(root).unwrap().to_path_buf(),
        kind,
        bytes,
        symlink_target,
    });

    if file_type.is_dir() {
        for child in fs::read_dir(path).unwrap() {
            snapshot_path(root, &child.unwrap().path(), entries);
        }
    }
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("failed to create test symlink: {error}"),
    }
}
