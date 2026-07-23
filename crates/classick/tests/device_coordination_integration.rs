use classick::device::DeviceId;
use classick::device_coordination::{
    CoordinationFailure, DeviceMutationSession, ExternalGenerationChange,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const DEVICE_ID: &str = "000A27002138B0A8";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempMount(std::path::PathBuf);

impl TempMount {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "classick-coordination-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("iPod_Control/iTunes")).unwrap();
        fs::write(
            path.join("iPod_Control/iTunes/iTunesDB"),
            b"initial database",
        )
        .unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempMount {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn device_id() -> DeviceId {
    DeviceId::parse(DEVICE_ID).unwrap()
}

#[test]
fn one_device_has_one_live_classick_writer_and_unlocked_sidecar_is_reusable() {
    let mount = TempMount::new("contention");
    let first = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();

    let error = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap_err();
    assert!(matches!(error, CoordinationFailure::AlreadyLocked));

    drop(first);
    DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();
    assert!(mount
        .path()
        .join("iPod_Control/classick/device.lock")
        .is_file());
}

#[test]
fn different_devices_can_be_mutated_concurrently() {
    let first_mount = TempMount::new("first");
    let second_mount = TempMount::new("second");

    let _first = DeviceMutationSession::acquire(first_mount.path(), device_id()).unwrap();
    let _second = DeviceMutationSession::acquire(
        second_mount.path(),
        DeviceId::parse("000A27002138B0A9").unwrap(),
    )
    .unwrap();
}

#[test]
fn generation_fence_detects_external_metadata_changes() {
    let mount = TempMount::new("generation");
    let session = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();
    session.verify_expected_generation().unwrap();

    fs::write(
        mount.path().join("iPod_Control/iTunes/iTunesDB"),
        b"external database",
    )
    .unwrap();

    let error = session.verify_expected_generation().unwrap_err();
    assert!(matches!(error, ExternalGenerationChange::Changed { .. }));
}

#[test]
fn generation_fence_ignores_macos_appledouble_metadata() {
    let mount = TempMount::new("appledouble-generation");
    let session = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();

    fs::write(
        mount.path().join("iPod_Control/iTunes/._iTunesDB"),
        b"volatile AppleDouble metadata",
    )
    .unwrap();
    fs::write(
        mount.path().join("iPod_Control/classick/._pending"),
        b"volatile directory metadata",
    )
    .unwrap();

    session.verify_expected_generation().unwrap();
}

#[test]
fn verified_owned_publication_advances_the_expected_generation() {
    let mount = TempMount::new("advance");
    let session = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();

    session
        .publish_verified(|| {
            fs::write(
                mount.path().join("iPod_Control/iTunes/iTunesDB"),
                b"classick database",
            )?;
            Ok(())
        })
        .unwrap();

    session.verify_expected_generation().unwrap();
}

#[test]
fn lock_path_redirection_fails_closed() {
    let mount = TempMount::new("redirect");
    let classick = mount.path().join("iPod_Control/classick");
    fs::create_dir_all(&classick).unwrap();

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            mount.path().join("iPod_Control/iTunes/iTunesDB"),
            classick.join("device.lock"),
        )
        .unwrap();
        let error = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap_err();
        assert!(matches!(error, CoordinationFailure::UnsafeLeasePath { .. }));
    }
}

#[test]
fn child_process_cannot_take_a_held_lease() {
    if std::env::var_os("CLASSICK_COORDINATION_CHILD").is_some() {
        let mount = std::env::var_os("CLASSICK_COORDINATION_MOUNT").unwrap();
        let result = DeviceMutationSession::acquire(Path::new(&mount), device_id());
        assert!(matches!(result, Err(CoordinationFailure::AlreadyLocked)));
        return;
    }

    let mount = TempMount::new("child");
    let _session = DeviceMutationSession::acquire(mount.path(), device_id()).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_process_cannot_take_a_held_lease")
        .arg("--nocapture")
        .env("CLASSICK_COORDINATION_CHILD", "1")
        .env("CLASSICK_COORDINATION_MOUNT", mount.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
