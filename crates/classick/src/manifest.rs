//! Sync state: per-source-file record of (source identity, iPod identity).
//! Atomic JSON file at %APPDATA%\classick\manifest.json per SPEC §4.3.

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
    /// Source library root from the last successful sync. Used to detect when
    /// the user accidentally points --source at a different directory (which
    /// would otherwise produce a catastrophic remove=N diff against an
    /// unrelated source). Phase 2/3.x/3.y/3.z manifests deserialize cleanly
    /// because of #[serde(default)].
    #[serde(default)]
    pub last_source_root: Option<PathBuf>,
    pub tracks: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn empty() -> Self {
        Self { version: 1, ipod_serial: None, last_source_root: None, tracks: Vec::new() }
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
    /// One of: "ffmpeg" | "refalac" | "passthrough" | "unknown". Identifies
    /// which encoder produced the on-iPod file (or that it was a passthrough
    /// copy with no encoder involved). Used by diff's encoder-mismatch
    /// heuristic to trigger Modify when the user changes --encoder.
    /// Phase 2 manifests deserialize as "unknown" so the upgrade run doesn't
    /// trigger a thundering re-encode.
    #[serde(default = "default_encoder")]
    pub encoder: String,
    /// E.g. "ffmpeg n7.0" or "refalac 1.85". Empty for passthrough or unknown.
    #[serde(default)]
    pub encoder_version: String,
    /// Source codec, e.g. "flac" | "mp3" | "aac" | "alac" | "wav" | "ogg"
    /// | "opus" | "aiff". Used for stats and future format-change detection.
    /// Phase 2 entries (FLAC-only era) default to "flac".
    #[serde(default = "default_source_format")]
    pub source_format: String,
}

fn default_source_known() -> bool { true }
fn default_encoder() -> String { "unknown".to_string() }
fn default_source_format() -> String { "flac".to_string() }

