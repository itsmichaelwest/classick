use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, MoveFileExW, BY_HANDLE_FILE_INFORMATION,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    Missing,
    Regular,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    volume_serial: u32,
    file_index_high: u32,
    file_index_low: u32,
}

pub(super) struct ManagedDirectory {
    root: PathBuf,
    mount_directory: File,
    playlists_directory: File,
    directory: File,
    identity: DirectoryIdentity,
}

// Windows exposes these leaf operations through paths, not the held directory handle.
// The held mount/parent/root handles deny delete sharing, so an attacker cannot redirect
// the managed tree outside the validated root. A concurrent same-name leaf replacement
// can still race replacement, deletion, or content reads between validation and the path
// syscall; callers revalidate around each operation, but that narrow race is not atomic.

impl ManagedDirectory {
    pub(super) fn open_existing(mount: &Path) -> io::Result<Self> {
        let mount_directory = open_directory(mount)?;
        let playlists = mount.join("Playlists");
        let playlists_directory = open_directory(&playlists)?;
        let root = playlists.join("Classick");
        let directory = open_directory(&root)?;
        Self::from_directories(mount, root, mount_directory, playlists_directory, directory)
    }

    pub(super) fn open_or_create(mount: &Path) -> io::Result<Self> {
        let mount_directory = open_directory(mount)?;
        let playlists = mount.join("Playlists");
        create_directory_if_missing(&playlists)?;
        let playlists_directory = open_directory(&playlists)?;
        let root = playlists.join("Classick");
        create_directory_if_missing(&root)?;
        let directory = open_directory(&root)?;
        Self::from_directories(mount, root, mount_directory, playlists_directory, directory)
    }

    fn from_directories(
        mount: &Path,
        root: PathBuf,
        mount_directory: File,
        playlists_directory: File,
        directory: File,
    ) -> io::Result<Self> {
        let (mount_identity, mount_attributes) = identity(&mount_directory)?;
        if mount_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("projection mount is a reparse point"));
        }
        let (_, playlists_attributes) = identity(&playlists_directory)?;
        if playlists_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("Playlists directory is a reparse point"));
        }
        let (root_identity, attributes) = identity(&directory)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other(
                "managed projection root is a reparse point",
            ));
        }
        let managed = Self {
            root,
            mount_directory,
            playlists_directory,
            directory,
            identity: root_identity,
        };
        let (current_mount_identity, current_mount_attributes) = identity(&open_directory(mount)?)?;
        if current_mount_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || current_mount_identity != mount_identity
        {
            return Err(io::Error::other(
                "projection mount changed during validation",
            ));
        }
        Ok(managed)
    }

    pub(super) fn ensure_path_identity(&self) -> io::Result<()> {
        let current = open_directory(&self.root)?;
        let (identity, attributes) = identity(&current)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || identity != self.identity {
            return Err(io::Error::other(
                "managed projection root changed after validation",
            ));
        }
        Ok(())
    }

    pub(super) fn entry_kind(&self, name: &str) -> io::Result<EntryKind> {
        match fs::symlink_metadata(self.root.join(name)) {
            Ok(metadata) => Ok(
                if metadata.file_type().is_file()
                    && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
                {
                    EntryKind::Regular
                } else {
                    EntryKind::Other
                },
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(EntryKind::Missing),
            Err(error) => Err(error),
        }
    }

    pub(super) fn has_exact_entry(&self, name: &str) -> io::Result<bool> {
        for entry in fs::read_dir(&self.root)? {
            if entry?.file_name() == OsStr::new(name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn create_new(&self, name: &str) -> io::Result<File> {
        self.ensure_path_identity()?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.root.join(name))
    }

    pub(super) fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        self.ensure_path_identity()?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.root.join(name))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.ensure_path_identity()?;
        Ok(bytes)
    }

    pub(super) fn rename_atomic(
        &self,
        source: &str,
        destination: &str,
        replace: bool,
    ) -> io::Result<()> {
        self.ensure_path_identity()?;
        let source = self.root.join(source);
        let destination = self.root.join(destination);
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let flags = MOVEFILE_WRITE_THROUGH
            | if replace {
                MOVEFILE_REPLACE_EXISTING
            } else {
                0
            };

        // MoveFileExW preserves atomic no-replace publication, while the held ancestor
        // handles enforce the tree boundary described above.
        let moved = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
        if moved == 0 {
            return Err(io::Error::last_os_error());
        }
        self.ensure_path_identity()
    }

    pub(super) fn remove_file(&self, name: &str) -> io::Result<()> {
        self.ensure_path_identity()?;
        fs::remove_file(self.root.join(name))?;
        self.ensure_path_identity()
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        let _ = (
            &self.mount_directory,
            &self.playlists_directory,
            &self.directory,
        );
        Ok(())
    }
}

fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn create_directory_if_missing(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn identity(file: &File) -> io::Result<(DirectoryIdentity, u32)> {
    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let result = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        DirectoryIdentity {
            volume_serial: info.dwVolumeSerialNumber,
            file_index_high: info.nFileIndexHigh,
            file_index_low: info.nFileIndexLow,
        },
        info.dwFileAttributes,
    ))
}
