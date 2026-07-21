pub mod catalogue;
pub mod discovery;
pub mod facts;
pub mod id;
pub mod inventory;
pub(crate) mod legacy_v2;
pub mod readiness;

pub use catalogue::{
    hardware_facts_from_decoded_model_code, hardware_facts_from_reported_model_code,
    hardware_facts_from_usb, HARDWARE_CATALOGUE_VERSION,
};
pub use discovery::{
    assemble_device_observation, observe_mount, DeviceObservation, DeviceObservationIdentity,
    ObservationId, OrdinaryUsbFacts, ReportedDeviceObservation,
};
pub use facts::{Fact, FactConfidence, FactSource, HardwareFacts, IpodColour, IpodFamily};
pub use id::DeviceId;
pub(crate) use inventory::scan_device_observations;
pub use inventory::{DeviceObservationScanner, ObservationInventory};
pub use readiness::{classify_device_readiness, DeviceReadiness};

#[cfg(test)]
mod catalogue_tests;
#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod inventory_tests;
#[cfg(test)]
mod legacy_v2_tests;
#[cfg(test)]
mod observation_tests;
#[cfg(test)]
mod readiness_tests;
#[cfg(test)]
mod tests;
