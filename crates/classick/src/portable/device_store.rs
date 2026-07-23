use super::profile::PortableProfile;
use super::reconcile::DeviceProfileObservation;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const PROFILE_RELATIVE_PATH: &str = "iPod_Control/classick/profile.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedDeviceProfile {
    Absent,
    Valid(PortableProfile),
    Invalid(String),
}

impl OwnedDeviceProfile {
    pub fn as_observation(&self) -> DeviceProfileObservation<'_> {
        match self {
            Self::Absent => DeviceProfileObservation::Absent,
            Self::Valid(profile) => DeviceProfileObservation::Valid(profile),
            Self::Invalid(diagnostic) => DeviceProfileObservation::Invalid(diagnostic),
        }
    }
}

pub fn profile_path(mount: &Path) -> PathBuf {
    mount.join(PROFILE_RELATIVE_PATH)
}

pub fn read_profile(mount: &Path) -> Result<OwnedDeviceProfile> {
    let path = profile_path(mount);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OwnedDeviceProfile::Absent);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read portable device profile {}", path.display()));
        }
    };
    let json = match std::str::from_utf8(&bytes) {
        Ok(json) => json,
        Err(error) => return Ok(OwnedDeviceProfile::Invalid(error.to_string())),
    };
    match PortableProfile::from_json(json) {
        Ok(profile) => Ok(OwnedDeviceProfile::Valid(profile)),
        Err(error) => Ok(OwnedDeviceProfile::Invalid(format!("{error:#}"))),
    }
}
