mod generation;

pub use generation::{DeviceGeneration, GenerationEntry};

use crate::device::DeviceId;
use anyhow::{Context, Result};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CONTROL_DIRECTORY: &str = "iPod_Control";
const CLASSICK_DIRECTORY: &str = "classick";
const LEASE_FILE: &str = "device.lock";

#[derive(Debug)]
pub enum CoordinationFailure {
    AlreadyLocked,
    UnsafeLeasePath {
        path: PathBuf,
        reason: String,
    },
    Unavailable {
        operation: &'static str,
        source: std::io::Error,
    },
}

impl fmt::Display for CoordinationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyLocked => formatter.write_str(
                "coordination_unavailable: another Classick process is using this device",
            ),
            Self::UnsafeLeasePath { path, reason } => write!(
                formatter,
                "coordination_unavailable: unsafe lease path {}: {reason}",
                path.display()
            ),
            Self::Unavailable { operation, source } => {
                write!(formatter, "coordination_unavailable: {operation}: {source}")
            }
        }
    }
}

impl std::error::Error for CoordinationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum ExternalGenerationChange {
    Changed {
        expected: DeviceGeneration,
        actual: DeviceGeneration,
    },
    Unavailable(String),
}

impl fmt::Display for ExternalGenerationChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Changed { .. } => formatter.write_str(
                "external_generation_changed: device metadata changed outside this Classick session",
            ),
            Self::Unavailable(message) => {
                write!(formatter, "external_generation_changed: {message}")
            }
        }
    }
}

impl std::error::Error for ExternalGenerationChange {}

pub struct DeviceMutationSession {
    device_id: DeviceId,
    mount: PathBuf,
    _lease: DeviceLease,
    expected_generation: Mutex<DeviceGeneration>,
}

impl fmt::Debug for DeviceMutationSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceMutationSession")
            .field("device_id", &self.device_id)
            .field("mount", &self.mount)
            .finish_non_exhaustive()
    }
}

impl DeviceMutationSession {
    pub fn acquire(mount: &Path, device_id: DeviceId) -> Result<Self, CoordinationFailure> {
        let mount = canonical_directory(mount, "resolve device mount")?;
        let control =
            canonical_directory(&mount.join(CONTROL_DIRECTORY), "resolve control directory")?;
        if !control.starts_with(&mount) {
            return Err(unsafe_path(
                &control,
                "control directory escapes the device mount",
            ));
        }

        let classick = control.join(CLASSICK_DIRECTORY);
        ensure_real_directory(&classick)?;
        let canonical_classick =
            canonical_directory(&classick, "resolve Classick device directory")?;
        if !canonical_classick.starts_with(&control) {
            return Err(unsafe_path(
                &classick,
                "Classick directory escapes the control directory",
            ));
        }

        let lease_path = canonical_classick.join(LEASE_FILE);
        let lease = DeviceLease::acquire(&lease_path)?;
        let expected_generation =
            generation::capture(&mount).map_err(|error| CoordinationFailure::Unavailable {
                operation: "capture initial device generation",
                source: io_other(error),
            })?;

        Ok(Self {
            device_id,
            mount,
            _lease: lease,
            expected_generation: Mutex::new(expected_generation),
        })
    }

    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    pub fn mount(&self) -> &Path {
        &self.mount
    }

    pub fn verify_scope(&self, mount: &Path, device_id: &DeviceId) -> Result<()> {
        if device_id != &self.device_id {
            anyhow::bail!("mutation session belongs to another device");
        }
        let canonical = fs::canonicalize(mount).context("resolve mutation mount")?;
        if canonical != self.mount {
            anyhow::bail!("mutation session belongs to another mount");
        }
        Ok(())
    }

    pub fn verify_expected_generation(&self) -> std::result::Result<(), ExternalGenerationChange> {
        let actual = generation::capture(&self.mount)
            .map_err(|error| ExternalGenerationChange::Unavailable(format!("{error:#}")))?;
        let expected = self
            .expected_generation
            .lock()
            .map_err(|_| {
                ExternalGenerationChange::Unavailable(
                    "device generation state is unavailable".to_string(),
                )
            })?
            .clone();
        if actual != expected {
            return Err(ExternalGenerationChange::Changed { expected, actual });
        }
        Ok(())
    }

