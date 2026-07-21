use super::{
    DeviceId, FactConfidence, HardwareFacts, IpodFamily, ObservationInventory, OrdinaryUsbFacts,
};
use crate::ipod::device::{mount_for_volume_guid, volume_guid_for_mount, DetectedIpod};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

pub(crate) fn scan_for_ipods() -> Vec<DetectedIpod> {
    static CACHE: OnceLock<Mutex<LegacyV2PollingCache>> = OnceLock::new();

    let candidates = crate::ipod::device::candidate_mount_points();
    CACHE
        .get_or_init(|| Mutex::new(LegacyV2PollingCache::default()))
        .lock()
        .expect("legacy v2 polling cache poisoned")
        .scan_with(
            candidates,
            || adapt_observation_inventory(&super::scan_device_observations()),
            mount_for_volume_guid,
            crate::ipod::device::ordinary_usb_facts_for_mount,
        )
}

#[derive(Default)]
pub(super) struct LegacyV2PollingCache {
    candidates: BTreeSet<PathBuf>,
    known: Vec<KnownReadyObservation>,
}

struct KnownReadyObservation {
    detected: DetectedIpod,
    fingerprint: ReadyLayoutFingerprint,
}

#[derive(Clone, PartialEq, Eq)]
struct ReadyLayoutFingerprint {
    sysinfo: FileFingerprint,
    database: FileFingerprint,
}

#[derive(Clone, PartialEq, Eq)]
struct FileFingerprint {
    length: u64,
    modified: SystemTime,
}

impl LegacyV2PollingCache {
    pub(super) fn scan_with(
        &mut self,
        candidates: Vec<PathBuf>,
        cold_scan: impl FnOnce() -> Vec<DetectedIpod>,
        mut resolve_volume: impl FnMut(&str) -> Option<PathBuf>,
        mut probe_usb: impl FnMut(&Path) -> Option<OrdinaryUsbFacts>,
    ) -> Vec<DetectedIpod> {
        let candidate_set: BTreeSet<_> = candidates.into_iter().collect();
        if let Some(cached) = self.revalidate(&candidate_set, &mut resolve_volume, &mut probe_usb) {
            return cached;
        }

        let detected = cold_scan();
        self.remember(candidate_set, &detected);
        detected
    }

    fn revalidate(
        &self,
        candidates: &BTreeSet<PathBuf>,
        resolve_volume: &mut impl FnMut(&str) -> Option<PathBuf>,
        probe_usb: &mut impl FnMut(&Path) -> Option<OrdinaryUsbFacts>,
    ) -> Option<Vec<DetectedIpod>> {
        if self.known.is_empty() {
            return None;
        }

        let mut expected_candidates = self.candidates.clone();
        let mut detected = Vec::with_capacity(self.known.len());
        for known in &self.known {
            let mount = match known.detected.volume_guid.as_deref() {
                Some(volume_guid) => resolve_volume(volume_guid)?,
                None => PathBuf::from(&known.detected.drive),
            };
            if ready_layout_fingerprint(&mount)? != known.fingerprint {
                return None;
            }
            let current_device_id = usb_device_id(probe_usb(&mount))?;
            let cached_device_id = DeviceId::parse(&known.detected.serial).ok()?;
            if current_device_id != cached_device_id {
                return None;
            }
            expected_candidates.remove(Path::new(&known.detected.drive));
            expected_candidates.insert(mount.clone());

            let mut current = known.detected.clone();
            current.drive = mount.to_string_lossy().into_owned();
            detected.push(current);
        }

        (*candidates == expected_candidates).then_some(detected)
    }

    fn remember(&mut self, candidates: BTreeSet<PathBuf>, detected: &[DetectedIpod]) {
        self.candidates = candidates;
        self.known = detected
            .iter()
            .filter_map(|detected| {
                let fingerprint = ready_layout_fingerprint(Path::new(&detected.drive))?;
                Some(KnownReadyObservation {
                    detected: detected.clone(),
                    fingerprint,
                })
            })
            .collect();
    }
}

