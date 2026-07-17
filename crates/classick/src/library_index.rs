//! Per-file tag index of the source library — the data behind the Choose
//! Music browser and the selection filter. PURE CACHE: atomic writes,
//! last-writer-wins (scan subprocess and sync inline-probe may both write);
//! a lost entry costs one re-probe, never correctness.

use crate::selection::TrackFacts;
use crate::source::SourceEntry;
use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// v2 added `IndexedTrack::year` (playlist_rules Field::Year needs cached
/// year data). Bumped so `load_or_empty` discards any v1 cache instead of
/// serving `year: None` forever for files whose (mtime,size) never drift —
/// see `load_or_empty`.
pub const INDEX_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedTrack {
    pub mtime: i64,
    pub size: u64,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album_artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub duration_ms: u64,
    /// Release year, parsed from the tag's date string. `None` when the
    /// file has no date tag, or (pre-v2 caches) was never probed for one.
    #[serde(default)]
    pub year: Option<i32>,
}

impl IndexedTrack {
    pub fn facts(&self) -> TrackFacts<'_> {
        TrackFacts {
            artist: &self.artist,
            album_artist: &self.album_artist,
            album: &self.album,
            genre: &self.genre,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryIndex {
    pub version: u32,
    pub source_root: PathBuf,
    /// None = never fully scanned (fresh/invalidated cache).
    #[serde(default)]
    pub scanned_at_unix_secs: Option<u64>,
    #[serde(default)]
    pub files: BTreeMap<PathBuf, IndexedTrack>,
}

impl LibraryIndex {
    pub fn empty(source_root: PathBuf) -> Self {
        Self { version: INDEX_VERSION, source_root, scanned_at_unix_secs: None, files: BTreeMap::new() }
    }
}

/// Tag payload of one probe. Same fields IndexedTrack caches.
#[derive(Debug, Clone, Default)]
pub struct TrackTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub title: String,
    pub duration_ms: u64,
    pub year: Option<i32>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UpdateStats {
    pub probed: usize,
    pub reused: usize,
    pub dropped: usize,
    pub failed: usize,
}

/// <config dir>/classick/library-index.json — beside config.toml/manifest.json.
pub fn default_index_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve config dir"))?;
    Ok(dir.join(crate::PROJECT_DIR).join("library-index.json"))
}

/// Load the cache. Missing/corrupt/different-root/stale-version all mean
/// the same thing: start from an empty index for `source_root` (full
/// rescan). Never errors — the index is a cache, not a source of truth.
///
/// The version check matters beyond schema drift: `update_index` only
/// re-probes a file when its (mtime,size) changed, so a cache built before
/// a new cached field existed (e.g. `year` in v2) would otherwise serve
/// `None` for that field forever on an unchanged library. A version bump
/// forces one full rescan to backfill it.
pub fn load_or_empty(path: &Path, source_root: &Path) -> LibraryIndex {
    let loaded: Option<LibraryIndex> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    match loaded {
        Some(idx) if idx.source_root == source_root && idx.version == INDEX_VERSION => idx,
        Some(idx) if idx.source_root != source_root => {
            tracing::info!(
                "library_index: source root changed ({} -> {}); starting fresh",
                idx.source_root.display(), source_root.display());
            LibraryIndex::empty(source_root.to_path_buf())
        }
        Some(idx) => {
            tracing::info!(
                "library_index: cache version changed ({} -> {INDEX_VERSION}); starting fresh",
                idx.version);
            LibraryIndex::empty(source_root.to_path_buf())
        }
        None => LibraryIndex::empty(source_root.to_path_buf()),
    }
}

pub fn save_atomic(path: &Path, index: &LibraryIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let json = serde_json::to_string(index)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp index {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, json.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Read the selection-relevant tags with lofty. Field mapping mirrors
/// `transcode/macos_probe.rs::probe_output_from_lofty` so the browser shows
/// the same values the iTunesDB gets.
pub fn read_track_tags(path: &Path) -> Result<TrackTags> {
    let tagged = Probe::open(path)
        .with_context(|| format!("lofty open {}", path.display()))?
        .read()
        .with_context(|| format!("lofty read {}", path.display()))?;
    let mut tags = TrackTags::default();
    if let Some(t) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        let get = |key: &ItemKey| t.get_string(key).unwrap_or("").to_owned();
        tags.artist = get(&ItemKey::TrackArtist);
        tags.album_artist = get(&ItemKey::AlbumArtist);
        tags.album = get(&ItemKey::AlbumTitle);
        tags.genre = get(&ItemKey::Genre);
        tags.title = get(&ItemKey::TrackTitle);
        tags.year = t
            .get_string(&ItemKey::Year)
            .or_else(|| t.get_string(&ItemKey::RecordingDate))
            .and_then(crate::tags::parse_year);
    }
    tags.duration_ms = tagged.properties().duration().as_millis() as u64;
    Ok(tags)
}

/// The walked entries whose (mtime, size) don't match the cache — i.e. what
/// a scan will actually probe. Used to size the progress total up front.
pub fn stale_entries(index: &LibraryIndex, entries: &[SourceEntry]) -> Vec<SourceEntry> {
    entries.iter()
        .filter(|e| {
            index.files.get(&e.path)
                .map(|rec| rec.mtime != e.mtime || rec.size != e.size)
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

/// Refresh `index` against the walker's `entries`. (mtime,size) hits are
/// reused stat-only; misses go through `probe`; entries for vanished files
/// are dropped. A probe failure logs, records empty tags (Unknown-Artist
/// bucket), and keeps scanning — mirrors the walker's skip-don't-abort
/// policy. `on_progress(current, total_to_probe, path)` fires per probe.
pub fn update_index(
    index: &mut LibraryIndex,
    entries: &[SourceEntry],
    mut probe: impl FnMut(&Path) -> Result<TrackTags>,
    mut on_progress: impl FnMut(usize, usize, &Path),
) -> UpdateStats {
    let mut stats = UpdateStats::default();
    let stale = stale_entries(index, entries);
    let total = stale.len();
    let stale_paths: std::collections::HashSet<&PathBuf> = stale.iter().map(|e| &e.path).collect();

    let mut current = 0usize;
    for e in entries {
        if !stale_paths.contains(&e.path) {
            stats.reused += 1;
            continue;
        }
        current += 1;
        on_progress(current, total, &e.path);
        let tags = match probe(&e.path) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!("library_index: probe failed for {} ({err:#}); bucketing as unknown", e.path.display());
                stats.failed += 1;
                TrackTags::default()
            }
        };
        stats.probed += 1;
        index.files.insert(e.path.clone(), IndexedTrack {
            mtime: e.mtime,
            size: e.size,
            artist: tags.artist,
            album_artist: tags.album_artist,
            album: tags.album,
            genre: tags.genre,
            title: tags.title,
            duration_ms: tags.duration_ms,
            year: tags.year,
        });
    }

    let live: std::collections::HashSet<&PathBuf> = entries.iter().map(|e| &e.path).collect();
    let before = index.files.len();
    index.files.retain(|path, _| live.contains(path));
    stats.dropped = before - index.files.len();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceEntry;
    use std::path::PathBuf;

    fn entry(path: &str, mtime: i64, size: u64) -> SourceEntry {
        SourceEntry { path: PathBuf::from(path), mtime, size }
    }

    fn tags(artist: &str) -> TrackTags {
        TrackTags {
            artist: artist.to_string(),
            album_artist: String::new(),
            album: "A".to_string(),
            genre: "G".to_string(),
            title: "T".to_string(),
            duration_ms: 1000,
            year: None,
        }
    }

    #[test]
    fn cache_hit_is_not_probed() {
        let root = PathBuf::from("/music");
        let mut index = LibraryIndex::empty(root);
        index.files.insert(PathBuf::from("/music/a.flac"), IndexedTrack {
            mtime: 100, size: 5, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        let entries = vec![entry("/music/a.flac", 100, 5)];
        let stats = update_index(&mut index, &entries,
            |_| panic!("probe must not fire on (mtime,size) match"),
            |_, _, _| {});
        assert_eq!(stats.reused, 1);
        assert_eq!(stats.probed, 0);
    }

    #[test]
    fn cache_miss_probes_and_records() {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        let entries = vec![entry("/music/new.flac", 100, 5)];
        let stats = update_index(&mut index, &entries, |_| Ok(tags("Aphex Twin")), |_, _, _| {});
        assert_eq!(stats.probed, 1);
        assert_eq!(index.files[&PathBuf::from("/music/new.flac")].artist, "Aphex Twin");
        assert_eq!(index.files[&PathBuf::from("/music/new.flac")].mtime, 100);
    }

    #[test]
    fn changed_file_is_reprobed() {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        index.files.insert(PathBuf::from("/music/a.flac"), IndexedTrack {
            mtime: 100, size: 5, artist: "Old".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        let entries = vec![entry("/music/a.flac", 200, 5)]; // mtime bumped
        let stats = update_index(&mut index, &entries, |_| Ok(tags("New")), |_, _, _| {});
        assert_eq!(stats.probed, 1);
        assert_eq!(index.files[&PathBuf::from("/music/a.flac")].artist, "New");
    }

    #[test]
    fn vanished_files_are_dropped() {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        index.files.insert(PathBuf::from("/music/gone.flac"), IndexedTrack {
            mtime: 1, size: 1, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        let stats = update_index(&mut index, &[], |_| unreachable!(), |_, _, _| {});
        assert_eq!(stats.dropped, 1);
        assert!(index.files.is_empty());
    }

    #[test]
    fn probe_failure_records_empty_tags_and_counts_failed() {
        // One corrupt file must not abort the scan — it lands in the
        // Unknown Artist bucket and stays cached until the file changes.
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        let entries = vec![entry("/music/bad.flac", 100, 5)];
        let stats = update_index(&mut index, &entries,
            |_| Err(anyhow::anyhow!("boom")), |_, _, _| {});
        assert_eq!(stats.failed, 1);
        let rec = &index.files[&PathBuf::from("/music/bad.flac")];
        assert_eq!(rec.artist, "");
        assert_eq!(rec.mtime, 100, "stat still cached so it isn't re-probed every scan");
    }

    #[test]
    fn on_progress_reports_probed_files_only() {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        index.files.insert(PathBuf::from("/music/hit.flac"), IndexedTrack {
            mtime: 1, size: 1, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        let entries = vec![entry("/music/hit.flac", 1, 1), entry("/music/miss.flac", 2, 2)];
        let mut seen = Vec::new();
        update_index(&mut index, &entries, |_| Ok(tags("Y")),
            |current, total, _path| seen.push((current, total)));
        assert_eq!(seen, vec![(1, 1)], "only the cache miss is progress-reported");
    }

    #[test]
    fn stale_entries_counts_misses() {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        index.files.insert(PathBuf::from("/music/hit.flac"), IndexedTrack {
            mtime: 1, size: 1, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        let entries = vec![entry("/music/hit.flac", 1, 1), entry("/music/miss.flac", 2, 2)];
        let stale = stale_entries(&index, &entries);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].path, PathBuf::from("/music/miss.flac"));
    }

    #[test]
    fn load_or_empty_discards_index_for_different_root() {
        let base = std::env::temp_dir().join(format!("classick-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("library-index.json");
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        index.scanned_at_unix_secs = Some(42);
        save_atomic(&path, &index).unwrap();

        let same = load_or_empty(&path, &PathBuf::from("/music"));
        assert_eq!(same.scanned_at_unix_secs, Some(42));

        let other = load_or_empty(&path, &PathBuf::from("/other"));
        assert_eq!(other.scanned_at_unix_secs, None, "root change forces full rescan");
        assert_eq!(other.source_root, PathBuf::from("/other"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn load_or_empty_survives_corrupt_file() {
        let base = std::env::temp_dir().join(format!("classick-idx-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("library-index.json");
        std::fs::write(&path, b"{ nope").unwrap();
        let idx = load_or_empty(&path, &PathBuf::from("/music"));
        assert!(idx.files.is_empty(), "corrupt cache = never-scanned, not an error");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn read_track_tags_reads_lofty_fields() {
        // Reads a committed FLAC fixture with lofty — NO ffmpeg. The macOS
        // side must never depend on ffmpeg (afconvert-only), so both the
        // runtime probe (lofty) and this test stay ffmpeg-free.
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac");
        let tags = read_track_tags(std::path::Path::new(fixture)).unwrap();
        assert_eq!(tags.title, "Test Title");
        assert_eq!(tags.artist, "Test Artist");
        assert_eq!(tags.album, "Test Album");
        assert_eq!(tags.album_artist, "Test AA");
        assert_eq!(tags.genre, "Electronic");
        assert_eq!(tags.year, Some(2024), "fixture's date=2024 Vorbis comment parses to a year");
        assert!(tags.duration_ms >= 900, "1s fixture should have ~1000ms duration");
    }

    #[test]
    fn load_or_empty_discards_index_on_version_mismatch() {
        // Simulates an on-disk cache written by a previous INDEX_VERSION: even
        // though source_root matches, a version bump must force a full rescan
        // rather than silently serving stale-shaped records (e.g. `year: None`
        // forever for files whose mtime/size never changes again).
        let base = std::env::temp_dir().join(format!("classick-idx-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("library-index.json");
        let mut stale = LibraryIndex::empty(PathBuf::from("/music"));
        stale.version = INDEX_VERSION - 1;
        stale.scanned_at_unix_secs = Some(99);
        stale.files.insert(PathBuf::from("/music/a.flac"), IndexedTrack {
            mtime: 1, size: 1, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1, year: None,
        });
        save_atomic(&path, &stale).unwrap();

        let loaded = load_or_empty(&path, &PathBuf::from("/music"));
        assert_eq!(loaded.version, INDEX_VERSION, "reset to the current version");
        assert_eq!(loaded.scanned_at_unix_secs, None, "version bump forces full rescan");
        assert!(loaded.files.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn indexed_track_without_year_key_deserializes_to_none() {
        // A pre-v2 cache entry (or any hand-written JSON) that never had a
        // "year" key at all must still decode, defaulting to None — the
        // #[serde(default)] contract legacy files rely on.
        let json = r#"{"mtime":1,"size":5,"artist":"X","album_artist":"","album":"A","genre":"G","title":"T","duration_ms":1}"#;
        let track: IndexedTrack = serde_json::from_str(json).unwrap();
        assert_eq!(track.year, None);
    }
}
