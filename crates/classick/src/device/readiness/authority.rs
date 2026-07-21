//! Keeps layout recognition and structural parsing bound to one filesystem
//! authority. Unix parses through the opened descriptor; Windows denies write
//! and delete sharing while libgpod reopens the locked path.

use crate::ipod::OwnedDb;
use std::path::Path;

pub(super) enum Inspection {
    Unrecognized,
    MissingDatabase,
    InvalidDatabase,
    Database(DatabaseAuthority),
}

pub(crate) struct DatabaseAuthority(platform::DatabaseAuthority);

impl DatabaseAuthority {
    pub(crate) fn is_structurally_valid(&self) -> bool {
        self.0.is_structurally_valid()
    }

    pub(super) fn is_current(&self) -> bool {
        self.0.is_current()
    }
}

pub(super) fn inspect(mount: &Path) -> Inspection {
    platform::inspect(mount)
}

#[cfg(unix)]
mod platform {
    use super::{DatabaseAuthority as PublicAuthority, Inspection, OwnedDb};
    use std::ffi::CString;
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    pub(super) struct DatabaseAuthority {
        mount_path: PathBuf,
        mount: File,
        control: File,
        device: File,
        sysinfo: File,
        itunes: File,
        database: File,
    }

    impl DatabaseAuthority {
        pub(super) fn is_structurally_valid(&self) -> bool {
            OwnedDb::parse_from_file_handle(&self.database, &self.mount_path).is_ok()
        }

        pub(super) fn is_current(&self) -> bool {
            let Ok(mount) = open_path_directory(&self.mount_path) else {
                return false;
            };
            if !same_file(&mount, &self.mount) {
                return false;
            }
            let Ok(control) = open_child_directory(mount.as_raw_fd(), "iPod_Control") else {
                return false;
            };
            if !same_file(&control, &self.control) {
                return false;
            }
            let Ok(device) = open_child_directory(control.as_raw_fd(), "Device") else {
                return false;
            };
            if !same_file(&device, &self.device) {
                return false;
            }
            let Ok(sysinfo) = open_child_file(device.as_raw_fd(), "SysInfo") else {
                return false;
            };
            if !is_regular_file(&sysinfo) || !same_file(&sysinfo, &self.sysinfo) {
                return false;
            }
            let Ok(itunes) = open_child_directory(control.as_raw_fd(), "iTunes") else {
                return false;
            };
            if !same_file(&itunes, &self.itunes) {
                return false;
            }
            let Ok(database) = open_child_file(itunes.as_raw_fd(), "iTunesDB") else {
                return false;
            };
            is_regular_file(&database) && same_file(&database, &self.database)
        }
    }

    pub(super) fn inspect(mount_path: &Path) -> Inspection {
        let Ok(mount) = open_path_directory(mount_path) else {
            return Inspection::Unrecognized;
        };
        let Ok(control) = open_child_directory(mount.as_raw_fd(), "iPod_Control") else {
            return Inspection::Unrecognized;
        };
        let Ok(device) = open_child_directory(control.as_raw_fd(), "Device") else {
            return Inspection::Unrecognized;
        };
        let Ok(sysinfo) = open_child_file(device.as_raw_fd(), "SysInfo") else {
            return Inspection::Unrecognized;
        };
        if !is_regular_file(&sysinfo) {
            return Inspection::Unrecognized;
        }
        let Ok(itunes) = open_child_directory(control.as_raw_fd(), "iTunes") else {
            return Inspection::Unrecognized;
        };
        let database = match open_child_file(itunes.as_raw_fd(), "iTunesDB") {
            Ok(database) => database,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Inspection::MissingDatabase;
            }
            Err(_) => return Inspection::InvalidDatabase,
        };
        if !is_regular_file(&database) {
            return Inspection::InvalidDatabase;
        }

