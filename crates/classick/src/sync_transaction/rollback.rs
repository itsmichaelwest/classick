use crate::atomic_file::AtomicFileWriter;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePoint {
    ArtworkPreparation,
    DatabaseWrite,
    DatabaseVerification,
    DeviceManifest,
    HostCache,
}

impl FailurePoint {
    pub fn requires_rollback(self) -> bool {
        !matches!(self, Self::HostCache)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotEntry {
    relative_path: PathBuf,
    hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotIndex {
    version: u32,
    entries: Vec<SnapshotEntry>,
}

#[derive(Debug, Clone)]
pub struct RollbackSnapshot {
    root: PathBuf,
    index: SnapshotIndex,
}

impl RollbackSnapshot {
    pub fn create(mount: &Path, root: &Path) -> Result<Self> {
        if root.exists() {
            std::fs::remove_dir_all(root)
                .with_context(|| format!("replace rollback snapshot {}", root.display()))?;
        }
        std::fs::create_dir_all(root)
            .with_context(|| format!("create rollback snapshot {}", root.display()))?;

        let mut sources = Vec::new();
        let db = crate::ipod::layout::itunes_db_path(mount);
        if !db.is_file() {
            bail!("cannot snapshot missing iTunesDB {}", db.display());
        }
        sources.push(db);
        let artwork_dir = mount.join("iPod_Control").join("Artwork");
        if artwork_dir.is_dir() {
            for entry in std::fs::read_dir(&artwork_dir)
                .with_context(|| format!("read artwork directory {}", artwork_dir.display()))?
            {
                let path = entry?.path();
                if is_managed_artwork_output(&path) {
                    sources.push(path);
                }
            }
        }
        sources.sort();

        let mut entries = Vec::with_capacity(sources.len());
        for source in sources {
            let relative = source
                .strip_prefix(mount)
                .with_context(|| format!("snapshot path escaped mount: {}", source.display()))?
                .to_path_buf();
            let bytes = std::fs::read(&source)
                .with_context(|| format!("read rollback input {}", source.display()))?;
            let destination = root.join(&relative);
            AtomicFileWriter::new()
                .write(&destination, &bytes)
                .with_context(|| format!("write rollback copy {}", destination.display()))?;
            entries.push(SnapshotEntry {
                relative_path: relative,
                hash: blake3::hash(&bytes).to_hex().to_string(),
            });
        }
        let index = SnapshotIndex {
            version: 1,
            entries,
        };
        AtomicFileWriter::new().write(
            &root.join("snapshot.json"),
            &serde_json::to_vec_pretty(&index).context("encode rollback index")?,
        )?;
        let snapshot = Self {
            root: root.to_path_buf(),
            index,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let bytes = std::fs::read(root.join("snapshot.json"))
            .with_context(|| format!("read rollback index {}", root.display()))?;
        let index: SnapshotIndex =
            serde_json::from_slice(&bytes).context("decode rollback index")?;
        if index.version != 1 {
            bail!("unsupported rollback snapshot version {}", index.version);
        }
        let snapshot = Self {
            root: root.to_path_buf(),
            index,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<()> {
        for entry in &self.index.entries {
            let path = self.root.join(&entry.relative_path);
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read rollback copy {}", path.display()))?;
            if blake3::hash(&bytes).to_hex().as_str() != entry.hash {
                bail!("rollback copy {} failed validation", path.display());
            }
        }
        Ok(())
    }

    pub fn restore(&self, mount: &Path) -> Result<()> {
        self.validate()?;
        let artwork_dir = mount.join("iPod_Control").join("Artwork");
        if artwork_dir.is_dir() {
            for entry in std::fs::read_dir(&artwork_dir)? {
                let path = entry?.path();
                if is_managed_artwork_output(&path) {
                    std::fs::remove_file(&path).with_context(|| {
                        format!("remove failed artwork output {}", path.display())
                    })?;
                }
            }
        }
        for entry in &self.index.entries {
            let source = self.root.join(&entry.relative_path);
            let destination = mount.join(&entry.relative_path);
            let bytes = std::fs::read(&source)?;
            AtomicFileWriter::new()
                .write(&destination, &bytes)
                .with_context(|| format!("restore rollback copy {}", destination.display()))?;
        }
        Ok(())
    }
}

pub(crate) fn is_managed_artwork_output(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "ArtworkDB" || name.ends_with(".ithmb")
}
