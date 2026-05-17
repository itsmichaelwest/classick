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

/// Diff a manifest against the current source state. See SPEC §4.3 for the
/// classification rules; this is "fingerprint + size determines unchanged",
/// not the spec's mention of mtime (mtime is captured but not used in the
/// comparison — it's unreliable across filesystems).
pub fn diff(manifest: &Manifest, sources: &[SourceEntry]) -> Vec<Action> {
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
                let unchanged = entry.source_fingerprint == src.fingerprint
                    && entry.source_size == src.size;
                if unchanged {
                    actions.push(Action::Unchanged((*entry).clone()));
                } else {
                    actions.push(Action::Modify(src.clone(), (*entry).clone()));
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

    actions
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
        }
    }

    fn sample_source(path: &str, fp: &str, size: u64) -> SourceEntry {
        SourceEntry {
            path: PathBuf::from(path),
            mtime: 1700000000,
            size,
            fingerprint: fp.to_string(),
        }
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
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_classifies_new() {
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Add(_)));
    }

    #[test]
    fn diff_classifies_modified_when_fingerprint_changes() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:bb", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_modified_when_size_changes() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 200)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_removed() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Remove(_)));
    }

    #[test]
    fn diff_preserves_unknown_source_entries() {
        let mut entry = sample_entry(r"C:\unknown.flac", "blake3:??", 0);
        entry.source_known = false;  // from --rebuild-manifest
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![];  // no sources present
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 0, "unknown-source entries are NOT removed when source is absent");
    }
}