    pub fn publish_verified<T>(&self, publish: impl FnOnce() -> Result<T>) -> Result<T> {
        self.verify_expected_generation()?;
        let value = publish()?;
        self.accept_verified_generation()?;
        Ok(value)
    }

    pub(crate) fn accept_verified_generation(&self) -> Result<()> {
        let actual = generation::capture(&self.mount)?;
        self.adopt_verified_generation(actual)
    }

    pub(crate) fn capture_current_generation(&self) -> Result<DeviceGeneration> {
        generation::capture(&self.mount)
    }

    pub fn current_generation(&self) -> Result<DeviceGeneration> {
        self.capture_current_generation()
    }

    pub(crate) fn adopt_verified_generation(&self, generation: DeviceGeneration) -> Result<()> {
        *self
            .expected_generation
            .lock()
            .map_err(|_| anyhow::anyhow!("device generation state is unavailable"))? = generation;
        Ok(())
    }
}

fn canonical_directory(
    path: &Path,
    operation: &'static str,
) -> Result<PathBuf, CoordinationFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| CoordinationFailure::Unavailable { operation, source })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_path(path, "expected a real directory"));
    }
    fs::canonicalize(path).map_err(|source| CoordinationFailure::Unavailable { operation, source })
}

fn ensure_real_directory(path: &Path) -> Result<(), CoordinationFailure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(unsafe_path(path, "expected a real directory"));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure_real_directory(path)
            }
            Err(source) => Err(CoordinationFailure::Unavailable {
                operation: "create Classick device directory",
                source,
            }),
        },
        Err(source) => Err(CoordinationFailure::Unavailable {
            operation: "inspect Classick device directory",
            source,
        }),
    }
}

fn unsafe_path(path: &Path, reason: impl Into<String>) -> CoordinationFailure {
    CoordinationFailure::UnsafeLeasePath {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn io_other(error: anyhow::Error) -> std::io::Error {
    std::io::Error::other(format!("{error:#}"))
}

struct DeviceLease {
    file: File,
}

impl DeviceLease {
    fn acquire(path: &Path) -> Result<Self, CoordinationFailure> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(unsafe_path(path, "lease is not a regular file"));
            }
        }

        let file = open_lease_file(path)?;
        let metadata = file
            .metadata()
            .map_err(|source| CoordinationFailure::Unavailable {
                operation: "inspect opened device lease",
                source,
            })?;
        if !metadata.is_file() {
            return Err(unsafe_path(path, "opened lease is not a regular file"));
        }
        lock_file(&file)?;
        Ok(Self { file })
    }
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn open_lease_file(path: &Path) -> Result<File, CoordinationFailure> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| CoordinationFailure::Unavailable {
            operation: "open device lease",
            source,
        })
}

#[cfg(windows)]
fn open_lease_file(path: &Path) -> Result<File, CoordinationFailure> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|source| CoordinationFailure::Unavailable {
            operation: "open device lease",
            source,
        })
}

#[cfg(unix)]
fn lock_file(file: &File) -> Result<(), CoordinationFailure> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let source = std::io::Error::last_os_error();
    let code = source.raw_os_error();
    if code == Some(libc::EWOULDBLOCK) || code == Some(libc::EAGAIN) {
        Err(CoordinationFailure::AlreadyLocked)
    } else {
        Err(CoordinationFailure::Unavailable {
            operation: "lock device lease",
            source,
        })
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) {
    use std::os::fd::AsRawFd;
    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(windows)]
fn lock_file(file: &File) -> Result<(), CoordinationFailure> {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if result != 0 {
        return Ok(());
    }
    let source = std::io::Error::last_os_error();
    if source.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Err(CoordinationFailure::AlreadyLocked)
    } else {
        Err(CoordinationFailure::Unavailable {
            operation: "lock device lease",
            source,
        })
    }
}

#[cfg(windows)]
fn unlock_file(file: &File) {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { zeroed() };
    let _ = unsafe { UnlockFileEx(file.as_raw_handle() as HANDLE, 0, 1, 0, &mut overlapped) };
}
