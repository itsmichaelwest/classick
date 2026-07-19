use super::{DeviceProjectionFs, ProjectionIo, TargetState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "rockbox-projection-fs-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn fixture() -> DeviceProjectionFs {
    let fs = DeviceProjectionFs::new(temp_dir("mount"));
    fs.validate_managed_root().unwrap();
    fs
}

#[test]
fn durable_write_has_exact_bytes_and_leaves_no_temp() {
    let fs = fixture();
    let authorized = HashSet::from(["Gym--0123456789.m3u8".to_string()]);
    fs.write_durable("Gym--0123456789.m3u8", b"/a\n", &authorized, false)
        .unwrap();
    assert_eq!(
        std::fs::read(fs.root().join("Gym--0123456789.m3u8")).unwrap(),
        b"/a\n"
    );
    assert!(fs
        .content_matches(
            "Gym--0123456789.m3u8",
            &blake3::hash(b"/a\n").to_hex().to_string(),
            &authorized,
        )
        .unwrap());
    assert!(!fs
        .content_matches(
            "Gym--0123456789.m3u8",
            &blake3::hash(b"different").to_hex().to_string(),
            &authorized,
        )
        .unwrap());
    assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".classick-")));
}

#[test]
fn foreign_collision_is_classified_and_never_replaced() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    std::fs::write(fs.root().join(name), b"foreign").unwrap();
    assert_eq!(
        fs.target_state(name, &HashSet::new()).unwrap(),
        TargetState::ForeignFile
    );
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs
        .write_durable(name, b"classick", &authorized, false)
        .is_err());
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"foreign");
    assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".classick-")));
}

#[test]
fn replacement_requires_an_existing_authorized_regular_file() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
    std::fs::write(fs.root().join(name), b"old").unwrap();
    assert_eq!(
        fs.target_state(name, &authorized).unwrap(),
        TargetState::RecordedFile
    );
    fs.write_durable(name, b"new", &authorized, true).unwrap();
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"new");
}

#[test]
fn injected_publish_failure_preserves_old_file_and_removes_temp() {
    let mount = temp_dir("injected-failure");
    let name = "Mix--0123456789.m3u8";
    let fs = DeviceProjectionFs::failing_before_rename(mount, name.to_string());
    fs.validate_managed_root().unwrap();
    std::fs::write(fs.root().join(name), b"old").unwrap();
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"old");
    assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".classick-")));
}

#[test]
fn authorization_and_filename_validation_fail_closed() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    let none = HashSet::new();
    assert!(fs.write_durable(name, b"x", &none, false).is_err());
    assert!(fs.remove_recorded(name, &none).is_err());
    for invalid in ["", "..", "../foreign.m3u8", "a/b.m3u8", "a\\b.m3u8"] {
        let authorized = HashSet::from([invalid.to_string()]);
        assert!(fs.write_durable(invalid, b"x", &authorized, false).is_err());
        assert!(fs.remove_recorded(invalid, &authorized).is_err());
    }
}

#[cfg(unix)]
#[test]
fn symlinked_root_is_rejected_without_touching_escape() {
    let outside = temp_dir("outside-root");
    let mount = temp_dir("symlink-root");
    std::fs::create_dir_all(mount.join("Playlists")).unwrap();
    std::os::unix::fs::symlink(&outside, mount.join("Playlists/Classick")).unwrap();
    let fs = DeviceProjectionFs::new(mount);
    let authorized = HashSet::from(["x.m3u8".to_string()]);
    assert!(fs
        .write_durable("x.m3u8", b"owned", &authorized, false)
        .is_err());
    assert!(!outside.join("x.m3u8").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_intermediate_directory_is_rejected() {
    let outside = temp_dir("outside-intermediate");
    let mount = temp_dir("symlink-intermediate");
    std::os::unix::fs::symlink(&outside, mount.join("Playlists")).unwrap();
    let fs = DeviceProjectionFs::new(mount);
    assert!(fs.validate_managed_root().is_err());
    assert!(!outside.join("Classick").exists());
}

#[cfg(unix)]
#[test]
fn symlink_target_is_foreign_and_escape_is_untouched() {
    let fs = fixture();
    let outside = temp_dir("outside-target").join("foreign.m3u8");
    std::fs::write(&outside, b"foreign").unwrap();
    let name = "Mix--0123456789.m3u8";
    std::os::unix::fs::symlink(&outside, fs.root().join(name)).unwrap();
    let authorized = HashSet::from([name.to_string()]);
    assert_eq!(
        fs.target_state(name, &authorized).unwrap(),
        TargetState::ForeignFile
    );
    assert!(fs
        .write_durable(name, b"classick", &authorized, true)
        .is_err());
    assert!(fs.remove_recorded(name, &authorized).is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"foreign");
}

#[test]
fn recorded_delete_is_idempotent_and_preserves_foreign_files() {
    let fs = fixture();
    let owned = "Gym--0123456789.m3u8";
    let foreign = "Foreign--9876543210.m3u8";
    std::fs::write(fs.root().join(owned), b"owned").unwrap();
    std::fs::write(fs.root().join(foreign), b"foreign").unwrap();
    let authorized = HashSet::from([owned.to_string()]);
    assert!(fs.remove_recorded(owned, &authorized).unwrap());
    assert!(!fs.remove_recorded(owned, &authorized).unwrap());
    assert_eq!(std::fs::read(fs.root().join(foreign)).unwrap(), b"foreign");
}
