use crate::rockbox_playlist::validate_recorded_filename;
use anyhow::{bail, Context, Result};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
#[path = "rockbox_projection_fs/unix_common.rs"]
mod unix_common;

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

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionFailurePoint {
    Write,
    Rename,
    Delete,
}

fn injected_failures() -> &'static Mutex<HashMap<PathBuf, ProjectionFailurePoint>> {
    static FAILURES: OnceLock<Mutex<HashMap<PathBuf, ProjectionFailurePoint>>> = OnceLock::new();
    FAILURES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(unix)]
enum MutationSwap {
    ManagedRoot { outside: PathBuf },
    Target { name: String, outside: PathBuf },
}

#[cfg(unix)]
fn injected_mutation_swaps() -> &'static Mutex<HashMap<PathBuf, MutationSwap>> {
    static SWAPS: OnceLock<Mutex<HashMap<PathBuf, MutationSwap>>> = OnceLock::new();
    SWAPS.get_or_init(|| Mutex::new(HashMap::new()))
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

    fn content_matches(
        &self,
        _name: &str,
        _expected_hash: &str,
        _authorized: &HashSet<String>,
    ) -> Result<bool> {
        bail!("projection content verification is unavailable")
    }
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

    #[doc(hidden)]
    pub fn fail_once_for_mount(mount: PathBuf, point: ProjectionFailurePoint) {
        injected_failures().lock().unwrap().insert(mount, point);
    }

    #[cfg(unix)]
    #[doc(hidden)]
    pub fn swap_managed_root_before_mutation_once(mount: PathBuf, outside: PathBuf) {
        injected_mutation_swaps()
            .lock()
            .unwrap()
            .insert(mount, MutationSwap::ManagedRoot { outside });
    }

    #[cfg(unix)]
    #[doc(hidden)]
    pub fn swap_target_before_mutation_once(mount: PathBuf, name: String, outside: PathBuf) {
        injected_mutation_swaps()
            .lock()
            .unwrap()
            .insert(mount, MutationSwap::Target { name, outside });
    }

    pub fn root(&self) -> PathBuf {
        self.mount.join("Playlists").join("Classick")
    }

    pub fn validate_managed_root(&self) -> Result<PathBuf> {
        let directory = self.open_managed_directory()?;
        directory
            .ensure_path_identity()
            .with_context(|| format!("validate managed playlist root {}", self.root().display()))?;
        Ok(self.root())
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

    fn inject(&self, point: ProjectionFailurePoint) -> Result<()> {
        let mut failures = injected_failures().lock().unwrap();
        if failures.get(&self.mount) != Some(&point) {
            return Ok(());
        }
        failures.remove(&self.mount);
        Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            format!("injected Rockbox projection failure at {point:?}"),
        )
        .into())
    }

    fn open_managed_directory(&self) -> Result<platform::ManagedDirectory> {
        platform::ManagedDirectory::open_or_create(&self.mount).with_context(|| {
            format!(
                "open managed playlist root {} without following links",
                self.root().display()
            )
        })
    }

    fn open_existing_managed_directory(&self) -> std::io::Result<platform::ManagedDirectory> {
        platform::ManagedDirectory::open_existing(&self.mount)
    }

    #[cfg(unix)]
    fn run_mutation_swap(&self) -> Result<()> {
        let Some(swap) = injected_mutation_swaps()
            .lock()
            .unwrap()
            .remove(&self.mount)
        else {
            return Ok(());
        };
        let root = self.root();
        match swap {
            MutationSwap::ManagedRoot { outside } => {
                static SEQUENCE: AtomicU64 = AtomicU64::new(0);
                let moved = self.mount.join("Playlists").join(format!(
                    ".Classick-swapped-{}-{}",
                    std::process::id(),
                    SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::rename(&root, &moved)?;
                std::os::unix::fs::symlink(outside, root)?;
            }
            MutationSwap::Target { name, outside } => {
                std::fs::remove_file(root.join(&name))?;
                std::fs::hard_link(outside, root.join(name))?;
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn run_mutation_swap(&self) -> Result<()> {
        Ok(())
    }
}

impl ProjectionIo for DeviceProjectionFs {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState> {
        validate_recorded_filename(name)?;
        let directory = match self.open_existing_managed_directory() {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TargetState::Missing);
            }
            Err(error) => return Err(error).context("open existing managed playlist root"),
        };
        directory.ensure_path_identity()?;
        match directory.entry_kind(name)? {
            platform::EntryKind::Missing => Ok(TargetState::Missing),
            platform::EntryKind::Regular
                if authorized.contains(name) && directory.has_exact_entry(name)? =>
            {
                Ok(TargetState::RecordedFile)
            }
            platform::EntryKind::Regular | platform::EntryKind::Other => {
                Ok(TargetState::ForeignFile)
            }
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
        let directory = self.open_managed_directory()?;
        directory.ensure_path_identity()?;
        validate_write_target(&directory, name, replace_recorded)?;
        let (temporary, mut file) = create_unique_temporary(&directory, name)?;
        let cleanup_temporary = Cell::new(true);
        let result = (|| {
            self.inject(ProjectionFailurePoint::Write)?;
            file.write_all(bytes)
                .with_context(|| format!("write projection temp {temporary:?}"))?;
            file.sync_all()
                .with_context(|| format!("sync projection temp {temporary:?}"))?;
            drop(file);

            directory.ensure_path_identity().with_context(|| {
                format!("revalidate managed root before publishing projection {name:?}")
            })?;
            validate_write_target(&directory, name, replace_recorded)?;
            #[cfg(unix)]
            let expected_target = if replace_recorded {
                Some(directory.entry_identity(name)?.ok_or_else(|| {
                    anyhow::anyhow!("recorded replacement target {name:?} disappeared")
                })?)
            } else {
                None
            };
            self.run_mutation_swap()?;
            directory.ensure_path_identity().with_context(|| {
                format!("confirm managed root before publishing projection {name:?}")
            })?;
            #[cfg(not(unix))]
            validate_write_target(&directory, name, replace_recorded)?;
            self.inject(ProjectionFailurePoint::Rename)?;
            if self.fail_before_rename.as_deref() == Some(name) {
                bail!("injected failure before projection rename for {name:?}");
            }
            #[cfg(unix)]
            if let Some(expected_target) = expected_target {
                cleanup_temporary.set(false);
                directory
                    .replace_if_identity(&temporary, name, expected_target)
                    .with_context(|| {
                        format!("publish Rockbox projection {name:?} from {temporary:?}")
                    })?;
            } else {
                directory
                    .rename_atomic(&temporary, name, false)
                    .with_context(|| {
                        format!("publish Rockbox projection {name:?} from {temporary:?}")
                    })?;
            }
            #[cfg(not(unix))]
            directory
                .rename_atomic(&temporary, name, replace_recorded)
                .with_context(|| {
                    format!("publish Rockbox projection {name:?} from {temporary:?}")
                })?;
            directory
                .sync()
                .context("sync managed projection directory")?;
            directory.ensure_path_identity().with_context(|| {
                format!("revalidate managed root after publishing projection {name:?}")
            })?;
            Ok(())
        })();
        if result.is_err() && cleanup_temporary.get() {
            let _ = directory.remove_file(&temporary);
        }
        result
    }

    fn remove_recorded(&self, name: &str, authorized: &HashSet<String>) -> Result<bool> {
        self.require_authorized(name, authorized)?;
        let directory = match self.open_existing_managed_directory() {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("open existing managed playlist root"),
        };
        directory.ensure_path_identity()?;
        match recorded_entry_state(&directory, name)? {
            platform::EntryKind::Missing => return Ok(false),
            platform::EntryKind::Regular => {}
            platform::EntryKind::Other => {
                bail!("recorded projection target {name:?} is not an exact regular file")
            }
        }
        directory.ensure_path_identity().with_context(|| {
            format!("revalidate managed root before deleting projection {name:?}")
        })?;
        if recorded_entry_state(&directory, name)? != platform::EntryKind::Regular {
            bail!("recorded projection target {name:?} changed before deletion");
        }
        #[cfg(unix)]
        let expected_target = directory
            .entry_identity(name)?
            .ok_or_else(|| anyhow::anyhow!("recorded projection target {name:?} disappeared"))?;
        self.run_mutation_swap()?;
        directory
            .ensure_path_identity()
            .with_context(|| format!("confirm managed root before deleting projection {name:?}"))?;
        #[cfg(not(unix))]
        if recorded_entry_state(&directory, name)? != platform::EntryKind::Regular {
            bail!("recorded projection target {name:?} changed before deletion");
        }
        self.inject(ProjectionFailurePoint::Delete)?;
        #[cfg(unix)]
        remove_identity_bound(&directory, name, expected_target)
            .with_context(|| format!("remove recorded projection {name:?}"))?;
        #[cfg(not(unix))]
        directory
            .remove_file(name)
            .with_context(|| format!("remove recorded projection {name:?}"))?;
        directory
            .sync()
            .context("sync managed projection directory")?;
        directory.ensure_path_identity().with_context(|| {
            format!("revalidate managed root after deleting projection {name:?}")
        })?;
        Ok(true)
    }

    fn content_matches(
        &self,
        name: &str,
        expected_hash: &str,
        authorized: &HashSet<String>,
    ) -> Result<bool> {
        self.require_authorized(name, authorized)?;
        let directory = self
            .open_existing_managed_directory()
            .context("open existing managed playlist root")?;
        directory.ensure_path_identity()?;
        if recorded_entry_state(&directory, name)? != platform::EntryKind::Regular {
            bail!("recorded projection target {name:?} is not an exact regular file");
        }
        let bytes = directory
            .read(name)
            .with_context(|| format!("read recorded projection {name:?}"))?;
        Ok(blake3::hash(&bytes).to_hex().as_str() == expected_hash)
    }
}

fn validate_write_target(
    directory: &platform::ManagedDirectory,
    name: &str,
    replace_recorded: bool,
) -> Result<()> {
    match recorded_entry_state(directory, name)? {
        platform::EntryKind::Regular if replace_recorded => Ok(()),
        platform::EntryKind::Missing if !replace_recorded => Ok(()),
        platform::EntryKind::Missing => {
            bail!("recorded replacement target {name:?} does not exist")
        }
        platform::EntryKind::Regular | platform::EntryKind::Other if replace_recorded => {
            bail!("replacement target {name:?} is not an exact regular file")
        }
        platform::EntryKind::Regular | platform::EntryKind::Other => {
            bail!("projection target {name:?} already exists")
        }
    }
}

fn recorded_entry_state(
    directory: &platform::ManagedDirectory,
    name: &str,
) -> Result<platform::EntryKind> {
    let kind = directory.entry_kind(name)?;
    if kind == platform::EntryKind::Regular && !directory.has_exact_entry(name)? {
        return Ok(platform::EntryKind::Other);
    }
    Ok(kind)
}

fn create_unique_temporary(
    directory: &platform::ManagedDirectory,
    name: &str,
) -> Result<(String, File)> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = format!(".{name}.classick-{}-{sequence}.tmp", std::process::id());
        match directory.create_new(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create unique projection temp {temporary:?}"));
            }
        }
    }
    bail!("could not allocate a unique projection temp for {name:?}")
}

#[cfg(unix)]
fn remove_identity_bound(
    directory: &platform::ManagedDirectory,
    name: &str,
    expected: platform::EntryIdentity,
) -> Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let quarantine = format!(
            ".{name}.classick-delete-{}-{sequence}.tmp",
            std::process::id()
        );
        match directory.remove_if_identity(name, &quarantine, expected) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not allocate a unique deletion quarantine for {name:?}")
}

#[cfg(test)]
#[path = "rockbox_projection_fs/extra_tests.rs"]
mod extra_tests;

#[cfg(test)]
#[path = "rockbox_projection_fs/tests.rs"]
mod tests;
