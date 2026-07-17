//! User's sync selection: which artists/albums/genres go to the iPod.
//! JSON at <config dir>/classick/selection.json. Missing/corrupt file
//! degrades to mode=All (sync everything) — never to "sync nothing".

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SELECTION_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    All,
    Include,
    Exclude,
}

/// One checkbox the user ticked. `artist`/`name` values compare
/// case-insensitively against the library index's tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionRule {
    Artist { name: String },
    Album { artist: String, album: String },
    Genre { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub version: u32,
    #[serde(default)]
    pub mode: SelectionMode,
    #[serde(default)]
    pub rules: Vec<SelectionRule>,
}

impl Selection {
    pub fn all() -> Self {
        Self { version: SELECTION_VERSION, mode: SelectionMode::All, rules: Vec::new() }
    }

    /// Does the current selection want this track on the iPod?
    pub fn wants(&self, facts: &TrackFacts) -> bool {
        match self.mode {
            SelectionMode::All => true,
            SelectionMode::Include => self.matches_any(facts),
            SelectionMode::Exclude => !self.matches_any(facts),
        }
    }

    fn matches_any(&self, facts: &TrackFacts) -> bool {
        let artist = facts.effective_artist();
        self.rules.iter().any(|rule| match rule {
            SelectionRule::Artist { name } => eq_fold(name, artist),
            SelectionRule::Album { artist: a, album } => {
                eq_fold(a, artist) && eq_fold(album, facts.album)
            }
            SelectionRule::Genre { name } => eq_fold(name, facts.genre),
        })
    }
}

/// The tag view rules evaluate against. Callers map index entries into this.
#[derive(Debug, Clone, Copy)]
pub struct TrackFacts<'a> {
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
}

impl<'a> TrackFacts<'a> {
    /// The grouping artist: album_artist when set (compilations group under
    /// "Various Artists"), else the track artist. Mirrors the browser display.
    pub fn effective_artist(&self) -> &'a str {
        if self.album_artist.is_empty() { self.artist } else { self.album_artist }
    }
}

fn eq_fold(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// <config dir>/classick/selection.json — beside config.toml.
pub fn default_selection_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve config dir"))?;
    Ok(dir.join(crate::PROJECT_DIR).join("selection.json"))
}

/// Which selection.json a given iPod actually uses: its own per-device file
/// when `custom_selection` is on, otherwise the shared one. `identity` is
/// `None` when no iPod is configured yet (or the caller doesn't know it) —
/// that degrades to shared, same as `custom_selection: false`.
pub fn effective_selection_path(identity: Option<&crate::config_file::IpodIdentity>) -> Result<PathBuf> {
    match identity {
        Some(id) if id.custom_selection => crate::device_state::device_selection_path(&id.serial),
        _ => default_selection_path(),
    }
}

/// One-time seed when a device switches shared -> custom: copy the shared
/// selection.json to the new per-device path so the user's existing choices
/// carry over instead of silently resetting to mode=All. No-op if there's
/// nothing to seed (`shared` missing) or the per-device file already exists
/// (never clobber an established custom selection).
pub fn seed_custom_selection(shared: &Path, custom: &Path) -> Result<()> {
    if custom.exists() || !shared.exists() {
        return Ok(());
    }
    if let Some(parent) = custom.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    std::fs::copy(shared, custom).with_context(|| {
        format!("seed custom selection {} -> {}", shared.display(), custom.display())
    })?;
    Ok(())
}

/// Never errors: missing or unparseable selection degrades to mode=All
/// (today's sync-everything behavior) with a logged warning.
pub fn load_or_all(path: &Path) -> Selection {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(sel) => sel,
            Err(e) => {
                tracing::warn!("selection: parse failed at {} ({e}); using mode=all", path.display());
                Selection::all()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Selection::all(),
        Err(e) => {
            tracing::warn!("selection: read failed at {} ({e}); using mode=all", path.display());
            Selection::all()
        }
    }
}

