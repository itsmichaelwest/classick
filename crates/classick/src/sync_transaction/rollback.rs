use crate::atomic_file::AtomicFileWriter;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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

    pub(crate) fn open_for_deletion(root: &Path) -> Result<Self> {
        require_real_directory(root, "rollback snapshot")?;
        let index_path = root.join("snapshot.json");
        require_regular_file(&index_path, "rollback snapshot index")?;
        let bytes = std::fs::read(&index_path)
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
        snapshot.validate_for_deletion()?;
        Ok(snapshot)
    }

    pub(crate) fn remove_for_deletion(self) -> Result<()> {
        self.validate_for_deletion()?;
        std::fs::remove_dir_all(&self.root)
            .with_context(|| format!("remove rollback snapshot {}", self.root.display()))?;
        let sidecar = appledouble_sibling(&self.root);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("remove rollback snapshot metadata {}", sidecar.display())
            }),
        }
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

    fn validate_for_deletion(&self) -> Result<()> {
        require_real_directory(&self.root, "rollback snapshot")?;
        let index_path = self.root.join("snapshot.json");
        require_regular_file(&index_path, "rollback snapshot index")?;
        if self.index.entries.is_empty() {
            bail!("rollback snapshot index has no entries");
        }

        let mut indexed_files = HashSet::new();
        let mut indexed_directories = HashSet::new();
        for entry in &self.index.entries {
            validate_relative_path(&entry.relative_path)?;
            if !is_allowed_snapshot_path(&entry.relative_path) {
                bail!(
                    "rollback snapshot index contains unsupported path {}",
                    entry.relative_path.display()
                );
            }
            if !indexed_files.insert(entry.relative_path.clone()) {
                bail!(
                    "rollback snapshot index has duplicate path {}",
                    entry.relative_path.display()
                );
            }
            let mut parent = entry.relative_path.parent();
            while let Some(path) = parent {
                if path.as_os_str().is_empty() {
                    break;
                }
                indexed_directories.insert(path.to_path_buf());
                parent = path.parent();
            }
            require_real_directory_chain(&self.root, &entry.relative_path)?;
            let path = self.root.join(&entry.relative_path);
            require_regular_file(&path, "rollback copy")?;
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read rollback copy {}", path.display()))?;
            if blake3::hash(&bytes).to_hex().as_str() != entry.hash {
                bail!("rollback copy {} failed validation", path.display());
            }
        }
        if !indexed_files.contains(Path::new("iPod_Control/iTunes/iTunesDB")) {
            bail!("rollback snapshot index has no iTunesDB entry");
        }

        let mut expected_files = indexed_files;
        expected_files.insert(PathBuf::from("snapshot.json"));
        validate_exact_tree(
            &self.root,
            Path::new(""),
            &expected_files,
            &indexed_directories,
        )
    }
}

fn is_allowed_snapshot_path(path: &Path) -> bool {
    if path == Path::new("iPod_Control/iTunes/iTunesDB") {
        return true;
    }
    path.parent() == Some(Path::new("iPod_Control/Artwork")) && is_managed_artwork_output(path)
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!(
            "rollback snapshot index path is not safe and relative: {}",
            path.display()
        );
    }
    Ok(())
}

fn require_real_directory_chain(root: &Path, relative: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(component) = component else {
                bail!(
                    "rollback snapshot index path is not safe and relative: {}",
                    relative.display()
                );
            };
            current.push(component);
            require_real_directory(&current, "rollback snapshot directory")?;
        }
    }
    Ok(())
}

fn validate_exact_tree(
    root: &Path,
    relative: &Path,
    expected_files: &HashSet<PathBuf>,
    expected_directories: &HashSet<PathBuf>,
) -> Result<()> {
    let directory = root.join(relative);
    let entries = std::fs::read_dir(&directory)
        .with_context(|| format!("read rollback snapshot directory {}", directory.display()))?;
    for entry in entries {
        let entry = entry?;
        let child_relative = relative.join(entry.file_name());
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspect rollback snapshot path {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "rollback snapshot contains redirected path {}",
                path.display()
            );
        }
        if metadata.is_dir() {
            if !expected_directories.contains(&child_relative) {
                bail!(
                    "rollback snapshot contains unindexed directory {}",
                    path.display()
                );
            }
            validate_exact_tree(root, &child_relative, expected_files, expected_directories)?;
        } else if metadata.is_file() {
            if !expected_files.contains(&child_relative)
                && !is_paired_appledouble(&child_relative, expected_files, expected_directories)
            {
                bail!(
                    "rollback snapshot contains unindexed file {}",
                    path.display()
                );
            }
        } else {
            bail!(
                "rollback snapshot contains unsupported path {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn is_paired_appledouble(
    path: &Path,
    expected_files: &HashSet<PathBuf>,
    expected_directories: &HashSet<PathBuf>,
) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(target_name) = file_name.strip_prefix("._") else {
        return false;
    };
    if target_name.is_empty() {
        return false;
    }
    let target = path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(target_name);
    expected_files.contains(&target) || expected_directories.contains(&target)
}

fn appledouble_sibling(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    path.with_file_name(format!("._{}", name.to_string_lossy()))
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => bail!("{label} is not a real directory: {}", path.display()),
        Err(error) => Err(error).with_context(|| format!("inspect {label} {}", path.display())),
    }
}

