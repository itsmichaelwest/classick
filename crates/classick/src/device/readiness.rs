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

pub(super) struct ReadinessAuthority {
    readiness: DeviceReadiness,
    database: DatabaseAuthority,
}

impl ReadinessAuthority {
    pub(super) fn readiness(&self) -> DeviceReadiness {
        self.readiness
    }

    pub(super) fn read_sysinfo(&self) -> Option<String> {
        self.database.read_sysinfo().ok()
    }

    pub(super) fn is_current(&self) -> bool {
        self.database.is_current()
    }
}

pub fn classify_device_readiness(mount: &Path) -> Option<DeviceReadiness> {
    classify_device_readiness_with(mount, DatabaseAuthority::is_structurally_valid)
}

pub(super) fn inspect_device_readiness(mount: &Path) -> Option<ReadinessAuthority> {
    inspect_device_readiness_with(mount, DatabaseAuthority::is_structurally_valid)
}

pub(super) fn classify_device_readiness_with(
    mount: &Path,
    validate_database: impl FnOnce(&DatabaseAuthority) -> bool,
) -> Option<DeviceReadiness> {
    let authority = inspect_device_readiness_with(mount, validate_database)?;
    if authority.is_current() {
        Some(authority.readiness())
    } else {
        Some(DeviceReadiness::InvalidDatabase)
    }
}

fn inspect_device_readiness_with(
    mount: &Path,
    validate_database: impl FnOnce(&DatabaseAuthority) -> bool,
) -> Option<ReadinessAuthority> {
    let (readiness, database) = match authority::inspect(mount) {
        Inspection::Unrecognized => return None,
        Inspection::MissingDatabase(database) => {
            (DeviceReadiness::NeedsAppleInitialization, database)
        }
        Inspection::InvalidDatabase(database) => (DeviceReadiness::InvalidDatabase, database),
        Inspection::Database(database) => {
            let readiness = if validate_database(&database) && database.is_current() {
                DeviceReadiness::Ready
            } else {
                DeviceReadiness::InvalidDatabase
            };
            (readiness, database)
        }
    };

    Some(ReadinessAuthority {
        readiness,
        database,
    })
}
