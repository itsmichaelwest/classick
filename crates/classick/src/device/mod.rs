pub mod catalogue;
pub mod facts;
pub mod id;
pub mod readiness;

pub use catalogue::{
    hardware_facts_from_decoded_model_code, hardware_facts_from_reported_model_code,
    hardware_facts_from_usb, HARDWARE_CATALOGUE_VERSION,
};
pub use facts::{Fact, FactConfidence, FactSource, HardwareFacts, IpodColour, IpodFamily};
pub use id::DeviceId;
pub use readiness::{classify_device_readiness, DeviceReadiness};

#[cfg(test)]
mod catalogue_tests;
#[cfg(test)]
mod readiness_tests;
#[cfg(test)]
mod tests;
