use serde::{Deserialize, Serialize};
use std::path::Path;

mod authority;

use authority::{DatabaseAuthority, Inspection};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceReadiness {
    Ready,
    NeedsAppleInitialization,
    InvalidDatabase,
    IdentityUnavailable,
}

pub fn classify_device_readiness(mount: &Path) -> Option<DeviceReadiness> {
    classify_device_readiness_with(mount, DatabaseAuthority::is_structurally_valid)
}

pub(super) fn classify_device_readiness_with(
    mount: &Path,
    validate_database: impl FnOnce(&DatabaseAuthority) -> bool,
) -> Option<DeviceReadiness> {
    let database = match authority::inspect(mount) {
        Inspection::Unrecognized => return None,
        Inspection::MissingDatabase => return Some(DeviceReadiness::NeedsAppleInitialization),
        Inspection::InvalidDatabase => return Some(DeviceReadiness::InvalidDatabase),
        Inspection::Database(database) => database,
    };

    if validate_database(&database) && database.is_current() {
        Some(DeviceReadiness::Ready)
    } else {
        Some(DeviceReadiness::InvalidDatabase)
    }
}
