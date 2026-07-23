use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationEntry {
    pub path: String,
    pub length: u64,
    pub blake3: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceGeneration {
    pub entries: Vec<GenerationEntry>,
}

impl PartialEq for DeviceGeneration {
    fn eq(&self, other: &Self) -> bool {
        self.entries
            .iter()
            .filter(|entry| !is_appledouble_path(&entry.path))
            .eq(other
                .entries
                .iter()
                .filter(|entry| !is_appledouble_path(&entry.path)))
    }
}

impl Eq for DeviceGeneration {}

pub(super) fn capture(mount: &Path) -> Result<DeviceGeneration> {
    let mut files = Vec::new();
    collect_tree(
        mount,
        &mount.join("iPod_Control/iTunes"),
        TreePolicy::All,
        &mut files,
    )?;
    collect_tree(
        mount,
        &mount.join("iPod_Control/Artwork"),
        TreePolicy::All,
        &mut files,
    )?;
    collect_tree(
        mount,
        &mount.join("iPod_Control/classick"),
        TreePolicy::Classick,
        &mut files,
    )?;
    collect_tree(
        mount,
        &mount.join("Playlists/Classick"),
        TreePolicy::All,
        &mut files,
    )?;
    collect_file(
        mount,
        &mount.join("iPod_Control/Device/SysInfoExtended"),
        &mut files,
    )?;
    files.sort();
    files.dedup();

    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let relative = file
            .strip_prefix(mount)
            .context("authoritative device path escapes mount")?;
        let metadata = fs::symlink_metadata(&file)
            .with_context(|| format!("inspect authoritative file {}", file.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(
                "authoritative device path is not a regular file: {}",
                file.display()
            );
        }
        entries.push(GenerationEntry {
            path: normalize(relative)?,
            length: metadata.len(),
            blake3: hash_file(&file)?,
        });
    }
    Ok(DeviceGeneration { entries })
}

#[derive(Clone, Copy)]
enum TreePolicy {
    All,
    Classick,
}

fn collect_tree(
    mount: &Path,
    root: &Path,
    policy: TreePolicy,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", root.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "authoritative root is not a real directory: {}",
            root.display()
        );
    }

    let mut children = fs::read_dir(root)
        .with_context(|| format!("enumerate {}", root.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        if child
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("._"))
        {
            continue;
        }
        let path = child.path();
        let relative_to_root = path
            .strip_prefix(root)
            .context("generation child escapes authority root")?;
        if matches!(policy, TreePolicy::Classick) && excluded_classick(relative_to_root) {
            continue;
        }
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("inspect {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("authoritative path is redirected: {}", path.display());
        }
        if metadata.is_dir() {
            collect_tree(mount, &path, policy, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            bail!(
                "authoritative path has unsupported type: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn excluded_classick(relative: &Path) -> bool {
    relative.components().next().is_some_and(|component| {
        let component = component.as_os_str();
        component == "pending" || component == "device.lock"
    })
}

fn is_appledouble_path(path: &str) -> bool {
    path.split('/').any(|component| component.starts_with("._"))
}

fn collect_file(mount: &Path, path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "authoritative device path is not a regular file: {}",
            path.display()
        );
    }
    if !path.starts_with(mount) {
        bail!("authoritative device path escapes mount");
    }
    files.push(path.to_path_buf());
    Ok(())
}

fn normalize(path: &Path) -> Result<String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => {
                components.push(value.to_string_lossy().into_owned())
            }
            _ => bail!("authoritative device path is not relative and normalized"),
        }
    }
    Ok(components.join("/"))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("open authoritative file {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hash authoritative file {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
