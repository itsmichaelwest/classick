use super::{
    hardware_facts_from_reported_model_code, hardware_facts_from_usb, DeviceId, DeviceReadiness,
    Fact, HardwareFacts,
};
use std::path::{Path, PathBuf};

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
