use super::discovery::{observe_mount_with_probe, OrdinaryUsbFacts};
use super::{DeviceObservationIdentity, DeviceReadiness, Fact, IpodFamily, ObservationId};
use crate::ffi;
use std::cell::Cell;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const DEVICE_ID: &str = "A1B2C3D4E5F60708";

#[test]
fn restored_mount_with_ordinary_usb_identity_is_visible_without_writes() {
    let mount = TestMount::recognizable("restored", "");
    let before = snapshot(mount.path());

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(41), |_| {
        Some(usb_facts(
            Some(DEVICE_ID),
            Some(0x1261),
            Some(160_000_000_000),
        ))
    })
    .unwrap();

    assert_eq!(
        observation.readiness(),
        DeviceReadiness::NeedsAppleInitialization
    );
    assert_eq!(observation.device_id().unwrap().as_str(), DEVICE_ID);
    assert_eq!(snapshot(mount.path()), before);
}

#[test]
fn initialized_empty_database_marker_is_invalid_without_writes() {
    let mount = TestMount::recognizable("invalid-database", "");
    fs::write(mount.path().join("iPod_Control/iTunes/iTunesDB"), []).unwrap();
    let before = snapshot(mount.path());

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(42), |_| {
        Some(usb_facts(Some(DEVICE_ID), None, None))
    })
    .unwrap();

    assert_eq!(observation.readiness(), DeviceReadiness::InvalidDatabase);
    assert_eq!(snapshot(mount.path()), before);
}

#[test]
fn unavailable_usb_identity_keeps_the_supplied_observation_id_and_is_never_mutable() {
    let cases = [
        Some(usb_facts(None, Some(0x1261), Some(160_000_000_000))),
        Some(usb_facts(
            Some("malformed-private-value"),
            Some(0x1261),
            Some(160_000_000_000),
        )),
        None,
    ];

    for (sequence, facts) in cases.into_iter().enumerate() {
        let mount = TestMount::recognizable(&format!("identity-unavailable-{sequence}"), "");
        let observation_id = ObservationId::new(sequence as u64 + 50);
        let before = snapshot(mount.path());
        let observation =
            observe_mount_with_probe(mount.path(), observation_id.clone(), move |_| facts).unwrap();

        assert_eq!(
            observation.identity(),
            &DeviceObservationIdentity::Unavailable(observation_id.clone())
        );
        assert_eq!(observation.observation_id(), Some(&observation_id));
        assert_eq!(
            observation.readiness(),
            DeviceReadiness::IdentityUnavailable
        );
        assert!(!observation.is_mutation_eligible());
        assert_eq!(snapshot(mount.path()), before);
        assert!(!format!("{observation:?}").contains("malformed-private-value"));
    }
}

#[test]
fn exact_pid_and_capacity_flow_to_the_catalogue_without_inventing_a_classic_variant() {
    let mount = TestMount::recognizable("ambiguous-classic", "");

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(60), |_| {
        Some(usb_facts(
            Some(DEVICE_ID),
            Some(0x1261),
            Some(160_000_000_000),
        ))
    })
    .unwrap();
    let facts = observation.hardware_facts();

    assert_eq!(facts.family, Some(Fact::decoded(IpodFamily::Classic)));
    assert_eq!(facts.capacity_bytes, Some(Fact::reported(160_000_000_000)));
    assert_eq!(facts.generation, None);
    assert_eq!(facts.model_code, None);
    assert_eq!(facts.colour, None);
}

#[test]
fn genuine_sysinfo_model_and_firmware_are_optional_read_only_facts() {
    let mount = TestMount::recognizable(
        "sysinfo-facts",
        "ModelNumStr: MC297\nFirmwareVersion: 2.0.5\n",
    );
    let before = snapshot(mount.path());

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(61), |_| {
        Some(usb_facts(
            Some(DEVICE_ID),
            Some(0x1261),
            Some(160_000_000_000),
        ))
    })
    .unwrap();
    let facts = observation.hardware_facts();

    assert_eq!(facts.model_code, Some(Fact::reported("MC297".to_owned())));
    assert_eq!(facts.firmware, Some(Fact::reported("2.0.5".to_owned())));
    assert_eq!(snapshot(mount.path()), before);

    let empty =
        TestMount::recognizable("empty-sysinfo-facts", "ModelNumStr:   \nFirmwareVersion:\n");
    let observation = observe_mount_with_probe(empty.path(), ObservationId::new(62), |_| {
        Some(usb_facts(Some(DEVICE_ID), None, None))
    })
    .unwrap();
    assert_eq!(observation.hardware_facts().model_code, None);
    assert_eq!(observation.hardware_facts().firmware, None);
}

#[test]
fn ordinary_probe_runs_once_and_only_after_the_mount_is_recognized() {
    let recognized = TestMount::recognizable("probe-once", "");
    let calls = Cell::new(0);

    let observation = observe_mount_with_probe(recognized.path(), ObservationId::new(70), |_| {
        calls.set(calls.get() + 1);
        Some(usb_facts(Some(DEVICE_ID), None, None))
    });

    assert!(observation.is_some());
    assert_eq!(calls.get(), 1);

    let non_candidate = TestMount::empty("probe-skipped");
    let calls = Cell::new(0);
    let observation =
        observe_mount_with_probe(non_candidate.path(), ObservationId::new(71), |_| {
            calls.set(calls.get() + 1);
            Some(usb_facts(Some(DEVICE_ID), None, None))
        });

    assert_eq!(observation, None);
    assert_eq!(calls.get(), 0);
}

