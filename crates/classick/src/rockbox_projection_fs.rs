use crate::rockbox_playlist::validate_recorded_filename;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "macos")]
#[path = "rockbox_projection_fs/macos.rs"]
mod platform;
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "rockbox_projection_fs/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "rockbox_projection_fs/windows.rs"]
mod platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    Missing,
    RecordedFile,
    ForeignFile,
}

pub trait ProjectionIo {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState>;

    fn write_durable(
        &self,
        name: &str,
        bytes: &[u8],
        authorized: &HashSet<String>,
        replace_recorded: bool,
    ) -> Result<()>;

    fn remove_recorded(&self, name: &str, authorized: &HashSet<String>) -> Result<bool>;
}

pub struct DeviceProjectionFs {
    mount: PathBuf,
    fail_before_rename: Option<String>,
}

impl DeviceProjectionFs {
    pub fn new(mount: PathBuf) -> Self {
        Self {
            mount,
            fail_before_rename: None,
        }
    }

    #[doc(hidden)]
    pub fn failing_before_rename(mount: PathBuf, name: String) -> Self {
        Self {
            mount,
            fail_before_rename: Some(name),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.mount.join("Playlists").join("Classick")
    }

    pub fn validate_managed_root(&self) -> Result<PathBuf> {
        let mount_metadata = fs::symlink_metadata(&self.mount)
            .with_context(|| format!("inspect projection mount {}", self.mount.display()))?;
        require_plain_directory(&self.mount, &mount_metadata)?;
        let canonical_mount = self
            .mount
            .canonicalize()
            .with_context(|| format!("canonicalize projection mount {}", self.mount.display()))?;

        let mut current = self.mount.clone();
        for component in ["Playlists", "Classick"] {
            current.push(component);
            ensure_plain_directory(&current)?;
        }

        let canonical_root = current
            .canonicalize()
            .with_context(|| format!("canonicalize managed playlist root {}", current.display()))?;
        if !canonical_root.starts_with(&canonical_mount) || canonical_root == canonical_mount {
            bail!(
                "managed playlist root {} escapes mount {}",
                canonical_root.display(),
                canonical_mount.display()
            );
        }
        Ok(current)
    }

    fn require_authorized<'a>(
        &self,
        name: &'a str,
        authorized: &HashSet<String>,
    ) -> Result<&'a str> {
        validate_recorded_filename(name)?;
        if !authorized.contains(name) {
            bail!("Rockbox projection {name:?} is not recorded as authorized");
        }
        Ok(name)
    }
}

impl ProjectionIo for DeviceProjectionFs {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState> {
        validate_recorded_filename(name)?;
        let root = self.validate_managed_root()?;
        let target = root.join(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if authorized.contains(name) && is_plain_regular_file(&metadata) {
                    Ok(TargetState::RecordedFile)
                } else {
                    Ok(TargetState::ForeignFile)
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TargetState::Missing),
            Err(error) => Err(error)
                .with_context(|| format!("inspect Rockbox projection {}", target.display())),
        }
    }