/// Atomic write: tmp + fsync + rename, same as manifest::save_atomic.
pub fn save_atomic(path: &Path, sel: &Selection) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let json = serde_json::to_string_pretty(sel)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp selection {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, json.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

use crate::library_index::{IndexedTrack, LibraryIndex, TrackTags};
use crate::source::SourceEntry;

/// Filter the walked source list down to what the selection wants. Files not
/// yet in the index (added since the last scan, or stat-stale) are probed
/// inline and folded in — the sync self-heals the gap between scans. Returns
/// the kept entries and whether the index gained/changed entries (caller
/// saves it back if so). Fail-open: if tags can't be read, the track is KEPT.
pub fn filter(
    sources: Vec<SourceEntry>,
    selection: &Selection,
    index: &mut LibraryIndex,
    mut probe: impl FnMut(&std::path::Path) -> anyhow::Result<TrackTags>,
) -> (Vec<SourceEntry>, bool) {
    if selection.mode == SelectionMode::All {
        return (sources, false);
    }
    let mut dirty = false;
    let kept = sources
        .into_iter()
        .filter(|src| {
            let fresh = index.files.get(&src.path)
                .map(|rec| rec.mtime == src.mtime && rec.size == src.size)
                .unwrap_or(false);
            if !fresh {
                match probe(&src.path) {
                    Ok(tags) => {
                        index.files.insert(src.path.clone(), IndexedTrack {
                            mtime: src.mtime, size: src.size,
                            artist: tags.artist, album_artist: tags.album_artist,
                            album: tags.album, genre: tags.genre,
                            title: tags.title, duration_ms: tags.duration_ms,
                            year: tags.year,
                        });
                        dirty = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "selection: cannot read tags for {} ({e:#}); keeping track (fail-open)",
                            src.path.display());
                        return true;
                    }
                }
            }
            let rec = &index.files[&src.path];
            selection.wants(&rec.facts())
        })
        .collect();
    (kept, dirty)
}

/// Sync-path entry point: load selection + index from their default (or
/// per-device, if `identity` has `custom_selection` on) paths, filter,
/// persist inline-probe additions. mode=All is a zero-cost passthrough (no
/// index load, no writes).
pub fn apply_to_sources(
    sources: Vec<SourceEntry>,
    source_root: &std::path::Path,
    identity: Option<&crate::config_file::IpodIdentity>,
    progress_log: impl Fn(String),
) -> Vec<SourceEntry> {
    let sel_path = match effective_selection_path(identity) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("selection: cannot resolve selection path ({e:#}); syncing everything");
            return sources;
        }
    };
    let idx_path = match crate::library_index::default_index_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("selection: cannot resolve index path ({e:#}); syncing everything");
            return sources;
        }
    };
    apply_with_paths(sources, source_root, &sel_path, &idx_path,
        crate::library_index::read_track_tags, progress_log)
}

