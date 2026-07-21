use super::legacy_v2::{adapt_known_mount, adapt_observation_inventory, LegacyV2PollingCache};
use super::{
    assemble_device_observation, DeviceObservation, DeviceObservationScanner, DeviceReadiness,
    ObservationId, OrdinaryUsbFacts, ReportedDeviceObservation,
};
use crate::ipod::device::DetectedIpod;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[path = "legacy_v2_polling_identity_tests.rs"]
mod polling_identity_tests;

#[test]
fn includes_only_unique_ready_observations_with_truthful_fields() {
    let ready = PathBuf::from("/Volumes/ready");
    let restored = PathBuf::from("/Volumes/restored");
    let invalid = PathBuf::from("/Volumes/invalid");
    let unavailable = PathBuf::from("/Volumes/unavailable");
    let mut scanner = DeviceObservationScanner::new();
    let inventory = scanner.scan_with(
        [&ready, &restored, &invalid, &unavailable],
        |mount, observation_id| {
            let (identity, readiness) = if mount == ready {
                (Some("000A27002138B0A8"), DeviceReadiness::Ready)
            } else if mount == restored {
                (
                    Some("000A27002138B0A9"),
                    DeviceReadiness::NeedsAppleInitialization,
                )
            } else if mount == invalid {
                (Some("000A27002138B0AA"), DeviceReadiness::InvalidDatabase)
            } else {
                (None, DeviceReadiness::Ready)
            };
            Some(observation(
                mount,
                observation_id,
                identity,
                readiness,
                Some(0x1261),
                None,
                Some(160_000_000_000),
            ))
        },
    );

    let detected = adapt_observation_inventory(&inventory);

    assert_eq!(
        detected,
        vec![DetectedIpod {
            serial: "0x000A27002138B0A8".to_owned(),
            model_label: "iPod Classic".to_owned(),
            drive: ready.to_string_lossy().into_owned(),
            name: None,
            volume_guid: None,
        }]
    );
}

#[test]
fn fail_closes_duplicate_identity_until_the_conflict_clears() {
    let first = PathBuf::from("/Volumes/first");
    let second = PathBuf::from("/Volumes/second");
    let mut scanner = DeviceObservationScanner::new();
    let duplicate = scanner.scan_with([&first, &second], |mount, observation_id| {
        Some(observation(
            mount,
            observation_id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
            Some(0x1261),
            None,
            Some(160_000_000_000),
        ))
    });

    assert!(adapt_observation_inventory(&duplicate).is_empty());

    let cleared = scanner.scan_with([&first], |mount, observation_id| {
        Some(observation(
            mount,
            observation_id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
            Some(0x1261),
            None,
            Some(160_000_000_000),
        ))
    });
    assert_eq!(adapt_observation_inventory(&cleared).len(), 1);
}

#[test]
fn keeps_distinct_devices_sorted_with_generic_and_exact_labels() {
    let later = PathBuf::from("/Volumes/later");
    let earlier = PathBuf::from("/Volumes/earlier");
    let mut scanner = DeviceObservationScanner::new();
    let inventory = scanner.scan_with([&later, &earlier], |mount, observation_id| {
        let (device_id, model_code) = if mount == later {
            ("000A27002138B0AF", None)
        } else {
            ("000A27002138B0A1", Some("MC297"))
        };
        Some(observation(
            mount,
            observation_id,
            Some(device_id),
            DeviceReadiness::Ready,
            Some(0x1261),
            model_code,
            Some(160_000_000_000),
        ))
    });

    let detected = adapt_observation_inventory(&inventory);

    assert_eq!(detected.len(), 2);
    assert_eq!(detected[0].serial, "0x000A27002138B0A1");
    assert_eq!(detected[0].model_label, "iPod Classic (3rd gen)");
    assert_eq!(detected[0].drive, earlier.to_string_lossy());
    assert_eq!(detected[1].serial, "0x000A27002138B0AF");
    assert_eq!(detected[1].model_label, "iPod Classic");
    assert!(!detected[1].model_label.contains("160"));
    assert!(!detected[1].model_label.contains("silver"));
    assert!(!detected[1].model_label.contains("3rd"));
}

