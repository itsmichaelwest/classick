use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;

pub(crate) struct UnixSocketLease {
    lock_file: File,
    socket_path: PathBuf,
    device: u64,
    inode: u64,
}

impl UnixSocketLease {
    pub(crate) fn bind(path: &Path) -> Result<(UnixListener, Self)> {
        let lock_path = path.with_extension("lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open unix socket lock {}", lock_path.display()))?;
        let lock_result =
            unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if lock_result != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("acquire unix socket lock {}", lock_path.display()));
        }

        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                std::fs::remove_file(path)
                    .with_context(|| format!("remove stale unix socket {}", path.display()))?;
            }
            Ok(_) => bail!(
                "refusing to replace non-socket unix socket path {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect unix socket {}", path.display()));
            }
        }

        let listener = UnixListener::bind(path)
            .with_context(|| format!("bind unix socket {}", path.display()))?;
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect bound unix socket {}", path.display()))?;
        let lease = Self {
            lock_file,
            socket_path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        Ok((listener, lease))
    }
}

impl Drop for UnixSocketLease {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.socket_path) {
            if metadata.dev() == self.device && metadata.ino() == self.inode {
                let _ = std::fs::remove_file(&self.socket_path);
            }
        }
        let _ = unsafe { libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("classick-unix-socket-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn socket_path(&self) -> PathBuf {
            self.0.join("classick.sock")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn second_lease_cannot_replace_the_owned_socket() {
        let dir = TestDir::new();
        let path = dir.socket_path();
        let (_listener, lease) = UnixSocketLease::bind(&path).unwrap();
        let owned = std::fs::symlink_metadata(&path).unwrap();

        let error = UnixSocketLease::bind(&path)
            .err()
            .expect("second bind must fail");

        let current = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!((current.dev(), current.ino()), (owned.dev(), owned.ino()));
        assert!(error.to_string().contains("lock"), "{error:#}");
        drop(lease);
    }

    #[tokio::test]
    async fn dropping_lease_removes_its_socket_but_keeps_the_lock_file() {
        let dir = TestDir::new();
        let path = dir.socket_path();
        let lock_path = path.with_extension("lock");
        let (_listener, lease) = UnixSocketLease::bind(&path).unwrap();
        assert!(path.exists());

        drop(lease);

        assert!(!path.exists());
        assert!(lock_path.exists());
    }

    #[tokio::test]
    async fn dropping_lease_preserves_a_replacement_inode() {
        let dir = TestDir::new();
        let path = dir.socket_path();
        let (_listener, lease) = UnixSocketLease::bind(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();

        drop(lease);

        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
}
