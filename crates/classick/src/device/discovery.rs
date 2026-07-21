use super::{
    classify_device_readiness, hardware_facts_from_reported_model_code, hardware_facts_from_usb,
    DeviceId, DeviceReadiness, Fact, HardwareFacts,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrdinaryUsbFacts {
    pub raw_usb_iserial: Option<String>,
    pub usb_product_id: Option<u16>,
    pub capacity_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationId(u64);

impl ObservationId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReportedDeviceObservation {
    pub mount_path: PathBuf,
    pub observation_id: ObservationId,
    pub raw_usb_iserial: Option<String>,
    pub usb_product_id: Option<u16>,
    pub reported_model_code: Option<String>,
    pub reported_firmware: Option<String>,
    pub capacity_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceObservationIdentity {
    Identified(DeviceId),
    Unavailable(ObservationId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceObservation {
    identity: DeviceObservationIdentity,
    mount_path: PathBuf,
    readiness: DeviceReadiness,
    hardware_facts: HardwareFacts,
}

impl DeviceObservation {
    pub fn identity(&self) -> &DeviceObservationIdentity {
        &self.identity
    }

    pub fn device_id(&self) -> Option<&DeviceId> {
        match &self.identity {
            DeviceObservationIdentity::Identified(device_id) => Some(device_id),
            DeviceObservationIdentity::Unavailable(_) => None,
        }
    }

    pub fn observation_id(&self) -> Option<&ObservationId> {
        match &self.identity {
            DeviceObservationIdentity::Identified(_) => None,
            DeviceObservationIdentity::Unavailable(observation_id) => Some(observation_id),
        }
    }

    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }

    pub fn readiness(&self) -> DeviceReadiness {
        self.readiness
    }

    pub fn hardware_facts(&self) -> &HardwareFacts {
        &self.hardware_facts
    }

    pub fn is_mutation_eligible(&self) -> bool {
        matches!(
            (&self.identity, self.readiness),
            (
                DeviceObservationIdentity::Identified(_),
                DeviceReadiness::Ready
            )
        )
    }
}

pub fn assemble_device_observation(
    reported: ReportedDeviceObservation,
    classify_readiness: impl FnOnce(&Path) -> Option<DeviceReadiness>,
) -> Option<DeviceObservation> {
    let ReportedDeviceObservation {
        mount_path,
        observation_id,
        raw_usb_iserial,
        usb_product_id,
        reported_model_code,
        reported_firmware,
        capacity_bytes,
    } = reported;

    let classified_readiness = classify_readiness(&mount_path)?;
    if classified_readiness == DeviceReadiness::IdentityUnavailable {
        return None;
    }
    let identity = raw_usb_iserial
        .as_deref()
        .and_then(|value| DeviceId::parse(value).ok())
        .map(DeviceObservationIdentity::Identified)
        .unwrap_or(DeviceObservationIdentity::Unavailable(observation_id));
    let readiness = match identity {
        DeviceObservationIdentity::Identified(_) => classified_readiness,
        DeviceObservationIdentity::Unavailable(_) => DeviceReadiness::IdentityUnavailable,
    };

    Some(DeviceObservation {
        identity,
        mount_path,
        readiness,
        hardware_facts: assemble_hardware_facts(
            usb_product_id,
            reported_model_code.as_deref(),
            reported_firmware,
            capacity_bytes,
        ),
    })
}

pub fn observe_mount(
    mount_path: &Path,
    observation_id: ObservationId,
) -> Option<DeviceObservation> {
    observe_mount_with_probe(
        mount_path,
        observation_id,
        crate::ipod::device::ordinary_usb_facts_for_mount,
    )
}

pub(super) fn observe_mount_with_probe(
    mount_path: &Path,
    observation_id: ObservationId,
    probe: impl FnOnce(&Path) -> Option<OrdinaryUsbFacts>,
) -> Option<DeviceObservation> {
    let readiness = classify_device_readiness(mount_path)?;
    let usb = probe(mount_path).unwrap_or_default();
    let sysinfo = read_existing_sysinfo_facts(mount_path);

    assemble_device_observation(
        ReportedDeviceObservation {
            mount_path: mount_path.to_path_buf(),
            observation_id,
            raw_usb_iserial: usb.raw_usb_iserial,
            usb_product_id: usb.usb_product_id,
            reported_model_code: sysinfo.model_code,
            reported_firmware: sysinfo.firmware,
            capacity_bytes: usb.capacity_bytes,
        },
        |_| Some(readiness),
    )
}

#[derive(Default)]
struct ExistingSysInfoFacts {
    model_code: Option<String>,
    firmware: Option<String>,
}

fn read_existing_sysinfo_facts(mount_path: &Path) -> ExistingSysInfoFacts {
    let path = crate::ipod::layout::sysinfo_path(mount_path);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return ExistingSysInfoFacts::default();
    };
    if !metadata.file_type().is_file() {
        return ExistingSysInfoFacts::default();
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return ExistingSysInfoFacts::default();
    };

    ExistingSysInfoFacts {
        model_code: flat_sysinfo_field(&contents, "ModelNumStr"),
        firmware: flat_sysinfo_field(&contents, "FirmwareVersion"),
    }
}

fn flat_sysinfo_field(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key)
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn assemble_hardware_facts(
    usb_product_id: Option<u16>,
    reported_model_code: Option<&str>,
    reported_firmware: Option<String>,
    capacity_bytes: Option<u64>,
) -> HardwareFacts {
    let exact_model = reported_model_code.and_then(hardware_facts_from_reported_model_code);
    let mut facts = exact_model
        .or_else(|| usb_product_id.map(|pid| hardware_facts_from_usb(pid, capacity_bytes)))
        .unwrap_or_default();

    facts.firmware = reported_firmware
        .filter(|value| !value.is_empty())
        .map(Fact::reported);
    facts.capacity_bytes = capacity_bytes.map(Fact::reported);
    facts
}
