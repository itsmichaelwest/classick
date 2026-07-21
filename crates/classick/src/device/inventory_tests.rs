use super::{
    assemble_device_observation, DeviceObservation, DeviceObservationScanner, DeviceReadiness,
    ObservationId, ReportedDeviceObservation,
};
use std::path::{Path, PathBuf};

#[test]
fn recognizable_non_ready_and_identity_unavailable_mounts_remain_in_inventory() {
    let restored = PathBuf::from("/Volumes/restored");
    let invalid = PathBuf::from("/Volumes/invalid");
    let unavailable = PathBuf::from("/Volumes/unavailable");
    let mut scanner = DeviceObservationScanner::new();

    let inventory = scanner.scan_with(
        [&restored, &invalid, &unavailable],
        |mount, observation_id| match mount {
            path if path == restored => Some(observation(
                path,
                observation_id,
                Some("000A27002138B0A8"),
                DeviceReadiness::NeedsAppleInitialization,
            )),
            path if path == invalid => Some(observation(
                path,
                observation_id,
                Some("000A27002138B0A9"),
                DeviceReadiness::InvalidDatabase,
            )),
            path if path == unavailable => Some(observation(
                path,
                observation_id,
                None,
                DeviceReadiness::Ready,
            )),
            _ => None,
        },
    );

    assert_eq!(inventory.observations().len(), 3);
    assert!(inventory
        .observations()
        .iter()
        .any(|entry| entry.readiness() == DeviceReadiness::NeedsAppleInitialization));
    assert!(inventory
        .observations()
        .iter()
        .any(|entry| entry.readiness() == DeviceReadiness::InvalidDatabase));
    assert!(inventory
        .observations()
        .iter()
        .any(|entry| entry.readiness() == DeviceReadiness::IdentityUnavailable));
}

#[test]
fn unavailable_mount_keeps_its_id_until_disappearance_then_reconnects_with_a_new_id() {
    let mount = PathBuf::from("/Volumes/unavailable");
    let mut scanner = DeviceObservationScanner::new();

    let first = scanner.scan_with([&mount], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });
    let first_id = first.observations()[0].observation_id().unwrap().clone();

    let second = scanner.scan_with([&mount], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });
    assert_eq!(second.observations()[0].observation_id(), Some(&first_id));

    scanner.scan_with(std::iter::empty::<&Path>(), |_, _| {
        unreachable!("empty candidate scan must not invoke the observer")
    });
    let reconnected = scanner.scan_with([&mount], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });

    assert_ne!(
        reconnected.observations()[0].observation_id(),
        Some(&first_id)
    );
}

#[test]
fn unavailable_id_survives_temporary_identity_recovery_on_the_same_connection() {
    let mount = PathBuf::from("/Volumes/intermittent-identity");
    let mut scanner = DeviceObservationScanner::new();
    let unavailable = scanner.scan_with([&mount], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });
    let original_id = unavailable.observations()[0]
        .observation_id()
        .unwrap()
        .clone();

    scanner.scan_with([&mount], |path, id| {
        Some(observation(
            path,
            id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
        ))
    });
    let unavailable_again = scanner.scan_with([&mount], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });

    assert_eq!(
        unavailable_again.observations()[0].observation_id(),
        Some(&original_id)
    );
}

#[test]
fn canonical_device_identity_survives_a_mount_path_change() {
    let first_mount = PathBuf::from("/Volumes/iPod");
    let second_mount = PathBuf::from("/Volumes/iPod 1");
    let mut scanner = DeviceObservationScanner::new();

    let first = scanner.scan_with([&first_mount], |path, id| {
        Some(observation(
            path,
            id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
        ))
    });
    let first_id = first.observations()[0].device_id().unwrap().clone();
    let second = scanner.scan_with([&second_mount], |path, id| {
        Some(observation(
            path,
            id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
        ))
    });

    assert_eq!(second.observations()[0].device_id(), Some(&first_id));
    assert_eq!(second.observations()[0].mount_path(), second_mount);
}

#[test]
fn duplicate_live_device_id_blocks_every_claim_until_the_conflict_clears() {
    let first_mount = PathBuf::from("/Volumes/first");
    let second_mount = PathBuf::from("/Volumes/second");
    let mut scanner = DeviceObservationScanner::new();

    let duplicate = scanner.scan_with([&first_mount, &second_mount], |path, id| {
        Some(observation(
            path,
            id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
        ))
    });

    assert_eq!(duplicate.observations().len(), 2);
    assert!(duplicate
        .observations()
        .iter()
        .all(|entry| !duplicate.is_uniquely_mutation_eligible(entry)));

    let cleared = scanner.scan_with([&first_mount], |path, id| {
        Some(observation(
            path,
            id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
        ))
    });
    assert!(cleared.is_uniquely_mutation_eligible(&cleared.observations()[0]));
}

#[test]
fn distinct_devices_are_independent_and_sorted_by_canonical_id() {
    let later = PathBuf::from("/Volumes/later");
    let earlier = PathBuf::from("/Volumes/earlier");
    let mut scanner = DeviceObservationScanner::new();

    let inventory = scanner.scan_with([&later, &earlier], |path, id| {
        let identity = if path == later {
            "000A27002138B0AF"
        } else {
            "000A27002138B0A1"
        };
        Some(observation(
            path,
            id,
            Some(identity),
            DeviceReadiness::Ready,
        ))
    });

    assert_eq!(
        inventory.observations()[0].device_id().unwrap().as_str(),
        "000A27002138B0A1"
    );
    assert_eq!(
        inventory.observations()[1].device_id().unwrap().as_str(),
        "000A27002138B0AF"
    );
    assert!(inventory
        .observations()
        .iter()
        .all(|entry| inventory.is_uniquely_mutation_eligible(entry)));
}

#[test]
fn each_supplied_candidate_is_observed_once_and_unrecognized_mounts_are_not_retained() {
    let recognized = PathBuf::from("/Volumes/recognized");
    let unrecognized = PathBuf::from("/Volumes/unrecognized");
    let mut scanner = DeviceObservationScanner::new();
    let mut calls = Vec::new();
    let mut unrecognized_id = None;

    let inventory = scanner.scan_with([&recognized, &unrecognized], |path, id| {
        calls.push(path.to_path_buf());
        if path == recognized {
            Some(observation(path, id, None, DeviceReadiness::Ready))
        } else {
            unrecognized_id = Some(id);
            None
        }
    });

    assert_eq!(calls, vec![recognized.clone(), unrecognized.clone()]);
    assert_eq!(inventory.observations().len(), 1);

    let first_unrecognized_id = scanner.scan_with([&unrecognized], |path, id| {
        Some(observation(path, id, None, DeviceReadiness::Ready))
    });
    assert_ne!(
        unrecognized_id.as_ref(),
        first_unrecognized_id.observations()[0].observation_id(),
        "an unrecognized candidate must not retain ephemeral bookkeeping"
    );
}

fn observation(
    mount_path: &Path,
    observation_id: ObservationId,
    raw_usb_iserial: Option<&str>,
    readiness: DeviceReadiness,
) -> DeviceObservation {
    assemble_device_observation(
        ReportedDeviceObservation {
            mount_path: mount_path.to_path_buf(),
            observation_id,
            raw_usb_iserial: raw_usb_iserial.map(str::to_owned),
            usb_product_id: None,
            reported_model_code: None,
            reported_firmware: None,
            capacity_bytes: None,
        },
        |_| Some(readiness),
    )
    .unwrap()
}