        Inspection::Database(PublicAuthority(DatabaseAuthority {
            mount_path: mount_path.to_path_buf(),
            mount,
            control,
            device,
            sysinfo,
            itunes,
            database,
        }))
    }

    fn open_path_directory(path: &Path) -> io::Result<File> {
        let path = c_string(path.as_os_str().as_bytes(), "mount path")?;
        open_directory(libc::AT_FDCWD, &path)
    }

    fn open_child_directory(parent: RawFd, name: &str) -> io::Result<File> {
        let name = c_string(name.as_bytes(), "directory name")?;
        open_directory(parent, &name)
    }

    fn open_directory(parent: RawFd, path: &CString) -> io::Result<File> {
        let descriptor = unsafe {
            libc::openat(
                parent,
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        file_from_descriptor(descriptor)
    }

    fn open_child_file(parent: RawFd, name: &str) -> io::Result<File> {
        let name = c_string(name.as_bytes(), "file name")?;
        let descriptor = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        file_from_descriptor(descriptor)
    }

    fn file_from_descriptor(descriptor: RawFd) -> io::Result<File> {
        if descriptor == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn is_regular_file(file: &File) -> bool {
        file.metadata()
            .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    fn same_file(left: &File, right: &File) -> bool {
        let (Ok(left), Ok(right)) = (left.metadata(), right.metadata()) else {
            return false;
        };
        left.dev() == right.dev() && left.ino() == right.ino()
    }

    fn c_string(bytes: &[u8], label: &str) -> io::Result<CString> {
        CString::new(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{label} contains NUL"))
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::{DatabaseAuthority as PublicAuthority, Inspection, OwnedDb};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Identity {
        volume_serial: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    pub(super) struct DatabaseAuthority {
        mount_path: PathBuf,
        mount: File,
        control: File,
        device: File,
        sysinfo: File,
        itunes: File,
        database: File,
    }

    impl DatabaseAuthority {
        pub(super) fn is_structurally_valid(&self) -> bool {
            OwnedDb::open(&self.mount_path).is_ok()
        }

        pub(super) fn is_current(&self) -> bool {
            let control_path = self.mount_path.join("iPod_Control");
            let device_path = control_path.join("Device");
            let itunes_path = control_path.join("iTunes");
            same_directory(&self.mount_path, &self.mount)
                && same_directory(&control_path, &self.control)
                && same_directory(&device_path, &self.device)
                && same_regular_file(&device_path.join("SysInfo"), &self.sysinfo)
                && same_directory(&itunes_path, &self.itunes)
                && same_regular_file(&itunes_path.join("iTunesDB"), &self.database)
        }
    }

    pub(super) fn inspect(mount_path: &Path) -> Inspection {
        let Ok(mount) = open_directory(mount_path) else {
            return Inspection::Unrecognized;
        };
        let control_path = mount_path.join("iPod_Control");
        let Ok(control) = open_directory(&control_path) else {
            return Inspection::Unrecognized;
        };
        let device_path = control_path.join("Device");
        let Ok(device) = open_directory(&device_path) else {
            return Inspection::Unrecognized;
        };
        let Ok(sysinfo) = open_regular_file(&device_path.join("SysInfo")) else {
            return Inspection::Unrecognized;
        };
        let itunes_path = control_path.join("iTunes");
        let Ok(itunes) = open_directory(&itunes_path) else {
            return Inspection::Unrecognized;
        };
        let database = match open_regular_file(&itunes_path.join("iTunesDB")) {
            Ok(database) => database,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Inspection::MissingDatabase;
            }
            Err(_) => return Inspection::InvalidDatabase,
        };

        Inspection::Database(PublicAuthority(DatabaseAuthority {
            mount_path: mount_path.to_path_buf(),
            mount,
            control,
            device,
            sysinfo,
            itunes,
            database,
        }))
    }

    fn open_directory(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let (_, attributes) = identity(&file)?;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(io::Error::other("readiness authority is not a directory"));
        }
        Ok(file)
    }

    fn open_regular_file(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let (_, attributes) = identity(&file)?;
        if attributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
            return Err(io::Error::other(
                "readiness authority is not a regular file",
            ));
        }
        Ok(file)
    }

    fn same_directory(path: &Path, expected: &File) -> bool {
        open_directory(path).is_ok_and(|current| same_file(&current, expected))
    }

    fn same_regular_file(path: &Path, expected: &File) -> bool {
        open_regular_file(path).is_ok_and(|current| same_file(&current, expected))
    }

    fn same_file(left: &File, right: &File) -> bool {
        let (Ok((left, _)), Ok((right, _))) = (identity(left), identity(right)) else {
            return false;
        };
        left == right
    }

    fn identity(file: &File) -> io::Result<(Identity, u32)> {
        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        let result =
            unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            Identity {
                volume_serial: info.dwVolumeSerialNumber,
                file_index_high: info.nFileIndexHigh,
                file_index_low: info.nFileIndexLow,
            },
            info.dwFileAttributes,
        ))
    }
}