    fn write_durable(
        &self,
        name: &str,
        bytes: &[u8],
        authorized: &HashSet<String>,
        replace_recorded: bool,
    ) -> Result<()> {
        self.require_authorized(name, authorized)?;
        let root = self.validate_managed_root()?;
        let target = root.join(name);
        validate_write_target(&target, replace_recorded)?;
        let (temporary, mut file) = create_unique_temporary(&root, name)?;
        let result = (|| {
            file.write_all(bytes)
                .with_context(|| format!("write projection temp {}", temporary.display()))?;
            file.sync_all()
                .with_context(|| format!("sync projection temp {}", temporary.display()))?;
            drop(file);

            self.validate_managed_root()?;
            validate_write_target(&target, replace_recorded)?;
            if self.fail_before_rename.as_deref() == Some(name) {
                bail!("injected failure before projection rename for {name:?}");
            }
            platform::rename_atomic(&temporary, &target, replace_recorded).with_context(|| {
                format!(
                    "publish Rockbox projection {} from {}",
                    target.display(),
                    temporary.display()
                )
            })?;
            sync_directory(&root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn remove_recorded(&self, name: &str, authorized: &HashSet<String>) -> Result<bool> {
        self.require_authorized(name, authorized)?;
        let root = self.validate_managed_root()?;
        let target = root.join(name);
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect recorded projection {}", target.display()));
            }
            Ok(metadata) if is_plain_regular_file(&metadata) => {}
            Ok(_) => bail!(
                "recorded projection target {} is not a regular non-link file",
                target.display()
            ),
        }
        fs::remove_file(&target)
            .with_context(|| format!("remove recorded projection {}", target.display()))?;
        sync_directory(&root)?;
        Ok(true)
    }
}

fn ensure_plain_directory(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("create managed directory {}", path.display()));
                }
            }
            fs::symlink_metadata(path)
                .with_context(|| format!("reinspect managed directory {}", path.display()))?
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect managed directory {}", path.display()));
        }
    };
    require_plain_directory(path, &metadata)
}

fn require_plain_directory(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_link_or_reparse(metadata) || !metadata.file_type().is_dir() {
        bail!(
            "managed path {} is not a regular non-link directory",
            path.display()
        );
    }
    Ok(())
}

fn is_plain_regular_file(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_file() && !is_link_or_reparse(metadata)
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_write_target(target: &Path, replace_recorded: bool) -> Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if replace_recorded && is_plain_regular_file(&metadata) => Ok(()),
        Ok(_) if replace_recorded => bail!(
            "replacement target {} is not a regular non-link file",
            target.display()
        ),
        Ok(_) => bail!("projection target {} already exists", target.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !replace_recorded => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(
            "recorded replacement target {} does not exist",
            target.display()
        ),
        Err(error) => {
            Err(error).with_context(|| format!("inspect projection target {}", target.display()))
        }
    }
}