#[test]
fn capacity_inference_does_not_become_an_exact_model_label() {
    let mount = PathBuf::from("/Volumes/classic-80");
    let mut scanner = DeviceObservationScanner::new();
    let inventory = scanner.scan_with([&mount], |path, observation_id| {
        Some(observation(
            path,
            observation_id,
            Some("000A27002138B0A8"),
            DeviceReadiness::Ready,
            Some(0x1261),
            None,
            Some(80_000_000_000),
        ))
    });

    assert_eq!(
        adapt_observation_inventory(&inventory)[0].model_label,
        "iPod Classic"
    );
}

#[test]
fn adaptation_does_not_probe_candidates_a_second_time() {
    let first = PathBuf::from("/Volumes/first");
    let second = PathBuf::from("/Volumes/second");
    let calls = Cell::new(0);
    let mut scanner = DeviceObservationScanner::new();
    let inventory = scanner.scan_with([&first, &second], |mount, observation_id| {
        calls.set(calls.get() + 1);
        let identity = if mount == first {
            "000A27002138B0A1"
        } else {
            "000A27002138B0A2"
        };
        Some(observation(
            mount,
            observation_id,
            Some(identity),
            DeviceReadiness::Ready,
            Some(0x1261),
            None,
            Some(160_000_000_000),
        ))
    });

    assert_eq!(calls.get(), 2);
    assert_eq!(adapt_observation_inventory(&inventory).len(), 2);
    assert_eq!(calls.get(), 2);
}

