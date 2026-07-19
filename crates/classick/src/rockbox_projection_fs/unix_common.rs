use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    Missing,
    Regular,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EntryIdentity {
    device: u64,
    inode: u64,
}

pub(super) struct ManagedDirectory {
    root: PathBuf,
    directory: File,
    device: u64,
    inode: u64,
}

impl ManagedDirectory {
    pub(super) fn open_existing(mount: &Path) -> io::Result<Self> {
        let mount_directory = open_path_directory(mount)?;
        let playlists = open_child(mount_directory.as_raw_fd(), "Playlists")?;
        let directory = open_child(playlists.as_raw_fd(), "Classick")?;
        Self::from_directory(mount, directory)
    }

    pub(super) fn open_or_create(mount: &Path) -> io::Result<Self> {
        let mount_directory = open_path_directory(mount)?;
        let playlists = open_or_create_child(mount_directory.as_raw_fd(), "Playlists")?;
        let directory = open_or_create_child(playlists.as_raw_fd(), "Classick")?;
        Self::from_directory(mount, directory)
    }

    fn from_directory(mount: &Path, directory: File) -> io::Result<Self> {
        let root = mount.join("Playlists").join("Classick");
        let metadata = directory.metadata()?;
        if !metadata.file_type().is_dir() {
            return Err(io::Error::other(
                "managed projection root is not a directory",
            ));
        }
        Ok(Self {
            root,
            device: metadata.dev(),
            inode: metadata.ino(),
            directory,
        })
    }

    pub(super) fn ensure_path_identity(&self) -> io::Result<()> {
        let metadata = fs::symlink_metadata(&self.root)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_dir()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(io::Error::other(
                "managed projection root changed after validation",
            ));
        }
        Ok(())
    }

    pub(super) fn entry_kind(&self, name: &str) -> io::Result<EntryKind> {
        let name = c_string(name.as_bytes(), "projection filename")?;
        let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
        let result = unsafe {
            libc::fstatat(
                self.raw_fd(),
                name.as_ptr(),
                &mut status,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Ok(if status.st_mode & libc::S_IFMT == libc::S_IFREG {
                EntryKind::Regular
            } else {
                EntryKind::Other
            });
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(EntryKind::Missing)
        } else {
            Err(error)
        }
    }

    pub(super) fn has_exact_entry(&self, name: &str) -> io::Result<bool> {
        let current = c_string(b".", "current directory name")?;
        let scan_fd = unsafe {
            libc::openat(
                self.raw_fd(),
                current.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if scan_fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let directory = unsafe { libc::fdopendir(scan_fd) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            unsafe { libc::close(scan_fd) };
            return Err(error);
        }
        let expected = name.as_bytes();
        let mut found = false;
        loop {
            let entry = unsafe { libc::readdir(directory) };
            if entry.is_null() {
                break;
            }
            let actual = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if actual == expected {
                found = true;
                break;
            }
        }
        let close_result = unsafe { libc::closedir(directory) };
        if close_result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(found)
    }

    pub(super) fn entry_identity(&self, name: &str) -> io::Result<Option<EntryIdentity>> {
        let name = c_string(name.as_bytes(), "projection filename")?;
        let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
        let result = unsafe {
            libc::fstatat(
                self.raw_fd(),
                name.as_ptr(),
                &mut status,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Ok(Some(EntryIdentity {
                device: status.st_dev as u64,
                inode: status.st_ino as u64,
            }));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        }
    }

    pub(super) fn create_new(&self, name: &str) -> io::Result<File> {
        let name = c_string(name.as_bytes(), "temporary projection filename")?;
        let fd = unsafe {
            libc::openat(
                self.raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    pub(super) fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        let name = c_string(name.as_bytes(), "projection filename")?;
        let fd = unsafe {
            libc::openat(
                self.raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn rename_atomic(
        &self,
        source: &str,
        destination: &str,
        replace: bool,
    ) -> io::Result<()> {
        super::platform::rename_atomic_at(self.raw_fd(), source, destination, replace)
    }

    pub(super) fn replace_if_identity(
        &self,
        source: &str,
        destination: &str,
        expected_destination: EntryIdentity,
    ) -> io::Result<()> {
        let source_identity = self.entry_identity(source)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "projection temp disappeared")
        })?;
        super::platform::exchange_atomic_at(self.raw_fd(), source, destination)?;

        let verification: io::Result<bool> = (|| {
            Ok(self.entry_identity(source)? == Some(expected_destination)
                && self.has_exact_entry(destination)?
                && self.entry_identity(destination)? == Some(source_identity))
        })();
        match verification {
            Ok(true) => self.remove_file(source),
            Ok(false) => Err(self.rollback_exchange_error(
                source,
                destination,
                "replacement target changed after authorization",
            )),
            Err(error) => Err(self.rollback_exchange_error(
                source,
                destination,
                &format!("verify exchanged replacement target: {error}"),
            )),
        }
    }

    pub(super) fn remove_if_identity(
        &self,
        name: &str,
        quarantine: &str,
        expected: EntryIdentity,
    ) -> io::Result<()> {
        self.rename_atomic(name, quarantine, false)?;
        let verification = self.entry_identity(quarantine);
        if !matches!(verification, Ok(Some(actual)) if actual == expected) {
            let rollback = self.rename_atomic(quarantine, name, false);
            let reason = match verification {
                Ok(_) => "deletion target changed after authorization".to_string(),
                Err(error) => format!("verify quarantined deletion target: {error}"),
            };
            return Err(match rollback {
                Ok(()) => io::Error::other(reason),
                Err(error) => io::Error::other(format!("{reason}; rollback failed: {error}")),
            });
        }
        self.remove_file(quarantine)
    }

    pub(super) fn remove_file(&self, name: &str) -> io::Result<()> {
        let name = c_string(name.as_bytes(), "projection filename")?;
        let result = unsafe { libc::unlinkat(self.raw_fd(), name.as_ptr(), 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.directory.sync_all()
    }

    pub(super) fn raw_fd(&self) -> RawFd {
        self.directory.as_raw_fd()
    }

    fn rollback_exchange_error(&self, source: &str, destination: &str, message: &str) -> io::Error {
        match super::platform::exchange_atomic_at(self.raw_fd(), source, destination) {
            Ok(()) => io::Error::other(message),
            Err(error) => io::Error::other(format!("{message}; rollback failed: {error}")),
        }
    }
}

fn open_path_directory(path: &Path) -> io::Result<File> {
    let path = c_string(path.as_os_str().as_bytes(), "directory path")?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn open_or_create_child(parent: RawFd, name: &str) -> io::Result<File> {
    match open_child(parent, name) {
        Ok(directory) => return Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let name = c_string(name.as_bytes(), "managed directory name")?;
    let created = unsafe { libc::mkdirat(parent, name.as_ptr(), 0o755) };
    if created == -1 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    open_child_c(parent, &name)
}

fn open_child(parent: RawFd, name: &str) -> io::Result<File> {
    let name = c_string(name.as_bytes(), "managed directory name")?;
    open_child_c(parent, &name)
}

fn open_child_c(parent: RawFd, name: &CString) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn c_string(bytes: &[u8], label: &str) -> io::Result<CString> {
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{label} contains NUL")))
}