fn create_unique_temporary(root: &Path, name: &str) -> Result<(PathBuf, File)> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = root.join(format!(
            ".{name}.classick-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create unique projection temp {}", temporary.display())
                });
            }
        }
    }
    bail!("could not allocate a unique projection temp for {name:?}")
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open managed directory {} for sync", path.display()))?
        .sync_all()
        .with_context(|| format!("sync managed directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "rockbox_projection_fs/extra_tests.rs"]
mod extra_tests;

#[cfg(test)]
mod tests {
    use super::{DeviceProjectionFs, ProjectionIo, TargetState};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "rockbox-projection-fs-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixture() -> DeviceProjectionFs {
        let fs = DeviceProjectionFs::new(temp_dir("mount"));
        fs.validate_managed_root().unwrap();
        fs
    }

    #[test]
    fn durable_write_has_exact_bytes_and_leaves_no_temp() {
        let fs = fixture();
        let authorized = HashSet::from(["Gym--0123456789.m3u8".to_string()]);

        fs.write_durable("Gym--0123456789.m3u8", b"/a\n", &authorized, false)
            .unwrap();

        assert_eq!(
            std::fs::read(fs.root().join("Gym--0123456789.m3u8")).unwrap(),
            b"/a\n"
        );
        assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".classick-")));
    }

    #[test]
    fn foreign_collision_is_classified_and_never_replaced() {
        let fs = fixture();
        let name = "Mix--0123456789.m3u8";
        std::fs::write(fs.root().join(name), b"foreign").unwrap();

        assert_eq!(
            fs.target_state(name, &HashSet::new()).unwrap(),
            TargetState::ForeignFile
        );
        let authorized = HashSet::from([name.to_string()]);
        assert!(fs
            .write_durable(name, b"classick", &authorized, false)
            .is_err());
        assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"foreign");
        assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".classick-")));
    }

    #[test]
    fn replacement_requires_an_existing_authorized_regular_file() {
        let fs = fixture();
        let name = "Mix--0123456789.m3u8";
        let authorized = HashSet::from([name.to_string()]);

        assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
        std::fs::write(fs.root().join(name), b"old").unwrap();
        assert_eq!(
            fs.target_state(name, &authorized).unwrap(),
            TargetState::RecordedFile
        );
        fs.write_durable(name, b"new", &authorized, true).unwrap();
        assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"new");
    }

    #[test]
    fn injected_publish_failure_preserves_old_file_and_removes_temp() {
        let mount = temp_dir("injected-failure");
        let name = "Mix--0123456789.m3u8";
        let fs = DeviceProjectionFs::failing_before_rename(mount, name.to_string());
        fs.validate_managed_root().unwrap();
        std::fs::write(fs.root().join(name), b"old").unwrap();
        let authorized = HashSet::from([name.to_string()]);

        assert!(fs.write_durable(name, b"new", &authorized, true).is_err());
        assert_eq!(std::fs::read(fs.root().join(name)).unwrap(), b"old");
        assert!(std::fs::read_dir(fs.root()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".classick-")));
    }

    #[test]
    fn authorization_and_filename_validation_fail_closed() {
        let fs = fixture();
        let name = "Mix--0123456789.m3u8";
        let none = HashSet::new();

        assert!(fs.write_durable(name, b"x", &none, false).is_err());
        assert!(fs.remove_recorded(name, &none).is_err());
        for invalid in ["", "..", "../foreign.m3u8", "a/b.m3u8", "a\\b.m3u8"] {
            let authorized = HashSet::from([invalid.to_string()]);
            assert!(fs.write_durable(invalid, b"x", &authorized, false).is_err());
            assert!(fs.remove_recorded(invalid, &authorized).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_rejected_without_touching_escape() {
        let outside = temp_dir("outside-root");
        let mount = temp_dir("symlink-root");
        std::fs::create_dir_all(mount.join("Playlists")).unwrap();
        std::os::unix::fs::symlink(&outside, mount.join("Playlists/Classick")).unwrap();
        let fs = DeviceProjectionFs::new(mount);
        let authorized = HashSet::from(["x.m3u8".to_string()]);

        assert!(fs
            .write_durable("x.m3u8", b"owned", &authorized, false)
            .is_err());
        assert!(!outside.join("x.m3u8").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_intermediate_directory_is_rejected() {
        let outside = temp_dir("outside-intermediate");
        let mount = temp_dir("symlink-intermediate");
        std::os::unix::fs::symlink(&outside, mount.join("Playlists")).unwrap();
        let fs = DeviceProjectionFs::new(mount);

        assert!(fs.validate_managed_root().is_err());
        assert!(!outside.join("Classick").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_foreign_and_escape_is_untouched() {
        let fs = fixture();
        let outside = temp_dir("outside-target").join("foreign.m3u8");
        std::fs::write(&outside, b"foreign").unwrap();
        let name = "Mix--0123456789.m3u8";
        std::os::unix::fs::symlink(&outside, fs.root().join(name)).unwrap();
        let authorized = HashSet::from([name.to_string()]);

        assert_eq!(
            fs.target_state(name, &authorized).unwrap(),
            TargetState::ForeignFile
        );
        assert!(fs
            .write_durable(name, b"classick", &authorized, true)
            .is_err());
        assert!(fs.remove_recorded(name, &authorized).is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"foreign");
    }

    #[test]
    fn recorded_delete_is_idempotent_and_preserves_foreign_files() {
        let fs = fixture();
        let owned = "Gym--0123456789.m3u8";
        let foreign = "Foreign--9876543210.m3u8";
        std::fs::write(fs.root().join(owned), b"owned").unwrap();
        std::fs::write(fs.root().join(foreign), b"foreign").unwrap();
        let authorized = HashSet::from([owned.to_string()]);

        assert!(fs.remove_recorded(owned, &authorized).unwrap());
        assert!(!fs.remove_recorded(owned, &authorized).unwrap());
        assert_eq!(std::fs::read(fs.root().join(foreign)).unwrap(), b"foreign");
    }
}
