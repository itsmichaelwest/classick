use super::{FactConfidence, HardwareFacts, IpodFamily, ObservationInventory};
use crate::ipod::device::{mount_for_volume_guid, volume_guid_for_mount, DetectedIpod};
use std::path::Path;

pub(crate) fn scan_for_ipods() -> Vec<DetectedIpod> {
    adapt_observation_inventory(&super::scan_device_observations())
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
