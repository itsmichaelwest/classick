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

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
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
            blake3::hash(b"/a\n").to_hex().as_ref(),
            &authorized,
        )
        .unwrap());
    assert!(!fs
        .content_matches(
            "Gym--0123456789.m3u8",
            blake3::hash(b"different").to_hex().as_ref(),
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
fn target_state_probe_does_not_create_the_managed_root() {
    let mount = temp_dir("read-only-probe");
    let fs = DeviceProjectionFs::new(mount);
    let name = "Gym--0123456789.m3u8";

    assert_eq!(
        fs.target_state(name, &HashSet::from([name.to_string()]))
            .unwrap(),
        TargetState::Missing
    );
    assert!(!fs.root().exists());
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
fn in_place_replacement_is_rejected_before_staging() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    let authorized = HashSet::from([name.to_string()]);
    assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
    std::fs::write(fs.root().join(name), b"old").unwrap();
    assert_eq!(
        fs.target_state(name, &authorized).unwrap(),
        TargetState::RecordedFile
    );
    assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"old");
    assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".classick-")));
}

#[test]
fn differently_cased_directory_entry_never_receives_recorded_authority() {
    let fs = fixture();
    let recorded = "Mix--0123456789.m3u8";
    let foreign_variant = "mix--0123456789.m3u8";
    std::fs::write(fs.root().join(foreign_variant), b"foreign").unwrap();
    if std::fs::symlink_metadata(fs.root().join(recorded)).is_err() {
        return;
    }
    let authorized = HashSet::from([recorded.to_string()]);

    assert_eq!(
        fs.target_state(recorded, &authorized).unwrap(),
        TargetState::ForeignFile
    );
    assert!(fs
        .write_durable(recorded, b"classick", &authorized, true)
        .is_err());
    assert!(fs
        .remove_recorded(recorded, &hash(b"foreign"), &authorized)
        .is_err());
    assert_eq!(
        std::fs::read(fs.root().join(foreign_variant)).unwrap(),
        b"foreign"
    );
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

#[cfg(unix)]
#[test]
fn target_swap_between_validation_and_delete_cannot_touch_escape() {
    let mount = temp_dir("target-delete-swap-mount");
    let fs = DeviceProjectionFs::new(mount.clone());
    fs.validate_managed_root().unwrap();
    let name = "Mix--0123456789.m3u8";
    std::fs::write(fs.root().join(name), b"recorded").unwrap();
    let outside = temp_dir("target-delete-swap-outside").join("outside.m3u8");
    std::fs::write(&outside, b"outside").unwrap();
    DeviceProjectionFs::swap_target_before_mutation_once(mount, name.to_string(), outside.clone());
    let authorized = HashSet::from([name.to_string()]);

    assert!(fs
        .remove_recorded(name, &hash(b"recorded"), &authorized)
        .is_err());

    assert_eq!(std::fs::read(outside).unwrap(), b"outside");
    assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"outside");
}

#[test]
fn authorization_and_filename_validation_fail_closed() {
    let fs = fixture();
    let name = "Mix--0123456789.m3u8";
    let none = HashSet::new();
    assert!(fs.write_durable(name, b"x", &none, false).is_err());
    assert!(fs.remove_recorded(name, &hash(b"x"), &none).is_err());
    for invalid in ["", "..", "../foreign.m3u8", "a/b.m3u8", "a\\b.m3u8"] {
        let authorized = HashSet::from([invalid.to_string()]);
        assert!(fs.write_durable(invalid, b"x", &authorized, false).is_err());
        assert!(fs
            .remove_recorded(invalid, &hash(b"x"), &authorized)
            .is_err());
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
    assert!(fs
        .remove_recorded(name, &hash(b"classick"), &authorized)
        .is_err());
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
    assert!(fs
        .remove_recorded(owned, &hash(b"owned"), &authorized)
        .unwrap());
    assert!(!fs
        .remove_recorded(owned, &hash(b"owned"), &authorized)
        .unwrap());
    assert_eq!(std::fs::read(fs.root().join(foreign)).unwrap(), b"foreign");
}

#[cfg(unix)]
#[test]
fn cleanup_failure_retains_only_the_deterministic_authorized_quarantine() {
    let mount = temp_dir("delete-cleanup-retry");
    let fs = DeviceProjectionFs::new(mount.clone());
    fs.validate_managed_root().unwrap();
    let name = "Gym--0123456789.m3u8";
    let expected_hash = hash(b"owned");
    std::fs::write(fs.root().join(name), b"owned").unwrap();
    let authorized = HashSet::from([name.to_string()]);
    DeviceProjectionFs::fail_once_for_mount(mount, super::ProjectionFailurePoint::DeleteCleanup);

    assert!(fs
        .remove_recorded(name, &expected_hash, &authorized)
        .is_err());

    let quarantine = super::deletion_quarantine_name(name, &expected_hash).unwrap();
    assert!(!fs.root().join(name).exists());
    assert_eq!(
        std::fs::read(fs.root().join(&quarantine)).unwrap(),
        b"owned"
    );
    assert_eq!(
        std::fs::read_dir(fs.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec![quarantine.clone()]
    );

    assert!(fs
        .remove_recorded(name, &expected_hash, &authorized)
        .unwrap());
    assert!(!fs.root().join(quarantine).exists());
}

#[cfg(unix)]
#[test]
fn missing_delete_retry_still_syncs_the_directory_after_post_unlink_failure() {
    let mount = temp_dir("delete-post-unlink-sync-retry");
    let fs = DeviceProjectionFs::new(mount.clone());
    fs.validate_managed_root().unwrap();
    let name = "Gym--0123456789.m3u8";
    let expected_hash = hash(b"owned");
    std::fs::write(fs.root().join(name), b"owned").unwrap();
    let authorized = HashSet::from([name.to_string()]);
    DeviceProjectionFs::fail_once_for_mount(
        mount.clone(),
        super::ProjectionFailurePoint::DeleteSync,
    );

    assert!(fs
        .remove_recorded(name, &expected_hash, &authorized)
        .is_err());
    assert!(!fs.root().join(name).exists());
    assert_eq!(std::fs::read_dir(fs.root()).unwrap().count(), 0);

    let syncs_before_retry = DeviceProjectionFs::delete_sync_count_for_mount(&mount);
    assert!(!fs
        .remove_recorded(name, &expected_hash, &authorized)
        .unwrap());
    assert_eq!(
        DeviceProjectionFs::delete_sync_count_for_mount(&mount),
        syncs_before_retry + 1
    );
}
