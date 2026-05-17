//! Recursive FLAC walker + BLAKE3 fingerprint (first 1 MiB).
//!
//! Per SPEC §4.2: case-insensitive `*.flac`, skip `_excluded` and `.unwanted`
//! subdirs. Fingerprint is BLAKE3 of the first 1 MiB; size is captured
//! separately so the diff can use (fingerprint, size) as the change signal.

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One FLAC discovered by the walker. Cheap to clone (a few hundred bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceEntry {
    pub path: PathBuf,
    /// Unix epoch seconds.
    pub mtime: i64,
    pub size: u64,
    /// `blake3:<64-hex-chars>`.
    pub fingerprint: String,
}

const FINGERPRINT_PREFIX_BYTES: usize = 1024 * 1024;

const SKIPPED_DIR_NAMES: &[&str] = &["_excluded", ".unwanted"];

/// Walk `root` for FLACs. Errors only on I/O failures at `root` itself;
/// per-entry errors (permission denied on a subdir, etc.) are logged via
/// tracing and skipped — we'd rather sync 1395/1400 than abort on one.
pub fn walk(root: &Path) -> Result<Vec<SourceEntry>> {
    if !root.exists() {
        return Err(anyhow!("source root does not exist: {}", root.display()));
    }
    let mut out = Vec::new();
    let iter = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_skipped_dir(e));
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("walkdir entry error (skipping): {e}");
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_flac(entry.path()) {
            continue;
        }
        match build_source_entry(entry.path()) {
            Ok(e) => out.push(e),
            Err(e) => tracing::warn!("skipping {}: {e}", entry.path().display()),
        }
    }
    Ok(out)
}

fn is_skipped_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    SKIPPED_DIR_NAMES.iter().any(|&s| name == s)
}

fn is_flac(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac"))
        .unwrap_or(false)
}

fn build_source_entry(path: &Path) -> Result<SourceEntry> {
    let meta = std::fs::metadata(path)
        .map_err(|e| anyhow!("stat: {e}"))?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let fingerprint = fingerprint(path)?;
    Ok(SourceEntry { path: path.to_path_buf(), mtime, size, fingerprint })
}

/// Hash up to the first 1 MiB of a file with BLAKE3.
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| anyhow!("open for fingerprint: {e}"))?;
    let mut buf = vec![0u8; FINGERPRINT_PREFIX_BYTES];
    let mut read = 0usize;
    while read < buf.len() {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,  // EOF
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow!("read for fingerprint: {e}")),
        }
    }
    let hash = blake3::hash(&buf[..read]);
    Ok(format!("blake3:{}", hash.to_hex()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_flac(dir: &std::path::Path, name: &str, payload: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(payload).unwrap();
        path
    }

    #[test]
    fn fingerprint_is_blake3_of_first_mib() {
        let tmp = tempdir_under_target();
        let path = write_flac(&tmp, "a.flac", &[0xAAu8; 16]);
        let fp = fingerprint(&path).unwrap();
        assert!(fp.starts_with("blake3:"));
        assert_eq!(fp.len(), "blake3:".len() + 64, "blake3 hex is 64 chars");
    }

    #[test]
    fn fingerprint_unchanged_when_only_bytes_beyond_first_mib_differ() {
        let tmp = tempdir_under_target();
        let mut payload_a = vec![0u8; 1024 * 1024 + 100];
        let mut payload_b = payload_a.clone();
        for i in (1024 * 1024)..payload_b.len() {
            payload_b[i] = 0xFF;
        }
        let a = write_flac(&tmp, "a.flac", &payload_a);
        let b = write_flac(&tmp, "b.flac", &payload_b);
        assert_eq!(fingerprint(&a).unwrap(), fingerprint(&b).unwrap(),
            "files identical in first 1 MiB hash the same regardless of suffix");
    }

    #[test]
    fn walker_finds_flacs_recursively_case_insensitive() {
        let tmp = tempdir_under_target();
        write_flac(&tmp, "song.flac", b"x");
        write_flac(&tmp, "Sub/SONG2.FLAC", b"x");
        write_flac(&tmp, "Sub/Sub2/song3.Flac", b"x");
        write_flac(&tmp, "song.mp3", b"x");  // not flac, ignored
        let entries = walk(&tmp).unwrap();
        let names: std::collections::HashSet<_> = entries.iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains("song.flac"));
        assert!(names.contains("SONG2.FLAC"));
        assert!(names.contains("song3.Flac"));
    }

    #[test]
    fn walker_skips_excluded_subdirs() {
        let tmp = tempdir_under_target();
        write_flac(&tmp, "ok.flac", b"x");
        write_flac(&tmp, "_excluded/skip.flac", b"x");
        write_flac(&tmp, ".unwanted/also-skip.flac", b"x");
        let entries = walk(&tmp).unwrap();
        let names: std::collections::HashSet<_> = entries.iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names.contains("ok.flac"));
    }

    #[test]
    fn source_entry_has_size_and_mtime() {
        let tmp = tempdir_under_target();
        let path = write_flac(&tmp, "a.flac", &[0x42u8; 1234]);
        let entries = walk(&tmp).unwrap();
        let e = entries.iter().find(|e| e.path == path).unwrap();
        assert_eq!(e.size, 1234);
        assert!(e.mtime > 0, "mtime is unix epoch seconds, should be > 0");
    }

    /// Create a unique temp dir under `target/` so leftover dirs don't
    /// pollute the system temp and so they're easy to clean.
    fn tempdir_under_target() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("walker-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