fn usb_device_id(facts: Option<OrdinaryUsbFacts>) -> Option<DeviceId> {
    DeviceId::parse(facts?.raw_usb_iserial.as_deref()?).ok()
}

fn ready_layout_fingerprint(mount: &Path) -> Option<ReadyLayoutFingerprint> {
    Some(ReadyLayoutFingerprint {
        sysinfo: file_fingerprint(&crate::ipod::layout::sysinfo_path(mount))?,
        database: file_fingerprint(&crate::ipod::layout::itunes_db_path(mount))?,
    })
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    Some(FileFingerprint {
        length: metadata.len(),
        modified: metadata.modified().ok()?,
    })
}

pub(crate) fn try_resolve_known_volume(
    volume_guid: &str,
    previous: &DetectedIpod,
) -> Option<DetectedIpod> {
    let mount = mount_for_volume_guid(volume_guid)?;
    adapt_known_mount(&mount, volume_guid, previous)
}

pub(super) fn adapt_known_mount(
    mount: &Path,
    volume_guid: &str,
    previous: &DetectedIpod,
) -> Option<DetectedIpod> {
    if !known_ready_layout_is_present(mount) {
        return None;
    }
    Some(DetectedIpod {
        serial: previous.serial.clone(),
        model_label: previous.model_label.clone(),
        drive: mount.to_string_lossy().into_owned(),
        name: previous.name.clone(),
        volume_guid: Some(volume_guid.to_owned()),
    })
}

fn known_ready_layout_is_present(mount: &Path) -> bool {
    [
        crate::ipod::layout::sysinfo_path(mount),
        crate::ipod::layout::itunes_db_path(mount),
    ]
    .into_iter()
    .all(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    })
}

pub(super) fn adapt_observation_inventory(inventory: &ObservationInventory) -> Vec<DetectedIpod> {
    inventory
        .observations()
        .iter()
        .filter(|observation| inventory.is_uniquely_mutation_eligible(observation))
        .map(|observation| {
            let device_id = observation
                .device_id()
                .expect("mutation-eligible observation must have a device ID");
            DetectedIpod {
                serial: format!("0x{device_id}"),
                model_label: model_label_from_facts(observation.hardware_facts()),
                drive: observation.mount_path().to_string_lossy().into_owned(),
                name: None,
                volume_guid: volume_guid_for_mount(observation.mount_path()),
            }
        })
        .collect()
}

fn model_label_from_facts(facts: &HardwareFacts) -> String {
    let Some(family) = facts.family.as_ref().map(|fact| fact.value) else {
        return "iPod (model unknown)".to_owned();
    };
    let family = match family {
        IpodFamily::Ipod => "iPod",
        IpodFamily::Classic => "iPod Classic",
        IpodFamily::Nano => "iPod Nano",
        IpodFamily::Mini => "iPod Mini",
        IpodFamily::Shuffle => "iPod Shuffle",
        IpodFamily::Photo => "iPod Photo",
        IpodFamily::Video => "iPod Video",
        IpodFamily::Touch => "iPod Touch",
    };

    match facts
        .generation
        .as_ref()
        .filter(|fact| fact.confidence == FactConfidence::Certain)
        .map(|fact| fact.value.as_str())
    {
        Some("1") => format!("{family} (1st gen)"),
        Some("2") => format!("{family} (2nd gen)"),
        Some("3") => format!("{family} (3rd gen)"),
        Some("4") => format!("{family} (4th gen)"),
        Some("5") => format!("{family} (5th gen)"),
        Some("6") => format!("{family} (6th gen)"),
        Some("7") => format!("{family} (7th gen)"),
        Some(generation) => format!("{family} ({generation} gen)"),
        None => family.to_owned(),
    }
}
