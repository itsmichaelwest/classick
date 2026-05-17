//! Sync state: per-source-file record of (source identity, iPod identity).
//! Atomic JSON file at %APPDATA%\ipod-sync\manifest.json per SPEC §4.3.

use crate::source::SourceEntry;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub ipod_serial: Option<String>,
    pub tracks: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn empty() -> Self {
        Self { version: 1, ipod_serial: None, tracks: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub source_path: PathBuf,
    pub source_mtime: i64,
    pub source_size: u64,
    pub source_fingerprint: String,
    pub ipod_dbid: u64,
    pub ipod_relpath: String,
    /// `false` means this entry was reconstructed by `--rebuild-manifest` from
    /// the iPod's DB and has no known source file. The diff preserves these
    /// untouched (no Remove action emitted) and the orchestrator skips them.
    #[serde(default = "default_source_known")]
    pub source_known: bool,
    /// BLAKE3 of just the FLAC audio frames (Phase 3.x). Empty string for
    /// entries written by Phase 2 or earlier — those fall through the diff's
    /// normal Modify path on first content change after upgrade, and the
    /// orchestrator populates this field on the resulting re-write.
    #[serde(default)]
    pub audio_fingerprint: String,
}

fn default_source_known() -> bool { true }

#[derive(Debug, Clone)]
pub enum Action {
    Add(SourceEntry),
    /// Source is present and changed; old manifest entry must be removed
    /// from iPod first, then the new source added.
    Modify(SourceEntry, ManifestEntry),
    Remove(ManifestEntry),
    Unchanged(ManifestEntry),
}

/// Diff a manifest against the current source state. See SPEC §4.3 / §6 #2.
///
/// Fast path: if a manifest entry exists for the path AND `(mtime, size)`
/// match what the walker stat'd, we trust the stored fingerprint and emit
/// `Unchanged` WITHOUT calling `compute_fingerprint`. This keeps the no-op
/// second run stat-only across thousands of files on slow filesystems (SMB).
///
/// Slow path: if mtime or size differs, we compute the fingerprint and
/// compare against the manifest. If it matches AND size matches (paranoia
/// guard against a truncated file whose first MiB happens to match), the
/// entry is Unchanged (mtime was merely touched). Otherwise it's Modify.
pub fn diff(
    manifest: &Manifest,
    sources: &[SourceEntry],
    mut compute_fingerprint: impl FnMut(&Path) -> Result<String>,
) -> Result<Vec<Action>> {
    let manifest_by_path: HashMap<&PathBuf, &ManifestEntry> = manifest
        .tracks
        .iter()
        .filter(|e| e.source_known)
        .map(|e| (&e.source_path, e))
        .collect();
    let source_paths: std::collections::HashSet<&PathBuf> =
        sources.iter().map(|s| &s.path).collect();

    let mut actions = Vec::new();

    for src in sources {
        match manifest_by_path.get(&src.path) {
            None => actions.push(Action::Add(src.clone())),
            Some(entry) => {
                let stat_matches = entry.source_mtime == src.mtime
                    && entry.source_size == src.size;
                if stat_matches {
                    // FAST PATH — no fingerprint read.
                    actions.push(Action::Unchanged((*entry).clone()));
                } else {
                    // Slow path: hash the first MiB and compare.
                    let fp = compute_fingerprint(&src.path)?;
                    let content_unchanged = fp == entry.source_fingerprint
                        && src.size == entry.source_size;
                    if content_unchanged {
                        // mtime was touched but content is identical.
                        actions.push(Action::Unchanged((*entry).clone()));
                    } else {
                        actions.push(Action::Modify(src.clone(), (*entry).clone()));
                    }
                }
            }
        }
    }

    for entry in &manifest.tracks {
        if !entry.source_known {
            continue;  // preserved as-is
        }
        if !source_paths.contains(&entry.source_path) {
            actions.push(Action::Remove(entry.clone()));
        }
    }

    Ok(actions)
}

/// Read the manifest from disk; return an empty manifest if the file doesn't exist.
pub fn load_or_default(path: &Path) -> Result<Manifest> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .with_context(|| format!("parse manifest at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::empty()),
        Err(e) => Err(anyhow!("read manifest at {}: {e}", path.display())),
    }
}

/// Write the manifest atomically: write to <path>.tmp, fsync, rename over.
/// Survives crashes mid-write; the target file is either fully old or fully new.
pub fn save_atomic(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let json = serde_json::to_string_pretty(manifest)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp manifest {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, json.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    // On Windows, std::fs::rename overwrites an existing target (unlike POSIX).
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceEntry;
    use std::path::PathBuf;

    fn sample_entry(path: &str, fp: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            source_path: PathBuf::from(path),
            source_mtime: 1700000000,
            source_size: size,
            source_fingerprint: fp.to_string(),
            ipod_dbid: 12345678901234,
            ipod_relpath: r"iPod_Control\Music\F12\KLMN.m4a".to_string(),
            source_known: true,
            audio_fingerprint: String::new(),
        }
    }

    fn sample_source(path: &str, _fp: &str, size: u64) -> SourceEntry {
        SourceEntry {
            path: PathBuf::from(path),
            mtime: 1700000000,
            size,
        }
    }

    /// Fingerprint callback that panics if called. Used to assert the fast
    /// path (no fingerprint computation) is taken.
    fn never_called() -> impl FnMut(&Path) -> Result<String> {
        |_| panic!("fingerprint callback should not be called when stat matches")
    }

    /// Fingerprint callback that always returns the given value.
    fn returns(fp: &'static str) -> impl FnMut(&Path) -> Result<String> {
        move |_| Ok(fp.to_string())
    }

    #[test]
    fn roundtrip_known_fixture() {
        const FIXTURE: &str = include_str!("../tests/fixtures/sample-manifest.json");
        let parsed: Manifest = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.tracks.len(), 2);
        assert!(parsed.tracks[0].source_known);

        let serialized = serde_json::to_string_pretty(&parsed).unwrap();
        let reparsed: Manifest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed.tracks, reparsed.tracks);
    }

    #[test]
    fn load_or_default_returns_empty_when_missing() {
        let path = std::env::temp_dir().join(format!("ipod-sync-test-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let m = load_or_default(&path).unwrap();
        assert_eq!(m.tracks.len(), 0);
        assert_eq!(m.version, 1);
    }

    #[test]
    fn save_atomic_roundtrip() {
        let path = std::env::temp_dir().join(format!("ipod-sync-test-rt-{}.json", std::process::id()));
        let m = Manifest { version: 1, ipod_serial: None, tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)] };
        save_atomic(&path, &m).unwrap();
        let loaded = load_or_default(&path).unwrap();
        assert_eq!(loaded.tracks.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diff_classifies_unchanged() {
        // mtime + size both match the manifest → FAST PATH; callback must
        // NOT fire. `never_called()` asserts that invariant for us.
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_classifies_new() {
        // Add path doesn't go through the fingerprint callback either.
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Add(_)));
    }

    #[test]
    fn diff_classifies_modified_when_fingerprint_changes() {
        // Bump mtime so stat doesn't match → slow path runs. Callback
        // returns a fingerprint that differs from the manifest → Modify.
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let mut src = sample_source(r"C:\a.flac", "blake3:bb", 100);
        src.mtime = 1700099999;
        let sources = vec![src];
        let actions = diff(&manifest, &sources, returns("blake3:bb")).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_modified_when_size_changes() {
        // Size differs → stat fails, callback fires once. Even if first-MiB
        // fingerprint matches, the size guard demotes to Modify (truncated
        // file scenario).
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 200)];
        let actions = diff(&manifest, &sources, returns("blake3:aa")).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_removed() {
        // No source list → Remove emitted without any callback work.
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![];
        let actions = diff(&manifest, &sources, never_called()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Remove(_)));
    }

    #[test]
    fn diff_preserves_unknown_source_entries() {
        let mut entry = sample_entry(r"C:\unknown.flac", "blake3:??", 0);
        entry.source_known = false;  // from --rebuild-manifest
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![];  // no sources present
        let actions = diff(&manifest, &sources, never_called()).unwrap();
        assert_eq!(actions.len(), 0, "unknown-source entries are NOT removed when source is absent");
    }

    #[test]
    fn diff_unchanged_after_touch_but_same_content() {
        // mtime differs from manifest, sizes equal, fingerprint still matches.
        // Slow path runs, callback fires, but result is Unchanged.
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let mut src = sample_source(r"C:\a.flac", "blake3:aa", 100);
        src.mtime = 1700099999;  // touched
        let actions = diff(&manifest, &[src], returns("blake3:aa")).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn manifest_entry_supports_optional_audio_fingerprint() {
        // Old-shape JSON (Phase 2 manifest with no audio_fingerprint field):
        let old = r#"{
            "source_path": "C:\\a.flac",
            "source_mtime": 1700000000,
            "source_size": 100,
            "source_fingerprint": "blake3:aa",
            "ipod_dbid": 1234,
            "ipod_relpath": "iPod_Control\\Music\\F01\\AAAA.m4a",
            "source_known": true
        }"#;
        let entry: ManifestEntry = serde_json::from_str(old).unwrap();
        assert_eq!(entry.audio_fingerprint, "",
            "Phase 2 entries must deserialize with empty audio_fingerprint");

        // New-shape JSON (Phase 3.x manifest):
        let new = r#"{
            "source_path": "C:\\b.flac",
            "source_mtime": 1700000000,
            "source_size": 200,
            "source_fingerprint": "blake3:bb",
            "ipod_dbid": 5678,
            "ipod_relpath": "iPod_Control\\Music\\F02\\BBBB.m4a",
            "source_known": true,
            "audio_fingerprint": "blake3-audio:cc"
        }"#;
        let entry: ManifestEntry = serde_json::from_str(new).unwrap();
        assert_eq!(entry.audio_fingerprint, "blake3-audio:cc");
    }
}
