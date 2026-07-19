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

    #[cfg(target_os = "linux")]
    {
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                directory,
                source.as_ptr(),
                directory,
                destination.as_ptr(),
                2_u32,
            )
        };
        if result == 0 {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }

    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic projection target exchange is unavailable",
    ))
}

pub(super) fn rename_atomic_at(
    directory: RawFd,
    source: &str,
    destination: &str,
    replace: bool,
) -> io::Result<()> {
    let source = c_name(source)?;
    let destination = c_name(destination)?;
    if replace {
        return renameat(directory, &source, &destination);
    }

    #[cfg(target_os = "linux")]
    {
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                directory,
                source.as_ptr(),
                directory,
                destination.as_ptr(),
                1_u32,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOSYS) {
            return Err(error);
        }
    }

    let linked = unsafe {
        libc::linkat(
            directory,
            source.as_ptr(),
            directory,
            destination.as_ptr(),
            0,
        )
    };
    if linked == -1 {
        return Err(io::Error::last_os_error());
    }
    let removed = unsafe { libc::unlinkat(directory, source.as_ptr(), 0) };
    if removed == -1 {
        let error = io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(directory, destination.as_ptr(), 0) };
        return Err(error);
    }
    Ok(())
}

fn renameat(directory: RawFd, source: &CString, destination: &CString) -> io::Result<()> {
    let result =
        unsafe { libc::renameat(directory, source.as_ptr(), directory, destination.as_ptr()) };
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
