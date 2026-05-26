//! Recursive FLAC walker + BLAKE3 fingerprint (first 1 MiB).
//!
//! Per SPEC §4.2: case-insensitive `*.flac`, skip `_excluded` and `.unwanted`
//! subdirs. Walker is stat-only — no file content reads — so a 1,400-file
//! tree over SMB stays in the seconds range. The fingerprint (BLAKE3 of the
//! first 1 MiB) is exposed as a standalone function the diff invokes lazily
//! only when mtime+size don't match the manifest (SPEC §6 #2).

use anyhow::{anyhow, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One FLAC discovered by the walker. Cheap to clone (a few hundred bytes).
/// Stat-only — no fingerprint here; see [`fingerprint`] for the on-demand hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceEntry {
    pub path: PathBuf,
    /// Unix epoch seconds.
    pub mtime: i64,
    pub size: u64,
}

const FINGERPRINT_PREFIX_BYTES: usize = 1024 * 1024;

/// Per-file retry count for the SMB-walking path. With the exponential `1<<n`
/// backoff below, 3 retries spans 1+2+4 = 7 seconds of sleep before giving up.
const WALKER_MAX_RETRIES: u32 = 3;

/// Streaming read buffer size for `audio_fingerprint`'s post-metadata hash.
/// 64 KiB matches the file-system block size on every supported volume and
/// keeps the hasher chunked without pinning more memory than necessary.
const AUDIO_HASH_CHUNK_BYTES: usize = 64 * 1024;

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
        if let Some(e) = read_entry_with_retry(&entry, WALKER_MAX_RETRIES) {
            out.push(e);
        }
    }
    Ok(out)
}

/// Try to build a `SourceEntry` for `entry`, retrying transient I/O failures
/// up to `max_retries` times with exponential backoff (1s, 2s, 4s for the
/// first three retries). Useful for SMB shares that occasionally glitch
/// mid-walk. Returns `None` if every attempt fails — the walker logs and
/// skips so the rest of the run can still complete.
fn read_entry_with_retry(entry: &walkdir::DirEntry, max_retries: u32) -> Option<SourceEntry> {
    let path = entry.path();
    for attempt in 0..=max_retries {
        match build_source_entry(path) {
            Ok(e) => return Some(e),
            Err(e) if attempt == max_retries || !is_transient(&e) => {
                tracing::warn!(
                    "walker: giving up on {} after {} attempt(s): {e}",
                    path.display(),
                    attempt + 1
                );
                return None;
            }
            Err(e) => {
                let backoff_secs = 1u64 << attempt; // 1, 2, 4
                tracing::warn!(
                    "walker: {} failed (attempt {}/{}); retrying in {}s: {e}",
                    path.display(),
                    attempt + 1,
                    max_retries + 1,
                    backoff_secs
                );
                std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
            }
        }
    }
    None
}

/// Returns true if the error looks worth retrying. NotFound/PermissionDenied
/// won't recover within 7s of backoff — one stale symlink would waste 7s of
/// blocking sleep per file. Only TimedOut/Interrupted/WouldBlock/UnexpectedEof
/// are treated as transient (common SMB hiccups).
///
/// If we can't find an io::Error in the chain, we conservatively retry — this
/// happens when the source isn't an io::Error at all (e.g. a future refactor
/// where build_source_entry adds a non-io failure mode).
fn is_transient(err: &anyhow::Error) -> bool {
    // `err.chain()` yields the top-level error first, then walks each
    // `.source()` — so a `with_context`-wrapped io::Error is found here
    // even though `err.downcast_ref::<io::Error>()` alone wouldn't see it.
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            return matches!(
                io_err.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::UnexpectedEof
            );
        }
    }
    // No io::Error in the chain — conservatively retry.
    true
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
    // `with_context` preserves the underlying io::Error in the anyhow chain
    // so `is_transient` can downcast and skip retry for permanent kinds
    // (NotFound, PermissionDenied, etc.).
    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(SourceEntry { path: path.to_path_buf(), mtime, size })
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

