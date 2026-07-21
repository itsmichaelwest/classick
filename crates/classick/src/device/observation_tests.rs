use super::{
    assemble_device_observation, classify_device_readiness, DeviceObservationIdentity,
    DeviceReadiness, Fact, FactSource, IpodColour, IpodFamily, ObservationId,
    ReportedDeviceObservation,
};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DEVICE_ID: &str = "A1B2C3D4E5F60708";

#[test]
fn valid_usb_identifiers_share_one_canonical_device_identity() {
    for raw_id in [DEVICE_ID, "a1b2c3d4e5f60708", "0xa1b2c3d4e5f60708"] {
        let observation =
            assemble_device_observation(reported("/Volumes/iPod", 1, Some(raw_id)), |_| {
                Some(DeviceReadiness::Ready)
            })
            .unwrap();

        assert_eq!(observation.device_id().unwrap().as_str(), DEVICE_ID);
        assert_eq!(observation.observation_id(), None);
    }
}

#[test]
fn mount_changes_do_not_change_device_identity() {
    let first = assemble_device_observation(reported("/Volumes/iPod", 1, Some(DEVICE_ID)), |_| {
        Some(DeviceReadiness::Ready)
    })
    .unwrap();
    let second = assemble_device_observation(reported("/media/ipod", 2, Some(DEVICE_ID)), |_| {
        Some(DeviceReadiness::Ready)
    })
    .unwrap();

    assert_eq!(first.device_id(), second.device_id());
    assert_eq!(first.mount_path(), Path::new("/Volumes/iPod"));
    assert_eq!(second.mount_path(), Path::new("/media/ipod"));
}

#[test]
fn missing_and_malformed_identifiers_remain_visible_but_never_mutable() {
    for (raw_id, sequence) in [(None, 7), (Some("not-a-guid"), 8)] {
        let supplied_id = ObservationId::new(sequence);
        let observation = assemble_device_observation(
            reported_with_observation_id("/Volumes/iPod", supplied_id.clone(), raw_id),
            |_| Some(DeviceReadiness::Ready),
        )
        .unwrap();

        assert_eq!(
            observation.identity(),
            &DeviceObservationIdentity::Unavailable(supplied_id.clone())
        );
        assert_eq!(observation.device_id(), None);
        assert_eq!(observation.observation_id(), Some(&supplied_id));
        assert_eq!(
            observation.readiness(),
            DeviceReadiness::IdentityUnavailable
        );
        assert!(!observation.is_mutation_eligible());
        if let Some(rejected_id) = raw_id {
            assert!(!format!("{observation:?}").contains(rejected_id));
        }
    }
}

#[test]
fn unrecognizable_mount_is_not_observed_even_with_valid_identity() {
    let observation =
        assemble_device_observation(reported("/Volumes/not-an-ipod", 1, Some(DEVICE_ID)), |_| {
            None
        });

    assert_eq!(observation, None);
}

#[test]
fn identified_candidates_retain_readiness_and_only_ready_is_mutation_eligible() {
    for (readiness, eligible) in [
        (DeviceReadiness::Ready, true),
        (DeviceReadiness::NeedsAppleInitialization, false),
        (DeviceReadiness::InvalidDatabase, false),
    ] {
        let observation =
            assemble_device_observation(reported("/Volumes/iPod", 1, Some(DEVICE_ID)), |_| {
                Some(readiness)
            })
            .unwrap();

        assert_eq!(observation.readiness(), readiness);
        assert_eq!(observation.is_mutation_eligible(), eligible);
    }
}

#[test]
fn readiness_classifier_runs_exactly_once_for_each_candidate() {
    for raw_id in [Some(DEVICE_ID), None] {
        let calls = Cell::new(0);

        let observation = assemble_device_observation(reported("/Volumes/iPod", 1, raw_id), |_| {
            calls.set(calls.get() + 1);
            Some(DeviceReadiness::InvalidDatabase)
        });

        assert!(observation.is_some());
        assert_eq!(calls.get(), 1);
    }
}

#[test]
fn exact_reported_model_facts_take_precedence_over_ambiguous_usb_facts() {
    let mut input = reported("/Volumes/iPod", 1, Some(DEVICE_ID));
    input.usb_product_id = Some(0x1261);
    input.reported_model_code = Some("mc297".to_owned());
    input.capacity_bytes = Some(80_000_000_000);

    let observation = assemble_device_observation(input, |_| Some(DeviceReadiness::Ready)).unwrap();
    let facts = observation.hardware_facts();

    assert_eq!(facts.family, Some(Fact::decoded(IpodFamily::Classic)));
    assert_eq!(facts.generation, Some(Fact::decoded("3".to_owned())));
    assert_eq!(facts.model_code, Some(Fact::reported("MC297".to_owned())));
    assert_eq!(facts.colour, Some(Fact::decoded(IpodColour::Black)));
    assert_eq!(facts.capacity_bytes, Some(Fact::reported(80_000_000_000)));
}

