use super::profile::PortableProfile;
use crate::atomic_file::AtomicFileWriter;
use crate::device::DeviceId;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const HOST_CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCache {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_canonical_device_id")]
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_imported_profile: Option<PortableProfile>,
}

impl HostCache {
    pub fn new(
        device_id: DeviceId,
        last_imported_profile: Option<PortableProfile>,
    ) -> Result<Self> {
        let cache = Self {
            schema_version: HOST_CACHE_SCHEMA_VERSION,
            device_id,
            last_imported_profile,
        };
        cache.validate()?;
        Ok(cache)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != HOST_CACHE_SCHEMA_VERSION {
            bail!("unsupported host cache schema {}", self.schema_version);
        }
        if let Some(profile) = &self.last_imported_profile {
            profile
                .validate()
                .context("validate cached portable profile")?;
            if profile.device_id != self.device_id {
                bail!("cached portable profile device ID does not match host cache");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCacheLoad {
    Missing,
    Loaded(HostCache),
}

#[derive(Debug, Clone)]
pub struct HostCacheStore {
    root: PathBuf,
    writer: AtomicFileWriter,
}

impl HostCacheStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_writer(root, AtomicFileWriter::new())
    }

    #[doc(hidden)]
    pub fn with_writer(root: impl Into<PathBuf>, writer: AtomicFileWriter) -> Self {
        Self {
            root: root.into(),
            writer,
        }
    }

    pub fn path(&self, device_id: &DeviceId) -> PathBuf {
        self.root
            .join("devices")
            .join(device_id.as_str())
            .join("cache.json")
    }

    pub fn load(&self, device_id: &DeviceId) -> Result<HostCacheLoad> {
        let path = self.path(device_id);
        reject_host_symlinks(&self.root, &path)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(HostCacheLoad::Missing),
            Err(error) => {
                return Err(error).with_context(|| format!("read host cache {}", path.display()));
            }
        };
        let cache =
            parse_cache(&bytes).with_context(|| format!("parse host cache {}", path.display()))?;
        if &cache.device_id != device_id {
            bail!("host cache device ID does not match its device directory");
        }
        Ok(HostCacheLoad::Loaded(cache))
    }

    pub fn save(&self, cache: &HostCache) -> Result<HostCache> {
        cache.validate()?;
        let path = self.path(&cache.device_id);
        reject_host_symlinks(&self.root, &path)?;
        let bytes = serialize_cache(cache)?;
        self.writer
            .write(&path, &bytes)
            .with_context(|| format!("save host cache {}", path.display()))?;
        reject_host_symlinks(&self.root, &path)?;
        let durable = fs::read(&path)
            .with_context(|| format!("verify durable host cache {}", path.display()))?;
        if durable != bytes {
            bail!("durable host cache bytes differ from the accepted value");
        }
        let reparsed = parse_cache(&durable).context("reparse durable host cache")?;
        if &reparsed != cache {
            bail!("durable host cache differs after exact reparse");
        }
        Ok(reparsed)
    }
}

fn serialize_cache(cache: &HostCache) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(cache)?;
    bytes.push(b'\n');
    let reparsed = parse_cache(&bytes).context("reparse serialized host cache")?;
    if &reparsed != cache {
        bail!("serialized host cache differs after exact reparse");
    }
    Ok(bytes)
}

fn parse_cache(bytes: &[u8]) -> Result<HostCache> {
    let cache: HostCache = serde_json::from_slice(bytes)?;
    cache.validate()?;
    Ok(cache)
}

pub(super) fn deserialize_canonical_device_id<'de, D>(deserializer: D) -> Result<DeviceId, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let device_id = DeviceId::parse(&value).map_err(serde::de::Error::custom)?;
    if value != device_id.as_str() {
        return Err(serde::de::Error::custom(
            "host-state device ID must use its canonical uppercase spelling",
        ));
    }
    Ok(device_id)
}

pub(super) fn reject_host_symlinks(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .context("host-state path escapes its configured root")?;
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        if index == 0 {
            inspect_host_path(&current, false)?;
        }
        current.push(component);
        inspect_host_path(&current, index + 1 == component_count)?;
    }
    Ok(())
}

fn inspect_host_path(path: &Path, final_component: bool) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect host-state path {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        bail!(
            "host-state path must not traverse a symlink: {}",
            path.display()
        );
    }
    if final_component {
        if !metadata.is_file() {
            bail!("host-state authority is not a file: {}", path.display());
        }
    } else if !metadata.is_dir() {
        bail!("host-state parent is not a directory: {}", path.display());
    }
    Ok(())
}
