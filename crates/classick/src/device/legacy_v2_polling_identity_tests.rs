use super::super::legacy_v2::LegacyV2PollingCache;
use super::super::OrdinaryUsbFacts;
use crate::ipod::device::DetectedIpod;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[test]
fn production_polling_cache_rejects_same_path_replacement_with_different_usb_identity() {
    let mount = ready_mount("same-path-replacement");
    let original = detected_at(&mount);
    let mut replacement = detected_at(&mount);
    replacement.serial = "0x000A27002138B0A9".to_owned();
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    poll_cache(
        &mut cache,
        &mount,
        &cold_scans,
        vec![original],
        Some("000A27002138B0A8"),
    );
    let current = poll_cache(
        &mut cache,
        &mount,
        &cold_scans,
        vec![replacement.clone()],
        Some("000A27002138B0A9"),
    );

    assert_eq!(cold_scans.get(), 2);
    assert_eq!(current, vec![replacement]);
    let _ = fs::remove_dir_all(mount);
}

#[test]
fn production_polling_cache_rejects_cached_identity_when_fresh_usb_identity_is_unavailable() {
    let mount = ready_mount("usb-identity-unavailable");
    let detected = detected_at(&mount);
    let cold_scans = Cell::new(0);
    let mut cache = LegacyV2PollingCache::default();

    poll_cache(
        &mut cache,
        &mount,
        &cold_scans,
        vec![detected],
        Some("000A27002138B0A8"),
    );
    let current = poll_cache(&mut cache, &mount, &cold_scans, Vec::new(), None);

    assert_eq!(cold_scans.get(), 2);
    assert!(current.is_empty());
    let _ = fs::remove_dir_all(mount);
}

fn poll_cache(
    cache: &mut LegacyV2PollingCache,
    mount: &Path,
    cold_scans: &Cell<u32>,
    cold_result: Vec<DetectedIpod>,
    usb_identity: Option<&str>,
) -> Vec<DetectedIpod> {
    cache.scan_with(
        vec![mount.to_path_buf()],
        || {
            cold_scans.set(cold_scans.get() + 1);
            cold_result
        },
        |_| None,
        |_| usb_identity.map(usb_facts),
    )
}

fn ready_mount(label: &str) -> PathBuf {
    let mount = std::env::temp_dir().join(format!(
        "classick-polling-cache-{label}-{}-{}",
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

fn detected_at(mount: &Path) -> DetectedIpod {
    DetectedIpod {
        serial: "0x000A27002138B0A8".to_owned(),
        model_label: "iPod Classic".to_owned(),
        drive: mount.to_string_lossy().into_owned(),
        name: None,
        volume_guid: None,
    }
}

fn usb_facts(raw_usb_iserial: &str) -> OrdinaryUsbFacts {
    OrdinaryUsbFacts {
        raw_usb_iserial: Some(raw_usb_iserial.to_owned()),
        usb_product_id: Some(0x1261),
        capacity_bytes: Some(160_000_000_000),
    }
}
