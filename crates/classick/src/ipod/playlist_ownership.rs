use crate::atomic_file::AtomicFileWriter;
use crate::ipod::layout;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANAGED_PLAYLIST_OWNERSHIP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedPlaylistKind {
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RockboxProjectionRecord {
    pub relative_filename: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPlaylistEntry {
    pub apple_playlist_id: u64,
    pub expected_kind: ManagedPlaylistKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rockbox: Option<RockboxProjectionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedPlaylistOwnership {
    pub schema_version: u32,
    pub device_serial: String,
    pub playlists: BTreeMap<String, ManagedPlaylistEntry>,
}

impl ManagedPlaylistOwnership {
    pub fn empty_for_serial(serial: impl Into<String>) -> Self {
        Self {
            schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
            device_serial: serial.into(),
            playlists: BTreeMap::new(),
        }
    }

    pub fn validate_for_serial(&self, serial: &str) -> Result<()> {
        validate_for_serial(self, serial)
    }
}

pub fn validate_for_serial(candidate: &ManagedPlaylistOwnership, serial: &str) -> Result<()> {
    if candidate.schema_version != MANAGED_PLAYLIST_OWNERSHIP_VERSION {
        anyhow::bail!(
            "unsupported playlist ownership schema version {}",
            candidate.schema_version
        );
    }
    if candidate.device_serial != serial {
        anyhow::bail!(
            "playlist ownership serial {:?} does not exactly match connected raw serial {:?}",
            candidate.device_serial,
            serial
        );
    }
    for (slug, entry) in &candidate.playlists {
        if !safe_slug(slug) {
            anyhow::bail!("unsafe managed playlist slug {slug:?}");
        }
        if entry.apple_playlist_id == 0 {
            anyhow::bail!("managed playlist {slug:?} has zero Apple playlist ID");
        }
        if entry.expected_kind != ManagedPlaylistKind::Normal {
            anyhow::bail!("managed playlist {slug:?} is not expected to be normal");
        }
    }
    Ok(())
}

fn safe_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipOrigin {
    Device,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPlaylistOwnership {
    pub value: ManagedPlaylistOwnership,
    pub origin: OwnershipOrigin,
}

#[derive(Debug, Clone)]
pub struct DeviceOwnershipStore {
    mount: PathBuf,
    serial: String,
    host_cache: PathBuf,
    atomic_writer: AtomicFileWriter,
}

impl DeviceOwnershipStore {
    pub fn new(
        mount: PathBuf,
        serial: String,
        host_cache: PathBuf,
        atomic_writer: AtomicFileWriter,
    ) -> Self {
        Self {
            mount,
            serial,
            host_cache,
            atomic_writer,
        }
    }

    pub fn load_device(&self) -> Result<ManagedPlaylistOwnership> {
        Ok(self.load_device_with_origin()?.value)
    }

    pub fn load_device_read_only(&self) -> Result<ManagedPlaylistOwnership> {
        self.load_device()
    }

    pub fn load_device_with_origin(&self) -> Result<LoadedPlaylistOwnership> {
        self.load_device_path(&layout::managed_playlists_path(&self.mount))
    }

    pub fn load_device_read_only_with_origin(&self) -> Result<LoadedPlaylistOwnership> {
        self.load_device_with_origin()
    }

    pub fn publish_device(&self, candidate: &ManagedPlaylistOwnership) -> Result<()> {
        candidate
            .validate_for_serial(&self.serial)
            .context("validate candidate device playlist ownership")?;
        let bytes = encode(candidate)?;
        let path = layout::managed_playlists_path(&self.mount);
        self.atomic_writer
            .write(&path, &bytes)
            .context("publish device playlist ownership")?;
        let published = self
            .load_device_path(&path)
            .context("reparse published device playlist ownership")?;
        if published.origin != OwnershipOrigin::Device || published.value != *candidate {
            anyhow::bail!("published device playlist ownership differs from candidate");
        }
        Ok(())
    }

    pub fn refresh_host_cache(
        &self,
        candidate: &ManagedPlaylistOwnership,
    ) -> Result<Option<String>> {
        candidate
            .validate_for_serial(&self.serial)
            .context("validate host-cache playlist ownership candidate")?;
        let device = self
            .load_device_with_origin()
            .context("verify device playlist ownership before host cache refresh")?;
        if device.origin != OwnershipOrigin::Device || device.value != *candidate {
            anyhow::bail!("candidate playlist ownership is not published device truth");
        }
        let bytes = encode(candidate)?;
        let refresh = self
            .preserve_legacy_host_cache()
            .and_then(|()| {
                self.atomic_writer
                    .write(&self.host_cache, &bytes)
                    .context("refresh host playlist ownership cache")
            })
            .and_then(|()| validate_file(&self.host_cache, &self.serial));
        Ok(refresh.err().map(|error| format!("{error:#}")))
    }

    fn preserve_legacy_host_cache(&self) -> Result<()> {
        let existing = match std::fs::read(&self.host_cache) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "read legacy host playlist cache {}",
                        self.host_cache.display()
                    )
                });
            }
        };
        if decode_and_validate(&existing, &self.serial).is_ok() {
            return Ok(());
        }
        let retained =
            crate::device_state::retained_legacy_managed_playlists_path(&self.host_cache);
        if retained.exists() {
            return Ok(());
        }
        self.atomic_writer
            .write(&retained, &existing)
            .context("retain legacy host playlist cache for diagnostics")
    }

    fn load_device_path(&self, path: &Path) -> Result<LoadedPlaylistOwnership> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedPlaylistOwnership {
                    value: ManagedPlaylistOwnership::empty_for_serial(self.serial.clone()),
                    origin: OwnershipOrigin::Missing,
                });
            }
            Err(error) => {
                return Err(anyhow!(
                    "invalid device playlist ownership at {}: {error}",
                    path.display()
                ));
            }
        };
        let value = decode_and_validate(&bytes, &self.serial).map_err(|error| {
            anyhow!(
                "invalid device playlist ownership at {}: {error:#}",
                path.display()
            )
        })?;
        Ok(LoadedPlaylistOwnership {
            value,
            origin: OwnershipOrigin::Device,
        })
    }
}

fn encode(candidate: &ManagedPlaylistOwnership) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(candidate).context("encode playlist ownership")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_and_validate(bytes: &[u8], serial: &str) -> Result<ManagedPlaylistOwnership> {
    let decoded: ManagedPlaylistOwnership =
        serde_json::from_slice(bytes).context("decode playlist ownership JSON")?;
    decoded.validate_for_serial(serial)?;
    Ok(decoded)
}

fn validate_file(path: &Path, serial: &str) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read published playlist ownership {}", path.display()))?;
    decode_and_validate(&bytes, serial)
        .with_context(|| format!("validate playlist ownership {}", path.display()))?;
    Ok(())
}
