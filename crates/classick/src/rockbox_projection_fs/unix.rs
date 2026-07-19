use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(super) fn rename_atomic(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    if replace {
        return fs::rename(source, destination);
    }

    #[cfg(target_os = "linux")]
    {
        return rename_no_replace_linux(source, destination);
    }
    #[cfg(not(target_os = "linux"))]
    {
        link_then_unlink(source, destination)
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace_linux(source: &Path, destination: &Path) -> io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let source = c_path(source)?;
    let destination = c_path(destination)?;
    let renamed = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if renamed == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOSYS) {
        link_then_unlink(
            Path::new(std::ffi::OsStr::from_bytes(source.as_bytes())),
            Path::new(std::ffi::OsStr::from_bytes(destination.as_bytes())),
        )
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn link_then_unlink(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}