/// Path-injected core of `apply_to_sources` (testable without touching the
/// user's real config dir).
pub fn apply_with_paths(
    sources: Vec<SourceEntry>,
    source_root: &std::path::Path,
    selection_path: &std::path::Path,
    index_path: &std::path::Path,
    probe: impl FnMut(&std::path::Path) -> anyhow::Result<TrackTags>,
    progress_log: impl Fn(String),
) -> Vec<SourceEntry> {
    let selection = load_or_all(selection_path);
    if selection.mode == SelectionMode::All {
        return sources;
    }
    let total = sources.len();
    let mut index = crate::library_index::load_or_empty(index_path, source_root);
    let (kept, dirty) = filter(sources, &selection, &mut index, probe);
    if dirty {
        if let Err(e) = crate::library_index::save_atomic(index_path, &index) {
            // Cache only — a failed save costs a re-probe next run, never a
            // wrong sync.
            tracing::warn!("selection: failed to save index after inline probes: {e:#}");
        }
    }
    progress_log(format!(
        "selection: {} of {} source track(s) selected (mode={:?})",
        kept.len(), total, selection.mode));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_index::{IndexedTrack, LibraryIndex, TrackTags};
    use crate::source::SourceEntry;
    use std::path::PathBuf;

    fn src(path: &str) -> SourceEntry {
        SourceEntry { path: PathBuf::from(path), mtime: 1, size: 10 }
    }

    fn indexed(artist: &str, album: &str, genre: &str) -> IndexedTrack {
        IndexedTrack {
            mtime: 1, size: 10,
            artist: artist.to_string(), album_artist: String::new(),
            album: album.to_string(), genre: genre.to_string(),
            title: String::new(), duration_ms: 0, year: None,
        }
    }

    fn facts<'a>(artist: &'a str, album_artist: &'a str, album: &'a str, genre: &'a str) -> TrackFacts<'a> {
        TrackFacts { artist, album_artist, album, genre }
    }

    #[test]
    fn mode_all_wants_everything() {
        let sel = Selection { version: 1, mode: SelectionMode::All, rules: vec![
            SelectionRule::Artist { name: "Nobody".into() },
        ]};
        assert!(sel.wants(&facts("Aphex Twin", "", "Drukqs", "IDM")),
            "mode=all ignores rules entirely");
    }

    #[test]
    fn include_mode_requires_a_matching_rule() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Boards of Canada".into() },
        ]};
        assert!(sel.wants(&facts("Boards of Canada", "", "Geogaddi", "IDM")));
        assert!(!sel.wants(&facts("Burial", "", "Untrue", "Dubstep")));
    }

    #[test]
    fn exclude_mode_inverts() {
        let sel = Selection { version: 1, mode: SelectionMode::Exclude, rules: vec![
            SelectionRule::Genre { name: "Ambient".into() },
        ]};
        assert!(!sel.wants(&facts("Eno", "", "Music for Airports", "Ambient")));
        assert!(sel.wants(&facts("Burial", "", "Untrue", "Dubstep")));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "aphex twin".into() },
            SelectionRule::Genre { name: "IDM".into() },
        ]};
        assert!(sel.wants(&facts("APHEX TWIN", "", "Drukqs", "")));
        assert!(sel.wants(&facts("Someone", "", "X", "idm")));
    }

    #[test]
    fn artist_means_album_artist_with_track_artist_fallback() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Various Artists".into() },
        ]};
        // album_artist present: it is the effective artist
        assert!(sel.wants(&facts("Aphex Twin", "Various Artists", "Compilation!", "")));
        // album_artist absent: track artist is the fallback
        assert!(!sel.wants(&facts("Aphex Twin", "", "Drukqs", "")));
    }

    #[test]
    fn album_rule_keys_on_artist_plus_album() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Album { artist: "Aphex Twin".into(), album: "Drukqs".into() },
        ]};
        assert!(sel.wants(&facts("Aphex Twin", "", "Drukqs", "")));
        assert!(!sel.wants(&facts("Aphex Twin", "", "Syro", "")), "other album, same artist");
        assert!(!sel.wants(&facts("Burial", "", "Drukqs", "")), "same album name, other artist");
    }

    #[test]
    fn empty_tags_are_matchable_buckets() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "".into() },
        ]};
        assert!(sel.wants(&facts("", "", "Untitled", "")), "Unknown Artist bucket is checkable");
        assert!(!sel.wants(&facts("Real Artist", "", "X", "")));
    }

    #[test]
    fn wire_shape_round_trips() {
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Boards of Canada".into() },
            SelectionRule::Album { artist: "Aphex Twin".into(), album: "Drukqs".into() },
            SelectionRule::Genre { name: "Ambient".into() },
        ]};
        let json = serde_json::to_string(&sel).unwrap();
        assert!(json.contains(r#""mode":"include""#));
        assert!(json.contains(r#""kind":"artist""#));
        assert!(json.contains(r#""kind":"album""#));
        assert!(json.contains(r#""kind":"genre""#));
        let back: Selection = serde_json::from_str(&json).unwrap();
        assert_eq!(sel, back);
    }

    #[test]
    fn load_or_all_returns_all_when_missing_or_corrupt() {
        let base = std::env::temp_dir().join(format!("classick-sel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let missing = base.join("nope.json");
        assert_eq!(load_or_all(&missing).mode, SelectionMode::All);

        let corrupt = base.join("bad.json");
        std::fs::write(&corrupt, b"{ not json").unwrap();
        assert_eq!(load_or_all(&corrupt).mode, SelectionMode::All,
            "corrupt selection must degrade to All, never to sync-nothing");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn save_then_load_round_trips() {
        let base = std::env::temp_dir().join(format!("classick-sel-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("selection.json");
        let sel = Selection { version: 1, mode: SelectionMode::Exclude, rules: vec![
            SelectionRule::Genre { name: "Podcast".into() },
        ]};
        save_atomic(&path, &sel).unwrap();
        assert_eq!(load_or_all(&path), sel);
        let _ = std::fs::remove_dir_all(&base);
    }

    fn identity(serial: &str, custom_selection: bool) -> crate::config_file::IpodIdentity {
        crate::config_file::IpodIdentity {
            serial: serial.to_string(),
            model_label: String::new(),
            name: None,
            custom_selection,
        }
    }

    #[test]
    fn effective_selection_path_none_identity_is_shared() {
        assert_eq!(effective_selection_path(None).unwrap(), default_selection_path().unwrap());
    }

    #[test]
    fn effective_selection_path_shared_when_flag_false() {
        let id = identity("EFFPATH-SHARED-TEST", false);
        assert_eq!(
            effective_selection_path(Some(&id)).unwrap(),
            default_selection_path().unwrap(),
            "custom_selection=false must resolve to the shared path"
        );
    }

    #[test]
    fn effective_selection_path_custom_when_flag_true() {
        let id = identity("EFFPATH-CUSTOM-TEST", true);
        let p = effective_selection_path(Some(&id)).unwrap();
        assert_eq!(
            p,
            crate::device_state::device_selection_path("EFFPATH-CUSTOM-TEST").unwrap(),
            "custom_selection=true must resolve to the per-device path"
        );
        assert_ne!(p, default_selection_path().unwrap());
        // device_selection_path() creates the device dir as a side effect
        // (real config dir, since effective_selection_path has no root-
        // injected variant); clean it up so tests don't litter the real
        // per-user config directory.
        if let Some(dir) = p.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn seed_custom_selection_copies_shared_to_custom_when_missing() {
        let base = std::env::temp_dir()
            .join(format!("classick-seed-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let shared = base.join("selection.json");
        let custom = base.join("devices").join("SER1").join("selection.json");
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Boards of Canada".into() },
        ]};
        save_atomic(&shared, &sel).unwrap();

        seed_custom_selection(&shared, &custom).unwrap();

        assert!(custom.exists(), "seed must create the per-device file");
        assert_eq!(load_or_all(&custom), sel, "seeded content must match the shared selection");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_custom_selection_noop_when_shared_missing() {
        let base = std::env::temp_dir()
            .join(format!("classick-seed-noshared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let shared = base.join("selection.json"); // never written
        let custom = base.join("devices").join("SER1").join("selection.json");

        seed_custom_selection(&shared, &custom).unwrap();

        assert!(!custom.exists(), "nothing to seed from; custom must stay absent");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_custom_selection_never_clobbers_existing_custom() {
        let base = std::env::temp_dir()
            .join(format!("classick-seed-noclobber-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let shared = base.join("selection.json");
        let custom = base.join("devices").join("SER1").join("selection.json");
        save_atomic(&shared, &Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Shared Artist".into() },
        ]}).unwrap();
        let established = Selection { version: 1, mode: SelectionMode::Exclude, rules: vec![
            SelectionRule::Genre { name: "Podcast".into() },
        ]};
        save_atomic(&custom, &established).unwrap();

        seed_custom_selection(&shared, &custom).unwrap();

        assert_eq!(
            load_or_all(&custom), established,
            "an already-established per-device selection must never be overwritten"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_custom_selection_is_idempotent() {
        // Calling it twice (e.g. two SaveConfig saves in a row with the flag
        // already true) must not re-copy or error the second time.
        let base = std::env::temp_dir()
            .join(format!("classick-seed-idempotent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let shared = base.join("selection.json");
        let custom = base.join("devices").join("SER1").join("selection.json");
        save_atomic(&shared, &Selection::all()).unwrap();

        seed_custom_selection(&shared, &custom).unwrap();
        assert!(custom.exists());
        // Mutate the custom file so a second copy would be observable.
        let user_edit = Selection { version: 1, mode: SelectionMode::Exclude, rules: vec![
            SelectionRule::Genre { name: "Live".into() },
        ]};
        save_atomic(&custom, &user_edit).unwrap();

        seed_custom_selection(&shared, &custom).unwrap();

        assert_eq!(load_or_all(&custom), user_edit, "second seed call must be a no-op");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn filter_mode_all_is_passthrough_and_never_probes() {
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        let sources = vec![src("/m/a.flac"), src("/m/b.flac")];
        let (kept, dirty) = filter(sources.clone(), &Selection::all(), &mut index,
            |_| panic!("mode=all must not probe"));
        assert_eq!(kept, sources);
        assert!(!dirty);
    }

    #[test]
    fn filter_include_keeps_only_matches() {
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        index.files.insert(PathBuf::from("/m/keep.flac"), indexed("Boards of Canada", "Geogaddi", "IDM"));
        index.files.insert(PathBuf::from("/m/drop.flac"), indexed("Burial", "Untrue", "Dubstep"));
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Boards of Canada".into() },
        ]};
        let (kept, _) = filter(vec![src("/m/keep.flac"), src("/m/drop.flac")], &sel, &mut index,
            |_| unreachable!("both files are indexed"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].path, PathBuf::from("/m/keep.flac"));
    }

    #[test]
    fn filter_probes_unindexed_files_inline_and_marks_dirty() {
        // A file added since the last scan isn't in the index — the sync
        // self-heals by probing it inline and folding it into the cache.
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Genre { name: "IDM".into() },
        ]};
        let (kept, dirty) = filter(vec![src("/m/fresh.flac")], &sel, &mut index, |_| Ok(TrackTags {
            artist: "Autechre".into(), album_artist: String::new(),
            album: "Amber".into(), genre: "IDM".into(), title: String::new(), duration_ms: 0, year: None,
        }));
        assert_eq!(kept.len(), 1);
        assert!(dirty, "inline probe must mark the index dirty so the caller saves it");
        assert_eq!(index.files[&PathBuf::from("/m/fresh.flac")].artist, "Autechre");
    }

    #[test]
    fn filter_reprobes_when_stat_differs_from_index() {
        // Index has stale (mtime,size) for the path — tags may have changed,
        // so trust must be re-established before evaluating rules.
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        index.files.insert(PathBuf::from("/m/a.flac"), IndexedTrack {
            mtime: 999, size: 999, // differs from src()'s (1, 10)
            artist: "Old".into(), album_artist: String::new(),
            album: "X".into(), genre: "Rock".into(), title: String::new(), duration_ms: 0, year: None,
        });
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "New".into() },
        ]};
        let (kept, dirty) = filter(vec![src("/m/a.flac")], &sel, &mut index, |_| Ok(TrackTags {
            artist: "New".into(), album_artist: String::new(),
            album: "X".into(), genre: "Rock".into(), title: String::new(), duration_ms: 0, year: None,
        }));
        assert_eq!(kept.len(), 1, "re-probed tags now match the rule");
        assert!(dirty);
    }

    #[test]
    fn filter_probe_failure_keeps_the_track() {
        // Fail-open: a track we can't read tags for stays in the sync rather
        // than silently disappearing from the iPod.
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        let sel = Selection { version: 1, mode: SelectionMode::Exclude, rules: vec![
            SelectionRule::Genre { name: "Podcast".into() },
        ]};
        let (kept, _) = filter(vec![src("/m/mystery.flac")], &sel, &mut index,
            |_| Err(anyhow::anyhow!("unreadable")));
        assert_eq!(kept.len(), 1, "unreadable tags must not drop a track");
    }

    #[test]
    fn deselected_tracks_become_remove_actions_via_diff() {
        // End-to-end composition: filter -> manifest::diff yields Remove for
        // on-iPod tracks the selection no longer wants. THE core promise.
        use crate::manifest::{diff, Action, Manifest, ManifestEntry};
        let mut index = LibraryIndex::empty(PathBuf::from("/m"));
        index.files.insert(PathBuf::from("/m/synced.flac"), indexed("Burial", "Untrue", "Dubstep"));
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![ManifestEntry {
                source_path: PathBuf::from("/m/synced.flac"),
                source_mtime: 1, source_size: 10,
                source_fingerprint: "blake3:aa".into(),
                ipod_dbid: 1, ipod_relpath: "F00/X.m4a".into(),
                source_known: true, audio_fingerprint: String::new(),
                encoder: "unknown".into(), encoder_version: String::new(),
                source_format: "flac".into(),
            }],
        };
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "Someone Else".into() },
        ]};
        let (kept, _) = filter(vec![src("/m/synced.flac")], &sel, &mut index, |_| unreachable!());
        assert!(kept.is_empty());
        let actions = diff(&manifest, &kept, |_| unreachable!(), |_| unreachable!(), "ffmpeg", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Remove(_)),
            "deselected + on-iPod must become an ordinary Remove");
    }

    #[test]
    fn apply_with_paths_filters_and_persists_inline_probes() {
        let base = std::env::temp_dir().join(format!("classick-selapply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let sel_path = base.join("selection.json");
        let idx_path = base.join("library-index.json");
        let root = PathBuf::from("/m");

        // Selection: include only IDM. Index: empty (forces inline probe).
        save_atomic(&sel_path, &Selection {
            version: 1, mode: SelectionMode::Include,
            rules: vec![SelectionRule::Genre { name: "IDM".into() }],
        }).unwrap();

        let sources = vec![src("/m/a.flac")];
        let kept = apply_with_paths(sources, &root, &sel_path, &idx_path, |_| Ok(TrackTags {
            artist: "Autechre".into(), album_artist: String::new(),
            album: "Amber".into(), genre: "IDM".into(), title: String::new(), duration_ms: 0, year: None,
        }), |_msg| {});
        assert_eq!(kept.len(), 1);

        // The inline probe must have been persisted.
        let idx = crate::library_index::load_or_empty(&idx_path, &root);
        assert_eq!(idx.files.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