fn require_regular_file(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => bail!("{label} is not a regular file: {}", path.display()),
        Err(error) => Err(error).with_context(|| format!("inspect {label} {}", path.display())),
    }
}

pub(crate) fn is_managed_artwork_output(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "ArtworkDB" || name.ends_with(".ithmb")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    fn root(label: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "rollback-delete-{label}-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn save_index(root: &Path, entries: Vec<SnapshotEntry>) {
        let index = SnapshotIndex {
            version: 1,
            entries,
        };
        std::fs::write(
            root.join("snapshot.json"),
            serde_json::to_vec_pretty(&index).unwrap(),
        )
        .unwrap();
    }

    fn entry(root: &Path, relative: &Path, contents: &[u8]) -> SnapshotEntry {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        SnapshotEntry {
            relative_path: relative.to_path_buf(),
            hash: blake3::hash(contents).to_hex().to_string(),
        }
    }

    #[test]
    fn deletion_validation_rejects_empty_unsafe_and_duplicate_indexes() {
        let empty = root("empty");
        save_index(&empty, Vec::new());
        assert!(RollbackSnapshot::open_for_deletion(&empty).is_err());

        for unsafe_path in [PathBuf::from("../escape"), PathBuf::from("/absolute")] {
            let unsafe_root = root("unsafe");
            save_index(
                &unsafe_root,
                vec![SnapshotEntry {
                    relative_path: unsafe_path,
                    hash: blake3::hash(b"outside").to_hex().to_string(),
                }],
            );
            assert!(RollbackSnapshot::open_for_deletion(&unsafe_root).is_err());
        }

        let duplicate = root("duplicate");
        let indexed = entry(
            &duplicate,
            Path::new("iPod_Control/iTunes/iTunesDB"),
            b"database",
        );
        save_index(&duplicate, vec![indexed.clone(), indexed]);
        assert!(RollbackSnapshot::open_for_deletion(&duplicate).is_err());
    }

    #[test]
    fn deletion_validation_rejects_safe_index_without_mandatory_database() {
        let root = root("missing-database");
        let indexed = entry(
            &root,
            Path::new("iPod_Control/Artwork/ArtworkDB"),
            b"artwork database",
        );
        save_index(&root, vec![indexed]);

        assert!(RollbackSnapshot::open_for_deletion(&root).is_err());
        assert_eq!(
            std::fs::read(root.join("iPod_Control/Artwork/ArtworkDB")).unwrap(),
            b"artwork database"
        );
    }

    #[test]
    fn deletion_validation_allows_only_snapshot_create_outputs() {
        for relative in [
            Path::new("copy.bin"),
            Path::new("iPod_Control/iTunes/other.bin"),
            Path::new("iPod_Control/Artwork/notes.txt"),
        ] {
            let root = root("unsupported-output");
            let database = entry(
                &root,
                Path::new("iPod_Control/iTunes/iTunesDB"),
                b"database",
            );
            let unsupported = entry(&root, relative, b"unsupported");
            save_index(&root, vec![database, unsupported]);
            assert!(
                RollbackSnapshot::open_for_deletion(&root).is_err(),
                "{}",
                relative.display()
            );
        }

        let valid = root("valid-outputs");
        let database = entry(
            &valid,
            Path::new("iPod_Control/iTunes/iTunesDB"),
            b"database",
        );
        let artwork_db = entry(
            &valid,
            Path::new("iPod_Control/Artwork/ArtworkDB"),
            b"artwork database",
        );
        let thumbnails = entry(
            &valid,
            Path::new("iPod_Control/Artwork/F1069_1.ithmb"),
            b"thumbnails",
        );
        save_index(&valid, vec![database, artwork_db, thumbnails]);
        assert!(RollbackSnapshot::open_for_deletion(&valid).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn deletion_validation_rejects_redirected_indexed_file() {
        let root = root("redirected-file");
        let outside = root.with_file_name("rollback-delete-outside.bin");
        std::fs::write(&outside, b"copy").unwrap();
        let database = root.join("iPod_Control/iTunes/iTunesDB");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &database).unwrap();
        save_index(
            &root,
            vec![SnapshotEntry {
                relative_path: PathBuf::from("iPod_Control/iTunes/iTunesDB"),
                hash: blake3::hash(b"copy").to_hex().to_string(),
            }],
        );

        assert!(RollbackSnapshot::open_for_deletion(&root).is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"copy");
    }
}
