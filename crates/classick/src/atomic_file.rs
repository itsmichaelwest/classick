use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Default)]
pub struct AtomicFileWriter;

impl AtomicFileWriter {
    pub fn new() -> Self {
        Self
    }

    pub fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("create atomic-file parent {}", parent.display()))?;
        let temporary = temporary_path(path);
        let result = (|| {
            let mut file = File::create(&temporary)
                .with_context(|| format!("create atomic temp file {}", temporary.display()))?;
            file.write_all(contents)
                .with_context(|| format!("write atomic temp file {}", temporary.display()))?;
            file.sync_all()
                .with_context(|| format!("sync atomic temp file {}", temporary.display()))?;
            drop(file);
            replace(&temporary, path).with_context(|| {
                format!("replace {} with {}", path.display(), temporary.display())
            })?;
            sync_parent_best_effort(parent);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let suffix = format!(
        ".tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let mut name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("atomic"))
        .to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent_best_effort(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_best_effort(_parent: &Path) {}

#[cfg(test)]
mod tests {
    use super::AtomicFileWriter;
    use std::path::{Path, PathBuf};

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!(
                "atomic-file-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn replaces_an_existing_file_without_exposing_partial_contents() {
        let root = tempdir();
        let path = root.join("manifest.json");
        std::fs::write(&path, b"old").unwrap();

        AtomicFileWriter::new()
            .write(&path, b"new manifest")
            .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new manifest");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
}
