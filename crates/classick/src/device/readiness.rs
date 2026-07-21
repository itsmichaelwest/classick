use crate::ipod::{layout, OwnedDb};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceReadiness {
    Ready,
    NeedsAppleInitialization,
    InvalidDatabase,
    IdentityUnavailable,
}

pub fn classify_device_readiness(mount: &Path) -> Option<DeviceReadiness> {
    classify_device_readiness_with(mount, |mount| match OwnedDb::open(mount) {
        Ok(database) => {
            drop(database);
            true
        }
        Err(_) => false,
    })
}

pub(super) fn classify_device_readiness_with(
    mount: &Path,
    validate_database: impl FnOnce(&Path) -> bool,
) -> Option<DeviceReadiness> {
    if !is_recognizable_layout(mount) {
        return None;
    }

    match fs::symlink_metadata(layout::itunes_db_path(mount)) {
        Ok(metadata) if !metadata.file_type().is_file() => Some(DeviceReadiness::InvalidDatabase),
        Ok(_) if validate_database(mount) => Some(DeviceReadiness::Ready),
        Ok(_) => Some(DeviceReadiness::InvalidDatabase),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Some(DeviceReadiness::NeedsAppleInitialization)
        }
        Err(_) => Some(DeviceReadiness::InvalidDatabase),
    }
}

fn is_recognizable_layout(mount: &Path) -> bool {
    let control = mount.join(layout::IPOD_CONTROL);
    let itunes = control.join(layout::ITUNES);

    is_directory(&control) && is_regular_file(&layout::sysinfo_path(mount)) && is_directory(&itunes)
}

fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}