#[derive(Debug, Clone)]
pub enum Action {
    Add(SourceEntry),
    /// Source is present and changed; old manifest entry must be removed
    /// from iPod first, then the new source added.
    Modify(SourceEntry, ManifestEntry),
    Remove(ManifestEntry),
    Unchanged(ManifestEntry),
    /// File fingerprint changed (e.g. tag/art edit) but the audio frames are
    /// bit-identical to what's already on the iPod. The orchestrator updates
    /// the iPod-side tags + thumbnails in place without re-transcoding or
    /// re-copying the audio file (Phase 3.x fast path).
    MetadataOnly {
        source: SourceEntry,
        entry: ManifestEntry,
    },
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
/// entry is Unchanged (mtime was merely touched). Otherwise, if the manifest
/// has a stored `audio_fingerprint`, we compute the source's current audio
/// fingerprint via `compute_audio_fingerprint`; matching values mean tags
/// or art changed without touching the audio frames → `MetadataOnly`.
/// Anything else falls through to `Modify`.
pub fn diff(
    manifest: &Manifest,
    sources: &[SourceEntry],
    mut compute_fingerprint: impl FnMut(&Path) -> Result<String>,
    mut compute_audio_fingerprint: impl FnMut(&Path) -> Result<String>,
    target_encoder: &str,
    force_reencode: bool,
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
                    // Encoder-mismatch check: if the stored encoder differs
                    // from what we'd use now, the file body on iPod is the
                    // wrong encoder's output and needs re-encoding even
                    // though the source is unchanged.
                    if is_encoder_mismatch(entry, target_encoder, force_reencode) {
                        actions.push(Action::Modify(src.clone(), (*entry).clone()));
                    } else {
                        actions.push(Action::Unchanged((*entry).clone()));
                    }
                } else {
                    // Slow path: hash the first MiB and compare.
                    let fp = compute_fingerprint(&src.path)?;
                    let content_unchanged = fp == entry.source_fingerprint
                        && src.size == entry.source_size;
                    if content_unchanged {
                        // mtime was touched but content is identical.
                        // Same encoder-mismatch check as the fast path.
                        if is_encoder_mismatch(entry, target_encoder, force_reencode) {
                            actions.push(Action::Modify(src.clone(), (*entry).clone()));
                        } else {
                            actions.push(Action::Unchanged((*entry).clone()));
                        }
                    } else if !entry.audio_fingerprint.is_empty() {
                        // Phase 3.x path: file fingerprint differs, but the
                        // manifest has a stored audio-only fingerprint to
                        // compare against. If the source's current audio
                        // fingerprint matches, it's a tag/art edit only.
                        let audio_fp = compute_audio_fingerprint(&src.path)?;
                        if audio_fp == entry.audio_fingerprint {
                            actions.push(Action::MetadataOnly {
                                source: src.clone(),
                                entry: (*entry).clone(),
                            });
                        } else {
                            actions.push(Action::Modify(src.clone(), (*entry).clone()));
                        }
                    } else {
                        // Bootstrap path: Phase 2 manifest entry with no
                        // stored audio_fingerprint to compare against — must
                        // fall through to Modify so the orchestrator
                        // populates audio_fingerprint on the rewrite.
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

/// True iff this manifest entry's stored encoder differs from the target
/// encoder in a way that means we should re-encode.
///
/// Carve-outs:
/// - `force = true`: always returns true. User asked for it.
/// - `encoder == "unknown"`: Phase 2 manifest entry (no encoder field on disk).
///   Don't trigger spurious re-encodes on first Phase 3 run — let the entry
///   get populated naturally on its next normal Modify.
/// - `encoder == "passthrough"`: there's no encoder for a copied file; the
///   on-iPod bytes are the source bytes regardless of what's set globally.
fn is_encoder_mismatch(entry: &ManifestEntry, target: &str, force: bool) -> bool {
    if force { return true; }
    if entry.encoder == "unknown" { return false; }
    if entry.encoder == "passthrough" { return false; }
    entry.encoder != target
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
            encoder: "unknown".to_string(),
            encoder_version: String::new(),
            source_format: "flac".to_string(),
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

    /// Audio-fingerprint callback that always returns the given value.
    fn returns_audio(fp: &'static str) -> impl FnMut(&Path) -> Result<String> {
        move |_| Ok(fp.to_string())
    }

    /// Audio-fingerprint callback that panics if invoked. Use when the test
    /// scenario must NOT reach the audio-fingerprint branch (e.g. fast-path
    /// stat-match, bootstrap entries with empty audio_fingerprint, removes,
    /// etc.).
    fn never_called_audio() -> impl FnMut(&Path) -> Result<String> {
        |_| panic!("audio fingerprint callback should not be called in this scenario")
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
        let path = std::env::temp_dir().join(format!("classick-test-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let m = load_or_default(&path).unwrap();
        assert_eq!(m.tracks.len(), 0);
        assert_eq!(m.version, 1);
    }

    #[test]
    fn save_atomic_roundtrip() {
        let path = std::env::temp_dir().join(format!("classick-test-rt-{}.json", std::process::id()));
        let m = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)] };
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
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_classifies_new() {
        // Add path doesn't go through the fingerprint callback either.
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Add(_)));
    }

    #[test]
    fn diff_classifies_modified_when_fingerprint_changes() {
        // Bump mtime so stat doesn't match → slow path runs. Callback
        // returns a fingerprint that differs from the manifest. Manifest
        // entry has empty audio_fingerprint (sample_entry default) so the
        // bootstrap branch emits Modify without invoking the audio callback.
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let mut src = sample_source(r"C:\a.flac", "blake3:bb", 100);
        src.mtime = 1700099999;
        let sources = vec![src];
        let actions = diff(&manifest, &sources, returns("blake3:bb"), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_modified_when_size_changes() {
        // Size differs → stat fails, callback fires once. Even if first-MiB
        // fingerprint matches, the size guard demotes to Modify (truncated
        // file scenario). Manifest entry has empty audio_fingerprint so the
        // bootstrap branch fires — no audio callback invocation.
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 200)];
        let actions = diff(&manifest, &sources, returns("blake3:aa"), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_removed() {
        // No source list → Remove emitted without any callback work.
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Remove(_)));
    }

    #[test]
    fn diff_preserves_unknown_source_entries() {
        let mut entry = sample_entry(r"C:\unknown.flac", "blake3:??", 0);
        entry.source_known = false;  // from --rebuild-manifest
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![entry] };
        let sources = vec![];  // no sources present
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 0, "unknown-source entries are NOT removed when source is absent");
    }

    #[test]
    fn diff_unchanged_after_touch_but_same_content() {
        // mtime differs from manifest, sizes equal, fingerprint still matches.
        // Slow path runs, callback fires, but result is Unchanged (file
        // fingerprint matched → audio callback not consulted).
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let mut src = sample_source(r"C:\a.flac", "blake3:aa", 100);
        src.mtime = 1700099999;  // touched
        let actions = diff(&manifest, &[src], returns("blake3:aa"), never_called_audio(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_classifies_metadata_only_when_audio_matches() {
        // Manifest has a stored audio_fingerprint; source's file fingerprint
        // differs (tags edited) but the audio_fingerprint matches → MetadataOnly.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![entry] };
        // Bump mtime so stat doesn't match → slow path runs.
        let mut src = sample_source(r"C:\a.flac", "blake3:bb", 100);
        src.mtime = 1700099999;
        let sources = vec![src];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            returns_audio("blake3-audio:zz"),
            "ffmpeg",
            false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::MetadataOnly { .. }),
            "got {:?}", actions[0]);
    }

    #[test]
    fn diff_falls_back_to_modify_when_manifest_has_no_audio_fingerprint() {
        // Phase 2 manifest entry — audio_fingerprint is empty string.
        let entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        assert_eq!(entry.audio_fingerprint, "",
            "test premise: sample_entry produces empty audio_fingerprint");
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![entry] };
        let mut src = sample_source(r"C:\a.flac", "blake3:bb", 100);
        src.mtime = 1700099999;
        let sources = vec![src];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            never_called_audio(),  // audio callback MUST NOT fire — nothing to compare to
            "ffmpeg",
            false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "bootstrap path: missing audio_fingerprint in manifest forces Modify, got {:?}", actions[0]);
    }

    #[test]
    fn diff_classifies_modify_when_audio_actually_changed() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![entry] };
        // Source: file fingerprint changed AND audio actually differs.
        let mut src = sample_source(r"C:\a.flac", "blake3:bb", 100);
        src.mtime = 1700099999;
        let sources = vec![src];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            returns_audio("blake3-audio:different"),
            "ffmpeg",
            false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_skips_audio_fingerprint_when_stat_matches() {
        // Even with a populated audio_fingerprint on the manifest, the fast
        // stat-match path must short-circuit before either callback fires.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest,
            &sources,
            never_called(),
            never_called_audio(),
            "ffmpeg",
            false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn manifest_without_last_source_root_deserializes_cleanly() {
        // Phase 2/3.x/3.y/3.z manifests don't have the last_source_root field.
        // The #[serde(default)] on the field must let them load with
        // last_source_root=None so the source-change safeguard short-circuits
        // on first post-upgrade run (no spurious prompt).
        let old = r#"{
            "version": 1,
            "ipod_serial": null,
            "tracks": []
        }"#;
        let m: Manifest = serde_json::from_str(old).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.last_source_root, None);
        assert!(m.tracks.is_empty());

        // And new-shape JSON with the field populated round-trips.
        let new = r#"{
            "version": 1,
            "ipod_serial": null,
            "last_source_root": "F:\\music",
            "tracks": []
        }"#;
        let m: Manifest = serde_json::from_str(new).unwrap();
        assert_eq!(m.last_source_root, Some(PathBuf::from(r"F:\music")));
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

    #[test]
    fn manifest_entry_without_encoder_field_deserializes_with_unknown_default() {
        // Phase 2 manifests have no encoder field. They MUST deserialize as
        // "unknown" so the encoder-mismatch heuristic skips them — otherwise
        // every Phase 2 entry would re-encode the first time the user runs
        // Phase 3, which is exactly what we don't want.
        let phase2 = r#"{
            "source_path": "C:\\a.flac",
            "source_mtime": 1700000000,
            "source_size": 100,
            "source_fingerprint": "blake3:aa",
            "ipod_dbid": 1234,
            "ipod_relpath": "iPod_Control\\Music\\F01\\AAAA.m4a",
            "source_known": true
        }"#;
        let entry: ManifestEntry = serde_json::from_str(phase2).unwrap();
        assert_eq!(entry.encoder, "unknown",
            "missing encoder field must default to 'unknown' for back-compat");
        assert_eq!(entry.encoder_version, "",
            "missing encoder_version must default to empty string");
    }

    #[test]
    fn manifest_entry_without_source_format_deserializes_with_flac_default() {
        // Phase 2 was FLAC-only, so guessing "flac" for legacy entries is
        // historically accurate (Phase 3 addendum Change 3).
        let phase2 = r#"{
            "source_path": "C:\\a.flac",
            "source_mtime": 1700000000,
            "source_size": 100,
            "source_fingerprint": "blake3:aa",
            "ipod_dbid": 1234,
            "ipod_relpath": "iPod_Control\\Music\\F01\\AAAA.m4a",
            "source_known": true
        }"#;
        let entry: ManifestEntry = serde_json::from_str(phase2).unwrap();
        assert_eq!(entry.source_format, "flac",
            "missing source_format must default to 'flac' (Phase 2 only handled FLAC)");
    }

    #[test]
    fn diff_encoder_match_emits_unchanged() {
        // entry.encoder == target_encoder, no fingerprint change → Unchanged.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest, &sources, never_called(), never_called_audio(),
            "ffmpeg", false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)),
            "encoder match + content unchanged must stay Unchanged; got {:?}", actions[0]);
    }

    #[test]
    fn diff_encoder_mismatch_emits_modify() {
        // entry.encoder = "ffmpeg", target_encoder = "refalac", no fingerprint
        // change → Modify (the on-iPod bytes are ffmpeg's, user wants refalac's).
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest, &sources, never_called(), never_called_audio(),
            "refalac", false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "encoder mismatch on otherwise-Unchanged entry must trigger Modify; got {:?}",
            actions[0]);
    }

    #[test]
    fn diff_force_reencode_overrides_match() {
        // entry.encoder = target_encoder but force=true → Modify regardless.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest, &sources, never_called(), never_called_audio(),
            "ffmpeg", true,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "--force-reencode must promote even encoder-match entries to Modify");
    }

    #[test]
    fn diff_unknown_encoder_preserved() {
        // Phase 2 entry has encoder="unknown". Target is "ffmpeg". The
        // carve-out keeps it Unchanged so the Phase 2→3 upgrade doesn't
        // trigger a thundering re-encode across the whole library.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "unknown".to_string();
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest, &sources, never_called(), never_called_audio(),
            "ffmpeg", false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)),
            "unknown-encoder entries (Phase 2 back-compat) must stay Unchanged; got {:?}",
            actions[0]);
    }

    #[test]
    fn diff_passthrough_encoder_immune_to_mismatch() {
        // Passthrough files have no encoder; switching --encoder is irrelevant
        // because the bytes on iPod are the source bytes verbatim.
        let mut entry = sample_entry(r"C:\a.mp3", "blake3:aa", 100);
        entry.encoder = "passthrough".to_string();
        entry.source_format = "mp3".to_string();
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry],
        };
        let sources = vec![sample_source(r"C:\a.mp3", "blake3:aa", 100)];
        let actions = diff(
            &manifest, &sources, never_called(), never_called_audio(),
            "refalac", false,
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)),
            "passthrough entries must be immune to encoder-mismatch; got {:?}",
            actions[0]);
    }
}