/// BLAKE3 of just the FLAC audio frames — bypasses METADATA_BLOCKs entirely so
/// tag/art edits don't change the fingerprint.
///
/// FLAC spec: https://xiph.org/flac/format.html
/// - 4-byte "fLaC" magic
/// - Sequence of METADATA_BLOCKs (each: 1 byte last-flag+type, 3 bytes BE length, payload)
/// - Audio frames until EOF
///
/// We walk past every metadata block (cheap — header + skip), then hash the rest.
pub fn audio_fingerprint(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| anyhow!("open for audio fingerprint: {e}"))?;

    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)
        .map_err(|e| anyhow!("read fLaC magic from {}: {e}", path.display()))?;
    if &magic != b"fLaC" {
        return Err(anyhow!(
            "not a FLAC file (missing fLaC magic) at {}",
            path.display()
        ));
    }

    // Skip every metadata block to position the cursor at the start of audio frames.
    loop {
        let mut header = [0u8; 4];
        f.read_exact(&mut header)
            .map_err(|e| anyhow!("read metadata block header from {}: {e}", path.display()))?;
        let is_last = (header[0] & 0x80) != 0;
        // 24-bit big-endian payload length follows the type byte.
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        f.seek(SeekFrom::Current(length as i64))
            .map_err(|e| anyhow!("seek past metadata block in {}: {e}", path.display()))?;
        if is_last {
            break;
        }
    }

    // Cursor is now at the start of audio frames. Stream-hash from here to EOF.
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; AUDIO_HASH_CHUNK_BYTES];
    loop {
        let n = f.read(&mut buf)
            .map_err(|e| anyhow!("read audio frames from {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("blake3-audio:{}", hasher.finalize().to_hex()))
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
        let payload_a = vec![0u8; 1024 * 1024 + 100];
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

    #[test]
    fn audio_fingerprint_invariant_across_tag_edits() {
        let tmp = tempdir_under_target();
        // Synthesize two FLACs with IDENTICAL audio but DIFFERENT metadata, via ffmpeg.
        // Same lavfi sine source → bit-identical PCM → bit-identical FLAC audio frames.
        let a = tmp.join("a.flac");
        let b = tmp.join("b.flac");
        ffmpeg_synth_flac(&a, "Title A", "Artist A");
        ffmpeg_synth_flac(&b, "Title B", "Artist B");

        let fa = audio_fingerprint(&a).unwrap();
        let fb = audio_fingerprint(&b).unwrap();
        assert_eq!(fa, fb,
            "tag-only differences must not change the audio fingerprint");
        assert!(fa.starts_with("blake3-audio:"),
            "fingerprint must be prefixed to distinguish from file fingerprint");
        assert_eq!(fa.len(), "blake3-audio:".len() + 64);

        // Sanity: confirm the FILE fingerprints DO differ (tags changed the file bytes)
        let file_a = fingerprint(&a).unwrap();
        let file_b = fingerprint(&b).unwrap();
        assert_ne!(file_a, file_b,
            "file fingerprints SHOULD differ when tags differ — this confirms the test setup");
    }

    #[test]
    fn audio_fingerprint_differs_when_audio_differs() {
        let tmp = tempdir_under_target();
        let a = tmp.join("a.flac");
        let b = tmp.join("b.flac");
        ffmpeg_synth_flac_with_freq(&a, "Same Title", 440.0);
        ffmpeg_synth_flac_with_freq(&b, "Same Title", 880.0);  // different sine frequency
        let fa = audio_fingerprint(&a).unwrap();
        let fb = audio_fingerprint(&b).unwrap();
        assert_ne!(fa, fb,
            "different audio content must produce different audio fingerprints");
    }

    #[test]
    fn audio_fingerprint_rejects_non_flac() {
        let tmp = tempdir_under_target();
        let p = tmp.join("not-a-flac.txt");
        std::fs::write(&p, b"hello world").unwrap();
        let err = audio_fingerprint(&p).unwrap_err();
        assert!(err.to_string().contains("fLaC"),
            "error message must mention the missing FLAC magic: {err}");
    }

    /// Helper: synthesize a 1-second 440Hz sine FLAC with the given title/artist tags via ffmpeg.
    /// Used by audio_fingerprint tests.
    fn ffmpeg_synth_flac(path: &std::path::Path, title: &str, artist: &str) {
        ffmpeg_synth_flac_with_freq_and_tags(path, 440.0, title, artist);
    }

    fn ffmpeg_synth_flac_with_freq(path: &std::path::Path, title: &str, freq: f64) {
        ffmpeg_synth_flac_with_freq_and_tags(path, freq, title, "Test Artist");
    }

    fn ffmpeg_synth_flac_with_freq_and_tags(
        path: &std::path::Path,
        freq: f64,
        title: &str,
        artist: &str,
    ) {
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel", "error", "-y",
                "-f", "lavfi",
                "-i", &format!("sine=frequency={freq}:duration=1:sample_rate=44100"),
                "-c:a", "flac",
                "-metadata", &format!("TITLE={title}"),
                "-metadata", &format!("ARTIST={artist}"),
            ])
            .arg(path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg synth failed for {}", path.display());
    }
}