#[test]
fn production_observer_and_v2_adaptation_perform_zero_device_writes() {
    let mount = std::env::temp_dir().join(format!(
        "classick-v2-observation-{}-{}",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
    fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    fs::write(mount.join("iPod_Control/Device/SysInfo"), []).unwrap();
    let before = snapshot(&mount);
    let mut scanner = DeviceObservationScanner::new();

    let inventory = scanner.scan_with([&mount], |path, observation_id| {
        super::discovery::observe_mount_with_probe(path, observation_id, |_| {
            Some(OrdinaryUsbFacts {
                raw_usb_iserial: Some("000A27002138B0A8".to_owned()),
                usb_product_id: Some(0x1261),
                capacity_bytes: Some(160_000_000_000),
            })
        })
    });

    assert_eq!(inventory.observations().len(), 1);
    assert!(adapt_observation_inventory(&inventory).is_empty());
    assert_eq!(snapshot(&mount), before);
    let _ = fs::remove_dir_all(mount);
}

#[test]
fn known_volume_fast_path_checks_layout_without_reparsing_the_database() {
    let mount = std::env::temp_dir().join(format!(
        "classick-known-volume-{}-{}",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
    fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    fs::write(mount.join("iPod_Control/Device/SysInfo"), []).unwrap();
    fs::write(mount.join("iPod_Control/iTunes/iTunesDB"), []).unwrap();
    let previous = DetectedIpod {
        serial: "0x000A27002138B0A8".to_owned(),
        model_label: "iPod Classic".to_owned(),
        drive: "G:\\".to_owned(),
        name: None,
        volume_guid: Some("volume-guid".to_owned()),
    };

    assert!(adapt_known_mount(&mount, "volume-guid", &previous).is_some());

    fs::remove_file(mount.join("iPod_Control/iTunes/iTunesDB")).unwrap();
    fs::create_dir(mount.join("iPod_Control/iTunes/iTunesDB")).unwrap();
    assert!(adapt_known_mount(&mount, "volume-guid", &previous).is_none());
    let _ = fs::remove_dir_all(mount);
}

#[test]
fn production_polling_cache_reuses_an_unchanged_ready_observation() {
    let mount = ready_mount("polling-cache-unchanged");
    let detected = detected_at(&mount, None);
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    let first = cache.scan_with(
        vec![mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );
    let second = cache.scan_with(
        vec![mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );

    assert_eq!(first, vec![detected.clone()]);
    assert_eq!(second, vec![detected]);
    assert_eq!(cold_scans.get(), 1);
    let _ = fs::remove_dir_all(mount);
}

#[test]
fn production_polling_cache_tracks_a_known_volume_mount_change_without_cold_scan() {
    let original = ready_mount("polling-cache-original-mount");
    let moved = original.with_file_name(format!(
        "classick-polling-cache-moved-mount-{}-{}",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let detected = detected_at(&original, Some("volume-guid"));
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    cache.scan_with(
        vec![original.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );
    fs::rename(&original, &moved).unwrap();
    let current = cache.scan_with(
        vec![moved.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            Vec::new()
        },
        |_| Some(moved.clone()),
        |_| Some(usb_facts("000A27002138B0A8")),
    );

    assert_eq!(cold_scans.get(), 1);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].drive, moved.to_string_lossy());
    let _ = fs::remove_dir_all(moved);
}

#[test]
fn production_polling_cache_cold_scans_when_the_database_changes() {
    let mount = ready_mount("polling-cache-database-change");
    let detected = detected_at(&mount, None);
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    cache.scan_with(
        vec![mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );
    fs::write(
        mount.join("iPod_Control/iTunes/iTunesDB"),
        b"changed database bytes",
    )
    .unwrap();
    let current = cache.scan_with(
        vec![mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );

    assert_eq!(cold_scans.get(), 2);
    assert_eq!(current, vec![detected]);
    let _ = fs::remove_dir_all(mount);
}

#[test]
fn production_polling_cache_cold_scans_when_candidate_inventory_changes() {
    let first_mount = ready_mount("polling-cache-first-candidate");
    let second_mount = ready_mount("polling-cache-second-candidate");
    let first = detected_at(&first_mount, None);
    let mut second = detected_at(&second_mount, None);
    second.serial = "0x000A27002138B0A9".to_owned();
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    cache.scan_with(
        vec![first_mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![first.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );
    let current = cache.scan_with(
        vec![first_mount.clone(), second_mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![first.clone(), second.clone()]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );

    assert_eq!(cold_scans.get(), 2);
    assert_eq!(current, vec![first, second]);
    let _ = fs::remove_dir_all(first_mount);
    let _ = fs::remove_dir_all(second_mount);
}

#[test]
fn production_polling_cache_cold_scans_and_drops_a_disconnected_device() {
    let mount = ready_mount("polling-cache-disconnect");
    let detected = detected_at(&mount, None);
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    cache.scan_with(
        vec![mount.clone()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            vec![detected]
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );
    fs::remove_dir_all(&mount).unwrap();
    let current = cache.scan_with(
        Vec::new(),
        || {
            cold_scans.set(cold_scans.get() + 1);
            Vec::new()
        },
        |_| None,
        |_| Some(usb_facts("000A27002138B0A8")),
    );

    assert_eq!(cold_scans.get(), 2);
    assert!(current.is_empty());
}

fn observation(
    mount_path: &Path,
    observation_id: ObservationId,
    raw_usb_iserial: Option<&str>,
    readiness: DeviceReadiness,
    usb_product_id: Option<u16>,
    reported_model_code: Option<&str>,
    capacity_bytes: Option<u64>,
) -> DeviceObservation {
    assemble_device_observation(
        ReportedDeviceObservation {
            mount_path: mount_path.to_path_buf(),
            observation_id,
            raw_usb_iserial: raw_usb_iserial.map(str::to_owned),
            usb_product_id,
            reported_model_code: reported_model_code.map(str::to_owned),
            reported_firmware: None,
            capacity_bytes,
        },
        |_| Some(readiness),
    )
    .unwrap()
}

fn snapshot(root: &Path) -> Vec<(PathBuf, bool, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, bool, Vec<u8>)>) {
        let mut children: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        children.sort();
        for child in children {
            let relative = child.strip_prefix(root).unwrap().to_path_buf();
            if child.is_dir() {
                entries.push((relative, true, Vec::new()));
                visit(root, &child, entries);
            } else {
                entries.push((relative, false, fs::read(child).unwrap()));
            }
        }
    }

    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries
}

fn ready_mount(label: &str) -> PathBuf {
    let mount = std::env::temp_dir().join(format!(
        "classick-{label}-{}-{}",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
    fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    fs::write(mount.join("iPod_Control/Device/SysInfo"), []).unwrap();
    fs::write(mount.join("iPod_Control/iTunes/iTunesDB"), b"database").unwrap();
    mount
}

fn detected_at(mount: &Path, volume_guid: Option<&str>) -> DetectedIpod {
    DetectedIpod {
        serial: "0x000A27002138B0A8".to_owned(),
        model_label: "iPod Classic".to_owned(),
        drive: mount.to_string_lossy().into_owned(),
        name: None,
        volume_guid: volume_guid.map(str::to_owned),
    }
}

fn usb_facts(raw_usb_iserial: &str) -> OrdinaryUsbFacts {
    OrdinaryUsbFacts {
        raw_usb_iserial: Some(raw_usb_iserial.to_owned()),
        usb_product_id: Some(0x1261),
        capacity_bytes: Some(160_000_000_000),
    }
}
