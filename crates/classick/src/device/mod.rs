pub mod facts;
pub mod id;

pub use facts::{Fact, FactConfidence, FactSource, HardwareFacts, IpodColour, IpodFamily};
pub use id::DeviceId;

#[cfg(test)]
mod tests;
