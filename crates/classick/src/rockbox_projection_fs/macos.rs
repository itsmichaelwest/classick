use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;

pub(super) use super::unix_common::{EntryIdentity, EntryKind, ManagedDirectory};

pub(super) fn exchange_atomic_at(
    directory: RawFd,
    source: &str,
    destination: &str,
) -> io::Result<()> {
    let source = c_name(source)?;
    let destination = c_name(destination)?;
    let result = unsafe {
        libc::renameatx_np(
            directory,
            source.as_ptr(),
            directory,
            destination.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn rename_atomic_at(
    directory: RawFd,
    source: &str,
    destination: &str,
    replace: bool,
) -> io::Result<()> {
    let source = c_name(source)?;
    let destination = c_name(destination)?;
    let result = if replace {
        unsafe { libc::renameat(directory, source.as_ptr(), directory, destination.as_ptr()) }
    } else {
        unsafe {
            libc::renameatx_np(
                directory,
                source.as_ptr(),
                directory,
                destination.as_ptr(),
                libc::RENAME_EXCL,
            )
        }
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn c_name(name: &str) -> io::Result<CString> {
    CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "filename contains NUL"))
}