#[test]
fn ambiguous_classic_usb_facts_never_invent_an_exact_model_or_colour() {
    let mut input = reported("/Volumes/iPod", 1, Some(DEVICE_ID));
    input.usb_product_id = Some(0x1261);
    input.capacity_bytes = Some(160_000_000_000);

    let observation = assemble_device_observation(input, |_| Some(DeviceReadiness::Ready)).unwrap();
    let facts = observation.hardware_facts();

    assert_eq!(facts.family, Some(Fact::decoded(IpodFamily::Classic)));
    assert_eq!(facts.generation, None);
    assert_eq!(facts.model_code, None);
    assert_eq!(facts.colour, None);
}

#[test]
fn firmware_and_exact_capacity_are_reported_facts_and_empty_values_are_omitted() {
    let mut input = reported("/Volumes/iPod", 1, Some(DEVICE_ID));
    input.reported_firmware = Some("2.0.5".to_owned());
    input.capacity_bytes = Some(159_900_000_000);

    let observation = assemble_device_observation(input, |_| Some(DeviceReadiness::Ready)).unwrap();
    let facts = observation.hardware_facts();

    assert_eq!(facts.firmware, Some(Fact::reported("2.0.5".to_owned())));
    assert_eq!(
        facts.firmware.as_ref().unwrap().source,
        FactSource::Reported
    );
    assert_eq!(facts.capacity_bytes, Some(Fact::reported(159_900_000_000)));

    for model_code in ["", " MC297"] {
        let mut input = reported("/Volumes/iPod", 1, Some(DEVICE_ID));
        input.reported_model_code = Some(model_code.to_owned());
        input.reported_firmware = Some(String::new());

        let observation =
            assemble_device_observation(input, |_| Some(DeviceReadiness::Ready)).unwrap();
        let facts = observation.hardware_facts();
        assert_eq!(facts.model_code, None);
        assert_eq!(facts.firmware, None);
    }
}

#[test]
fn public_reported_input_contains_only_platform_observation_facts() {
    let input = reported("/Volumes/iPod", 1, Some(DEVICE_ID));
    let ReportedDeviceObservation {
        mount_path,
        observation_id,
        raw_usb_iserial,
        usb_product_id,
        reported_model_code,
        reported_firmware,
        capacity_bytes,
    } = input;

    assert_eq!(mount_path, Path::new("/Volumes/iPod"));
    assert_eq!(observation_id, ObservationId::new(1));
    assert_eq!(raw_usb_iserial.as_deref(), Some(DEVICE_ID));
    assert_eq!(usb_product_id, None);
    assert_eq!(reported_model_code, None);
    assert_eq!(reported_firmware, None);
    assert_eq!(capacity_bytes, None);
}

#[test]
fn assembly_with_materialized_readiness_fixture_performs_zero_filesystem_writes() {
    let mount = materialize_factory_fixture();
    let before = snapshot(mount.path());

    let observation = assemble_device_observation(
        reported(mount.path(), 1, Some(DEVICE_ID)),
        classify_device_readiness,
    )
    .unwrap();

    assert_eq!(
        observation.readiness(),
        DeviceReadiness::NeedsAppleInitialization
    );
    assert_eq!(snapshot(mount.path()), before);
}

fn reported(
    mount_path: impl Into<PathBuf>,
    observation_sequence: u64,
    raw_usb_iserial: Option<&str>,
) -> ReportedDeviceObservation {
    reported_with_observation_id(
        mount_path,
        ObservationId::new(observation_sequence),
        raw_usb_iserial,
    )
}

fn reported_with_observation_id(
    mount_path: impl Into<PathBuf>,
    observation_id: ObservationId,
    raw_usb_iserial: Option<&str>,
) -> ReportedDeviceObservation {
    ReportedDeviceObservation {
        mount_path: mount_path.into(),
        observation_id,
        raw_usb_iserial: raw_usb_iserial.map(str::to_owned),
        usb_product_id: None,
        reported_model_code: None,
        reported_firmware: None,
        capacity_bytes: None,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    is_directory: bool,
    bytes: Option<Vec<u8>>,
}

struct TestMount(PathBuf);

impl TestMount {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "device-observation-{}-{}",
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

fn materialize_factory_fixture() -> TestMount {
    let mount = TestMount::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/device-readiness/factory-restored.paths");

    for line in fs::read_to_string(fixture).unwrap().lines() {
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

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    snapshot_path(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn snapshot_path(root: &Path, path: &Path, entries: &mut Vec<SnapshotEntry>) {
    let metadata = fs::symlink_metadata(path).unwrap();
    let is_directory = metadata.is_dir();
    entries.push(SnapshotEntry {
        path: path.strip_prefix(root).unwrap().to_path_buf(),
        is_directory,
        bytes: metadata.is_file().then(|| fs::read(path).unwrap()),
    });

    if is_directory {
        for child in fs::read_dir(path).unwrap() {
            snapshot_path(root, &child.unwrap().path(), entries);
        }
    }
}
