use super::{DeviceProjectionFs, ProjectionIo, TargetState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "rockbox-projection-fs-extra-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn validation_creates_each_managed_directory_below_the_mount() {
    let mount = temp_dir("create-root");
    let fs = DeviceProjectionFs::new(mount.clone());

    assert_eq!(
        fs.validate_managed_root().unwrap(),
        mount.join("Playlists/Classick")
    );
    assert!(mount.join("Playlists").is_dir());
    assert!(mount.join("Playlists/Classick").is_dir());
}

#[test]
fn directory_targets_are_foreign_and_never_mutated() {
    let fs = DeviceProjectionFs::new(temp_dir("directory-target"));
    fs.validate_managed_root().unwrap();
    let name = "Mix--0123456789.m3u8";
    std::fs::create_dir(fs.root().join(name)).unwrap();
    let authorized = HashSet::from([name.to_string()]);

    assert_eq!(
        fs.target_state(name, &authorized).unwrap(),
        TargetState::ForeignFile
    );
    assert!(fs
        .write_durable(name, b"classick", &authorized, true)
        .is_err());
    assert!(fs
        .remove_recorded(
            name,
            blake3::hash(b"classick").to_hex().as_ref(),
            &authorized
        )
        .is_err());
    assert!(fs.root().join(name).is_dir());
}