#[cfg(unix)]
#[test]
fn mount_replacement_during_usb_probe_is_rejected() {
    let mount = TestMount::recognizable(
        "mount-swap-original",
        "ModelNumStr: MC293\nFirmwareVersion: 2.0.4\n",
    );
    let replacement = TestMount::recognizable(
        "mount-swap-replacement",
        "ModelNumStr: MC297\nFirmwareVersion: 2.0.5\n",
    );
    write_valid_itunesdb(mount.path());
    let retired = mount.path().with_extension("retired");

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(72), |_| {
        fs::rename(mount.path(), &retired).unwrap();
        fs::rename(replacement.path(), mount.path()).unwrap();
        Some(usb_facts(Some(DEVICE_ID), Some(0x1261), None))
    });

    fs::rename(mount.path(), replacement.path()).unwrap();
    fs::rename(retired, mount.path()).unwrap();
    assert_eq!(observation, None);
}

#[cfg(unix)]
#[test]
fn sysinfo_replacement_during_usb_probe_is_rejected() {
    let mount = TestMount::recognizable(
        "sysinfo-swap",
        "ModelNumStr: MC293\nFirmwareVersion: 2.0.4\n",
    );
    let sysinfo = mount.path().join("iPod_Control/Device/SysInfo");
    let original = sysinfo.with_extension("original");
    let replacement = sysinfo.with_extension("replacement");
    fs::write(&replacement, "ModelNumStr: MC297\nFirmwareVersion: 2.0.5\n").unwrap();

    let observation = observe_mount_with_probe(mount.path(), ObservationId::new(73), |_| {
        fs::rename(&sysinfo, &original).unwrap();
        fs::rename(&replacement, &sysinfo).unwrap();
        Some(usb_facts(Some(DEVICE_ID), Some(0x1261), None))
    });

    fs::rename(&sysinfo, &replacement).unwrap();
    fs::rename(original, &sysinfo).unwrap();
    fs::remove_file(replacement).unwrap();
    assert_eq!(observation, None);
}

#[test]
fn ordinary_usb_facts_expose_only_the_approved_os_observations() {
    let facts = usb_facts(Some(DEVICE_ID), Some(0x1261), Some(160_000_000_000));
    let OrdinaryUsbFacts {
        raw_usb_iserial,
        usb_product_id,
        capacity_bytes,
    } = facts;

    assert_eq!(raw_usb_iserial.as_deref(), Some(DEVICE_ID));
    assert_eq!(usb_product_id, Some(0x1261));
    assert_eq!(capacity_bytes, Some(160_000_000_000));
}

#[test]
fn production_device_discovery_and_identity_have_no_scsi_reference() {
    let discovery = include_str!("discovery.rs");
    let legacy_adapter = include_str!("../ipod/device.rs");

    for source in [discovery, legacy_adapter] {
        assert!(!source.contains("crate::scsi_inquiry"));
        assert!(!source.contains("read_sysinfo_extended"));
    }
}

fn usb_facts(
    raw_usb_iserial: Option<&str>,
    usb_product_id: Option<u16>,
    capacity_bytes: Option<u64>,
) -> OrdinaryUsbFacts {
    OrdinaryUsbFacts {
        raw_usb_iserial: raw_usb_iserial.map(str::to_owned),
        usb_product_id,
        capacity_bytes,
    }
}

fn write_valid_itunesdb(mount: &Path) {
    fs::create_dir_all(mount.join("iPod_Control/Music/F00")).unwrap();
    let sysinfo = mount.join("iPod_Control/Device/SysInfo");
    let sysinfo_contents = fs::read(&sysinfo).unwrap();
    fs::write(&sysinfo, []).unwrap();

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
    fs::write(sysinfo, sysinfo_contents).unwrap();
}

struct TestMount(PathBuf);

impl TestMount {
    fn empty(label: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "ordinary-observation-{label}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn recognizable(label: &str, sysinfo: &str) -> Self {
        let mount = Self::empty(label);
        fs::create_dir_all(mount.path().join("iPod_Control/Device")).unwrap();
        fs::create_dir_all(mount.path().join("iPod_Control/iTunes")).unwrap();
        fs::write(mount.path().join("iPod_Control/Device/SysInfo"), sysinfo).unwrap();
        mount
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

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    is_directory: bool,
    bytes: Option<Vec<u8>>,
}

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    fn walk(root: &Path, current: &Path, entries: &mut Vec<SnapshotEntry>) {
        let mut children = fs::read_dir(current)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            entries.push(SnapshotEntry {
                path: path.strip_prefix(root).unwrap().to_path_buf(),
                is_directory: metadata.is_dir(),
                bytes: metadata.is_file().then(|| fs::read(&path).unwrap()),
            });
            if metadata.is_dir() {
                walk(root, &path, entries);
            }
        }
    }

    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries
}
