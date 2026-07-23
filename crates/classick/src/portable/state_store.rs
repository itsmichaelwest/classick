use super::host_cache::{serialize_cache, HostCache, HostCacheLoad, HostCacheStore};
use super::outbox::{
    coalesce_pending, serialize_outbox, OutboxLoad, PendingDeviceOutbox, PendingMutation,
    PendingOutboxStore,
};
use crate::atomic_file::AtomicFileWriter;
use crate::device::DeviceId;
use crate::portable::reconcile::ConditionalOutboxClear;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableHostState {
    pub cache: Option<HostCache>,
    pub outbox: PendingDeviceOutbox,
}

#[derive(Debug, Clone)]
pub struct PortableStateStore {
    root: PathBuf,
    writer: AtomicFileWriter,
}

impl PortableStateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writer: AtomicFileWriter::new(),
        }
    }

    pub fn load(&self, device_id: &DeviceId) -> Result<PortableHostState> {
        let cache = match HostCacheStore::new(&self.root).load(device_id)? {
            HostCacheLoad::Missing => None,
            HostCacheLoad::Loaded(cache) => Some(cache),
        };
        let outbox = match PendingOutboxStore::new(&self.root).load(device_id)? {
            OutboxLoad::Missing(outbox) | OutboxLoad::Loaded(outbox) => outbox,
        };
        Ok(PortableHostState { cache, outbox })
    }

    pub fn is_initialized(&self, device_id: &DeviceId) -> bool {
        HostCacheStore::new(&self.root).path(device_id).exists()
            || PendingOutboxStore::new(&self.root).path(device_id).exists()
    }

    pub fn initialize(
        &self,
        cache: &HostCache,
        outbox: &PendingDeviceOutbox,
    ) -> Result<PortableHostState> {
        cache.validate()?;
        outbox.validate()?;
        if cache.device_id != outbox.device_id {
            anyhow::bail!("portable cache and outbox belong to different devices");
        }

        let current = self.load(&cache.device_id)?;
        if self.is_initialized(&cache.device_id) {
            if current.cache.as_ref() == Some(cache) && current.outbox == *outbox {
                return Ok(current);
            }
            anyhow::bail!("portable host state is already initialized");
        }

        // Publish accepted intent first. If the process stops between these
        // writes, the complete mutation remains replayable and is never lost.
        self.write_outbox(outbox)
            .context("initialize portable host outbox")?;
        self.write_cache(cache)
            .context("initialize portable host cache")?;
        Ok(PortableHostState {
            cache: Some(cache.clone()),
            outbox: outbox.clone(),
        })
    }

    pub fn accept_mutation(&self, mutation: &PendingMutation) -> Result<PortableHostState> {
        self.accept_mutations(std::slice::from_ref(mutation))
    }

    pub fn accept_mutations(&self, mutations: &[PendingMutation]) -> Result<PortableHostState> {
        let first = mutations
            .first()
            .context("portable mutation batch is empty")?;
        let mut state = self.load(first.device_id())?;
        for mutation in mutations {
            state.outbox = coalesce_pending(&state.outbox, mutation)?;
        }
        self.write_outbox(&state.outbox)
            .context("durably accept portable device mutation")?;
        Ok(state)
    }

    pub fn import_device(&self, cache: &HostCache) -> Result<PortableHostState> {
        cache.validate()?;
        let state = self.load(&cache.device_id)?;
        if !state.outbox.mutations.is_empty() {
            anyhow::bail!("cannot import device state while host intent is pending");
        }
        self.write_cache(cache)
            .context("persist imported portable device state")?;
        Ok(PortableHostState {
            cache: Some(cache.clone()),
            outbox: state.outbox,
        })
    }

    pub fn confirm_device_commit(
        &self,
        cache: &HostCache,
        clear: &ConditionalOutboxClear,
    ) -> Result<PortableHostState> {
        cache.validate()?;
        let current = self.load(&cache.device_id)?;
        let cleared = clear.apply_to(&current.outbox)?;

        // Cache first, then clear. A crash between these writes leaves the
        // mutation replayable; the inverse ordering could lose accepted intent.
        self.write_cache(cache)
            .context("persist confirmed portable device state")?;
        self.write_outbox(&cleared)
            .context("clear confirmed portable device mutation")?;
        Ok(PortableHostState {
            cache: Some(cache.clone()),
            outbox: cleared,
        })
    }

    fn write_cache(&self, cache: &HostCache) -> Result<()> {
        let path = HostCacheStore::new(&self.root).path(&cache.device_id);
        ensure_owned_parent(&self.root, &path)?;
        self.writer.write(&path, &serialize_cache(cache)?)
    }

    fn write_outbox(&self, outbox: &PendingDeviceOutbox) -> Result<()> {
        let path = PendingOutboxStore::new(&self.root).path(&outbox.device_id);
        ensure_owned_parent(&self.root, &path)?;
        self.writer.write(&path, &serialize_outbox(outbox)?)
    }
}

fn ensure_owned_parent(root: &Path, target: &Path) -> Result<()> {
    let devices = root.join("devices");
    std::fs::create_dir_all(&devices)
        .with_context(|| format!("create portable devices root {}", devices.display()))?;
    let devices = std::fs::canonicalize(&devices)
        .with_context(|| format!("resolve portable devices root {}", devices.display()))?;
    let expected = devices.join(
        target
            .parent()
            .and_then(Path::file_name)
            .context("portable host-state path has no device directory")?,
    );
    match std::fs::create_dir(&expected) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("create portable device state {}", expected.display()));
        }
    }
    let metadata = std::fs::symlink_metadata(&expected)
        .with_context(|| format!("inspect portable device state {}", expected.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "portable device state parent is not a real directory: {}",
            expected.display()
        );
    }
    let canonical = std::fs::canonicalize(&expected)
        .with_context(|| format!("resolve portable device state {}", expected.display()))?;
    if !canonical.starts_with(&devices) || canonical != expected {
        anyhow::bail!("portable device state path is redirected");
    }
    Ok(())
}
