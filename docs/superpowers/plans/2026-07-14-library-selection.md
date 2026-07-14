# Library Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user choose what syncs to the iPod by Artist/Album/Genre — a selection model + library-index scan in the Rust core, additive daemon protocol v1.4.0, and a "Choose Music" window in the macOS app.

**Architecture:** A `selection.json` (mode: all/include/exclude + rules) filters the walked source list *before* `manifest::diff()`, so deselection becomes ordinary `Remove` actions with zero apply-loop changes. A `--scan-library` subprocess builds an incremental tag index (`library-index.json`, lofty, mtime/size-cached). The daemon aggregates the index for the UI and evaluates rules for live previews — using the SAME `selection` module the sync uses, so preview and reality cannot diverge in logic.

**Tech Stack:** Rust (serde, lofty 0.22, tokio daemon), newline-delimited JSON IPC, Swift 6 / SwiftUI (macOS 15+).

**Spec:** `docs/superpowers/specs/2026-07-14-library-selection-design.md` — read it first.

## Global Constraints

- Missing/corrupt `selection.json` degrades to `mode: "all"` (today's behavior) — **never** to "sync nothing".
- `library-index.json` is a **pure cache**: atomic writes, last-writer-wins, a lost entry costs one re-probe, never correctness. Both the scan subprocess and the sync's inline-probe fallback may write it.
- `crate::selection` is the **only** rule evaluator. The daemon preview and the sync filter both call it. Never re-implement matching.
- Matching is case-insensitive (`str::to_lowercase()` / Swift `lowercased()`); "artist" = album_artist falling back to track artist; empty tags are matchable buckets (wire carries `""`; UI renders "Unknown Artist" / "No Genre"); genre strings match verbatim (no delimiter splitting in v1).
- Daemon protocol bump 1.3.0 → **1.4.0**, purely additive. Unknown `status_update.state` values MUST be treated as `idle` by clients.
- `docs/ipc-protocol.md` is the wire-format source of truth — update it in the same commit as the Rust wire types.
- No `println!` outside examples (IPC mode: stdout IS the wire). Use `tracing`.
- Keep files ≤ ~500 LOC. Conventional Commits. Never `git add -A` (stage files by name). Don't amend; make new commits.
- Rust: `anyhow::Result`, `.context(...)` at boundaries. Swift: wire field names are verbatim snake_case copies of the Rust names.
- Build/test: `cargo test` from repo root; `cd ui/macos && swift test` for Swift. The daemon integration suite (`crates/classick/tests/daemon_runtime_integration.rs`) is `#[cfg(windows)]`-gated and does NOT run on macOS — daemon-arm coverage comes from extracted pure functions + the manual verification task.

---

## Stage A — Core (crates/classick)

### Task 1: `selection` module — types, persistence, matching

**Files:**
- Create: `crates/classick/src/selection.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod selection;` after `pub mod scsi_inquiry;`'s block, alphabetical: between `pub mod progress;`/`pub mod source;` region — exact spot: after the `#[cfg(windows)] pub mod scsi_inquiry;` lines, before `pub mod source;`)

**Interfaces:**
- Produces: `SelectionMode { All, Include, Exclude }`, `SelectionRule { Artist{name}, Album{artist, album}, Genre{name} }`, `Selection { version, mode, rules }`, `Selection::all()`, `Selection::wants(&TrackFacts) -> bool`, `TrackFacts<'a> { artist, album_artist, album, genre }`, `TrackFacts::effective_artist()`, `default_selection_path() -> Result<PathBuf>`, `load_or_all(&Path) -> Selection`, `save_atomic(&Path, &Selection) -> Result<()>`
- Consumes: `crate::PROJECT_DIR`, `dirs::config_dir()` (same pattern as `config_file::default_path`)

- [ ] **Step 1: Write the failing tests**

Create `crates/classick/src/selection.rs` with the test module only (plus a stub so it compiles as a module — no, per TDD write tests first; the file must exist to compile, so write the full test module and let `cargo test` fail on unresolved names):

```rust
//! User's sync selection: which artists/albums/genres go to the iPod.
//! JSON at <config dir>/classick/selection.json. Missing/corrupt file
//! degrades to mode=All (sync everything) — never to "sync nothing".

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

- [ ] **Step 2: Add the module and run tests to verify they fail**

Add `pub mod selection;` to `crates/classick/src/lib.rs`. Run: `cargo test -p classick selection::`
Expected: COMPILE ERROR — `Selection`, `SelectionMode`, etc. not found.

- [ ] **Step 3: Write the implementation** (above the test module in `selection.rs`)

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p classick selection::`
Expected: all 10 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/selection.rs crates/classick/src/lib.rs
git commit -m "feat(selection): selection model — mode all/include/exclude + artist/album/genre rules"
```

---

### Task 2: `library_index` module — tag-index cache with incremental update

**Files:**
- Create: `crates/classick/src/library_index.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod library_index;` after `pub mod ipod;`)

**Interfaces:**
- Produces: `IndexedTrack { mtime: i64, size: u64, artist, album_artist, album, genre, title: String, duration_ms: u64 }`, `LibraryIndex { version, source_root: PathBuf, scanned_at_unix_secs: Option<u64>, files: BTreeMap<PathBuf, IndexedTrack> }`, `LibraryIndex::empty(source_root)`, `LibraryIndex::facts(&IndexedTrack) -> TrackFacts` (associated fn `track_facts`), `default_index_path() -> Result<PathBuf>`, `load_or_empty(&Path, source_root: &Path) -> LibraryIndex`, `save_atomic(&Path, &LibraryIndex) -> Result<()>`, `TrackTags { artist, album_artist, album, genre, title: String, duration_ms: u64 }`, `read_track_tags(&Path) -> Result<TrackTags>` (lofty), `stale_entries(&LibraryIndex, &[SourceEntry]) -> Vec<SourceEntry>`, `update_index(&mut LibraryIndex, &[SourceEntry], probe, on_progress) -> UpdateStats { probed, reused, dropped, failed }`
- Consumes: `crate::source::SourceEntry`, `crate::selection::TrackFacts`, `lofty` (already an ungated dependency in `crates/classick/Cargo.toml`)

- [ ] **Step 1: Write the failing tests**

Test module inside `library_index.rs`. Probe injection keeps lofty out of the cache-logic tests:

```rust
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
        }
    }

    #[test]
    fn cache_hit_is_not_probed() {
        let root = PathBuf::from("/music");
        let mut index = LibraryIndex::empty(root);
        index.files.insert(PathBuf::from("/music/a.flac"), IndexedTrack {
            mtime: 100, size: 5, artist: "X".into(), album_artist: String::new(),
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1,
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
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1,
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
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1,
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
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1,
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
            album: "A".into(), genre: "G".into(), title: "T".into(), duration_ms: 1,
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
        // Real-file test using the same ffmpeg-synth helper pattern as
        // source.rs's audio_fingerprint tests (ffmpeg on PATH is an accepted
        // dev dependency of this suite).
        let base = std::env::temp_dir().join(format!("classick-idx-lofty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let p = base.join("t.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi",
                   "-i", "sine=frequency=440:duration=1:sample_rate=44100",
                   "-c:a", "flac",
                   "-metadata", "TITLE=Song", "-metadata", "ARTIST=Someone",
                   "-metadata", "ALBUM=Record", "-metadata", "GENRE=IDM"])
            .arg(&p).status().expect("spawn ffmpeg");
        assert!(status.success());
        let tags = read_track_tags(&p).unwrap();
        assert_eq!(tags.artist, "Someone");
        assert_eq!(tags.album, "Record");
        assert_eq!(tags.genre, "IDM");
        assert_eq!(tags.title, "Song");
        assert!(tags.duration_ms >= 900, "1s sine should have ~1000ms duration");
        let _ = std::fs::remove_dir_all(&base);
    }
}
```

- [ ] **Step 2: Add module, run tests to verify they fail**

Add `pub mod library_index;` to `lib.rs`. Run: `cargo test -p classick library_index::`
Expected: COMPILE ERROR — types not found.

- [ ] **Step 3: Write the implementation**

```rust
//! Per-file tag index of the source library — the data behind the Choose
//! Music browser and the selection filter. PURE CACHE: atomic writes,
//! last-writer-wins (scan subprocess and sync inline-probe may both write);
//! a lost entry costs one re-probe, never correctness.

use crate::selection::TrackFacts;
use crate::source::SourceEntry;
use anyhow::{Context, Result};
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const INDEX_VERSION: u32 = 1;

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

/// Load the cache. Missing/corrupt/different-root all mean the same thing:
/// start from an empty index for `source_root` (full rescan). Never errors —
/// the index is a cache, not a source of truth.
pub fn load_or_empty(path: &Path, source_root: &Path) -> LibraryIndex {
    let loaded: Option<LibraryIndex> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    match loaded {
        Some(idx) if idx.source_root == source_root => idx,
        Some(idx) => {
            tracing::info!(
                "library_index: source root changed ({} -> {}); starting fresh",
                idx.source_root.display(), source_root.display());
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
        });
    }

    let live: std::collections::HashSet<&PathBuf> = entries.iter().map(|e| &e.path).collect();
    let before = index.files.len();
    index.files.retain(|path, _| live.contains(path));
    stats.dropped = before - index.files.len();
    stats
}
```

Note: `stats.probed` counts successful+failed probes minus... check the test expectations: `probe_failure` test expects `failed == 1` and record present. In the code above `stats.probed += 1` also fires for failures — the cache-miss test expects `probed == 1` with success. Keep `probed` = attempted probes (1 on failure too) and the failure test only asserts `failed`; that is consistent.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p classick library_index::`
Expected: all 10 PASS (the lofty test needs ffmpeg on PATH, same as the existing `source::tests::audio_fingerprint_*` tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/library_index.rs crates/classick/src/lib.rs
git commit -m "feat(selection): library-index cache — lofty tag probe + mtime/size incremental update"
```

---

### Task 3: selection filter — compose selection + index into the source list

**Files:**
- Modify: `crates/classick/src/selection.rs` (append `filter` + tests)

**Interfaces:**
- Produces: `selection::filter(sources: Vec<SourceEntry>, selection: &Selection, index: &mut LibraryIndex, probe: impl FnMut(&Path) -> Result<TrackTags>) -> (Vec<SourceEntry>, bool)` — returns filtered list + `index_dirty` flag (true when inline probes added entries)
- Consumes: Task 1 types, Task 2 `LibraryIndex`/`TrackTags`/`IndexedTrack`

- [ ] **Step 1: Write the failing tests** (append to `selection.rs`'s test module)

```rust
    use crate::library_index::{IndexedTrack, LibraryIndex, TrackTags};
    use crate::source::SourceEntry;

    fn src(path: &str) -> SourceEntry {
        SourceEntry { path: PathBuf::from(path), mtime: 1, size: 10 }
    }

    fn indexed(artist: &str, album: &str, genre: &str) -> IndexedTrack {
        IndexedTrack {
            mtime: 1, size: 10,
            artist: artist.to_string(), album_artist: String::new(),
            album: album.to_string(), genre: genre.to_string(),
            title: String::new(), duration_ms: 0,
        }
    }
    use std::path::PathBuf;

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
            album: "Amber".into(), genre: "IDM".into(), title: String::new(), duration_ms: 0,
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
            album: "X".into(), genre: "Rock".into(), title: String::new(), duration_ms: 0,
        });
        let sel = Selection { version: 1, mode: SelectionMode::Include, rules: vec![
            SelectionRule::Artist { name: "New".into() },
        ]};
        let (kept, dirty) = filter(vec![src("/m/a.flac")], &sel, &mut index, |_| Ok(TrackTags {
            artist: "New".into(), album_artist: String::new(),
            album: "X".into(), genre: "Rock".into(), title: String::new(), duration_ms: 0,
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p classick selection::`
Expected: COMPILE ERROR — `filter` not found.

- [ ] **Step 3: Implement `filter`** (append to `selection.rs`, above tests)

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p classick selection::` — all PASS. Also run the full suite once (`cargo test -p classick --lib`) to catch accidental breakage.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/selection.rs
git commit -m "feat(selection): filter walked sources through the selection before diff"
```

---

### Task 4: `--scan-library` mode

**Files:**
- Modify: `crates/classick/src/cli.rs` (new flag), `crates/classick/src/config.rs` (new field), `crates/classick/src/orchestrator.rs` (dispatch), `crates/classick/src/lib.rs` (new module)
- Create: `crates/classick/src/scan.rs`

**Interfaces:**
- Produces: `Cli.scan_library: bool`, `Config.scan_library: bool`, `scan::run(&Config, &Progress) -> Result<RunOutcome>`
- Consumes: `source::walk`, `library_index::{load_or_empty, save_atomic, stale_entries, update_index, read_track_tags, default_index_path}`, `Progress::{header, summary, track_start, track_done, log}` (signatures in `progress.rs:143-186`), `RunOutcome::Completed` from `apply_loop`

- [ ] **Step 1: Write the failing tests**

In `cli.rs` tests:

```rust
    #[test]
    fn parses_scan_library_flag() {
        let cli = Cli::try_parse_from(["classick", "--scan-library"]).unwrap();
        assert!(cli.scan_library);
        let cli = Cli::try_parse_from(["classick"]).unwrap();
        assert!(!cli.scan_library);
    }

    #[test]
    fn scan_library_conflicts_with_backfill_rockbox() {
        assert!(Cli::try_parse_from(["classick", "--scan-library", "--backfill-rockbox"]).is_err());
    }
```

In `config.rs` tests:

```rust
    #[test]
    fn scan_library_threads_through_resolve() {
        let cli = Cli::try_parse_from(["classick", "--source", r"D:\m", "--scan-library"]).unwrap();
        let cfg = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert!(cfg.scan_library);
    }
```

Create `crates/classick/src/scan.rs` with its test module (scan logic is exercised through `library_index` unit tests; here test the end-to-end file behavior with a real temp library):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scan_writes_index_and_stamps_scanned_at() {
        let base = std::env::temp_dir().join(format!("classick-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src_dir = base.join("music");
        std::fs::create_dir_all(&src_dir).unwrap();
        // Plain non-audio bytes: lofty will fail to parse -> unknown bucket,
        // which is exactly the skip-don't-abort behavior we want covered
        // without an ffmpeg dependency in this test.
        std::fs::write(src_dir.join("a.flac"), b"not really flac").unwrap();

        let index_path = base.join("library-index.json");
        let stats = scan_source(&src_dir, &index_path).unwrap();
        assert_eq!(stats.probed, 1);
        let idx = crate::library_index::load_or_empty(&index_path, &src_dir);
        assert!(idx.scanned_at_unix_secs.is_some(), "completed scan must stamp scanned_at");
        assert_eq!(idx.files.len(), 1);

        // Second scan: stat-only, nothing probed.
        let stats2 = scan_source(&src_dir, &index_path).unwrap();
        assert_eq!(stats2.probed, 0);
        assert_eq!(stats2.reused, 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

`cargo test -p classick scan` and `cargo test -p classick cli::tests::parses_scan_library_flag`
Expected: COMPILE ERRORS (missing flag/field/module).

- [ ] **Step 3: Implement**

`cli.rs` — after the `backfill_rockbox` field:

```rust
    /// Scan the source library's tags into the library index
    /// (library-index.json), then exit. Powers the Choose Music browser.
    /// Incremental: files whose (mtime, size) match the cached record are
    /// not re-read.
    #[arg(long, conflicts_with = "backfill_rockbox")]
    pub scan_library: bool,
```

`config.rs` — add `pub scan_library: bool,` to `Config` (after `backfill_rockbox`), set `scan_library: cli.scan_library,` in `resolve_with`'s `Ok(Config { ... })`.

`orchestrator.rs` — in `orchestrate`, dispatch BEFORE the backfill branch (scan needs no iPod, no encoder preflights):

```rust
    let mut config = config::resolve(cli)?;
    if config.scan_library {
        return crate::scan::run(&config, progress);
    }
    if config.backfill_rockbox {
```

`lib.rs` — add `pub mod scan;` (after `pub mod progress;`).

`scan.rs` — one shared core (`scan_with`) so `run` (progress-emitting) and `scan_source` (test entry) never duplicate the walk:

```rust
//! `--scan-library` mode: refresh library-index.json from the source tree.
//! Progress rides the existing IPC event vocabulary (summary / track_start /
//! track_done / finish) so the daemon's forwarding and the UI's progress
//! rendering need no new machinery.

use crate::apply_loop::RunOutcome;
use crate::config::Config;
use crate::library_index::{self, UpdateStats};
use crate::progress::Progress;
use crate::source;
use anyhow::Result;
use std::path::Path;

pub fn run(config: &Config, progress: &Progress) -> Result<RunOutcome> {
    let index_path = library_index::default_index_path()?;
    progress.header(
        config.source.display().to_string(),
        String::new(), // no iPod involved in a scan
        index_path.display().to_string(),
    );
    let stats = scan_with(&config.source, &index_path, |current, total, path| {
        progress.track_start(
            current,
            total,
            path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        );
        progress.track_done();
    }, |walked, to_probe| {
        // summary: reuse the wire's existing shape; only total_planned
        // matters to progress consumers ("Scanning… X of N changed files").
        progress.summary(to_probe, 0, 0, 0, walked - to_probe, to_probe);
        progress.log(format!("scan: {walked} file(s) walked, {to_probe} changed/new to read"));
    })?;
    progress.log(format!(
        "scan: probed={} reused={} dropped={} failed={}",
        stats.probed, stats.reused, stats.dropped, stats.failed));
    Ok(RunOutcome::Completed)
}

/// Progress-free entry used by tests: walk + incremental update + save.
pub fn scan_source(source_root: &Path, index_path: &Path) -> Result<UpdateStats> {
    scan_with(source_root, index_path, |_, _, _| {}, |_, _| {})
}

/// Shared core: walk, size the probe set, incremental update, stamp
/// scanned_at, atomic save. `on_plan(walked, to_probe)` fires once before
/// probing starts; `on_progress` fires per probed file.
fn scan_with(
    source_root: &Path,
    index_path: &Path,
    on_progress: impl FnMut(usize, usize, &Path),
    on_plan: impl FnOnce(usize, usize),
) -> Result<UpdateStats> {
    let entries = source::walk(source_root)?;
    let mut index = library_index::load_or_empty(index_path, source_root);
    let to_probe = library_index::stale_entries(&index, &entries).len();
    on_plan(entries.len(), to_probe);
    let stats = library_index::update_index(
        &mut index, &entries, library_index::read_track_tags, on_progress);
    index.scanned_at_unix_secs = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    library_index::save_atomic(index_path, &index)?;
    Ok(stats)
}
```

- [ ] **Step 4: Run tests to verify they pass**

`cargo test -p classick` — full suite green (new + existing).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/cli.rs crates/classick/src/config.rs crates/classick/src/orchestrator.rs crates/classick/src/scan.rs crates/classick/src/lib.rs
git commit -m "feat(scan): --scan-library mode — incremental tag scan into library-index.json"
```

---

### Task 5: apply the selection in the sync path

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (in `run`, immediately after `let sources = preflight::walk_source(...)` at line ~117)
- Modify: `crates/classick/src/selection.rs` (add `apply_to_sources` orchestration helper + test)

**Interfaces:**
- Produces: `selection::apply_to_sources(sources: Vec<SourceEntry>, source_root: &Path, progress_log: impl Fn(String)) -> Vec<SourceEntry>` — loads selection + index from default paths, filters, saves index if dirty
- Consumes: Tasks 1–3

- [ ] **Step 1: Write the failing test** (in `selection.rs` tests — path-injected variant so no default-path pollution)

The default-path helper is a thin composition; test the path-injected core:

```rust
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
            album: "Amber".into(), genre: "IDM".into(), title: String::new(), duration_ms: 0,
        }), |_msg| {});
        assert_eq!(kept.len(), 1);

        // The inline probe must have been persisted.
        let idx = crate::library_index::load_or_empty(&idx_path, &root);
        assert_eq!(idx.files.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }
```

- [ ] **Step 2: Run to verify it fails**

`cargo test -p classick selection::` — COMPILE ERROR (`apply_with_paths` missing).

- [ ] **Step 3: Implement** (append to `selection.rs`)

```rust
/// Sync-path entry point: load selection + index from their default paths,
/// filter, persist inline-probe additions. mode=All is a zero-cost
/// passthrough (no index load, no writes).
pub fn apply_to_sources(
    sources: Vec<SourceEntry>,
    source_root: &std::path::Path,
    progress_log: impl Fn(String),
) -> Vec<SourceEntry> {
    let sel_path = match default_selection_path() {
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
```

`apply_loop.rs` — one insertion after `let sources = preflight::walk_source(config, progress, decision_rx)?;` (line ~117):

```rust
    let sources = crate::selection::apply_to_sources(
        sources,
        &config.source,
        |msg| progress.log(msg),
    );
```

(Everything downstream — diff, summary counts, review flow, `no_delete`, the daemon's `library_count` refresh from summary — picks the filtered list up automatically. This is the whole integration.)

- [ ] **Step 4: Run the full core suite**

`cargo test -p classick` — green. Also `cargo build --release` compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/selection.rs crates/classick/src/apply_loop.rs
git commit -m "feat(apply-loop): honor the user's selection when planning a sync"
```

---

## Stage B — Daemon + protocol v1.4.0

### Task 6: daemon wire types + protocol doc

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs`
- Modify: `docs/ipc-protocol.md` (append "Daemon v1.4.0" section)

**Interfaces:**
- Produces: `DAEMON_PROTOCOL_VERSION = "1.4.0"`; `DaemonStateLabel::Scanning`; `DaemonEvent::{LibraryUpdate, SelectionUpdate, SelectionPreview}`; `DaemonCommand::{GetLibrary, ScanLibrary, GetSelection, SaveSelection, PreviewSelection}`; wire structs `LibraryArtist { name, albums }`, `LibraryAlbum { name, genre: Option<String>, tracks, bytes }`, `LibraryGenre { name, tracks, bytes }`
- Consumes: `crate::selection::{SelectionMode, SelectionRule}` (already `Serialize + Deserialize`)

- [ ] **Step 1: Write the failing tests** (in `ipc_daemon.rs` tests)

```rust
    #[test]
    fn protocol_version_is_1_4_0() {
        assert_eq!(DAEMON_PROTOCOL_VERSION, "1.4.0");
    }

    #[test]
    fn new_selection_commands_deserialize() {
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"get_library"}"#).unwrap(),
            DaemonCommand::GetLibrary));
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"scan_library"}"#).unwrap(),
            DaemonCommand::ScanLibrary));
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"get_selection"}"#).unwrap(),
            DaemonCommand::GetSelection));

        let save: DaemonCommand = serde_json::from_str(
            r#"{"type":"save_selection","mode":"include","rules":[
                {"kind":"artist","name":"Boards of Canada"},
                {"kind":"album","artist":"Aphex Twin","album":"Drukqs"},
                {"kind":"genre","name":"Ambient"}]}"#).unwrap();
        match save {
            DaemonCommand::SaveSelection { mode, rules } => {
                assert_eq!(mode, crate::selection::SelectionMode::Include);
                assert_eq!(rules.len(), 3);
            }
            _ => panic!("expected SaveSelection"),
        }

        let preview: DaemonCommand = serde_json::from_str(
            r#"{"type":"preview_selection","mode":"exclude","rules":[]}"#).unwrap();
        assert!(matches!(preview, DaemonCommand::PreviewSelection { .. }));
    }

    #[test]
    fn library_update_serializes_aggregated_shape() {
        let evt = DaemonEvent::LibraryUpdate {
            source_root: Some("/music".into()),
            scanned_at_unix_secs: Some(42),
            artists: vec![LibraryArtist {
                name: "Aphex Twin".into(),
                albums: vec![LibraryAlbum {
                    name: "Drukqs".into(), genre: Some("IDM".into()), tracks: 30, bytes: 900,
                }],
            }],
            genres: vec![LibraryGenre { name: "IDM".into(), tracks: 30, bytes: 900 }],
            total_tracks: 30,
            total_bytes: 900,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"library_update""#));
        assert!(json.contains(r#""scanned_at_unix_secs":42"#));
        assert!(json.contains(r#""albums""#));
    }

    #[test]
    fn library_update_never_scanned_serializes_null_timestamp() {
        let evt = DaemonEvent::LibraryUpdate {
            source_root: None, scanned_at_unix_secs: None,
            artists: vec![], genres: vec![], total_tracks: 0, total_bytes: 0,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""scanned_at_unix_secs":null"#),
            "null (not omitted) — the UI branches on it for the never-scanned state");
    }

    #[test]
    fn selection_update_and_preview_serialize() {
        let upd = DaemonEvent::SelectionUpdate {
            mode: crate::selection::SelectionMode::Exclude,
            rules: vec![crate::selection::SelectionRule::Genre { name: "Podcast".into() }],
        };
        let json = serde_json::to_string(&upd).unwrap();
        assert!(json.contains(r#""type":"selection_update""#));
        assert!(json.contains(r#""mode":"exclude""#));

        let prev = DaemonEvent::SelectionPreview {
            selected_tracks: 2340, selected_bytes: 14_200_000_000,
            adds: 120, removes: 214,
        };
        let json = serde_json::to_string(&prev).unwrap();
        assert!(json.contains(r#""type":"selection_preview""#));
        assert!(json.contains(r#""removes":214"#));
    }

    #[test]
    fn scanning_state_label_serializes() {
        let s = serde_json::to_string(&DaemonStateLabel::Scanning).unwrap();
        assert_eq!(s, r#""scanning""#);
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p classick ipc_daemon::` → COMPILE ERROR.

- [ ] **Step 3: Implement**

In `ipc_daemon.rs`:
- `pub const DAEMON_PROTOCOL_VERSION: &str = "1.4.0";`
- `DaemonStateLabel` gains `Scanning`.
- Import `use crate::selection::{SelectionMode, SelectionRule};`
- New wire structs (derive `Debug, Clone, Serialize`):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct LibraryAlbum {
    pub name: String,
    /// Display-only: most common genre among the album's tracks; None on
    /// tie/absence. Genre RULES always match per-track (see spec).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    pub tracks: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryArtist {
    pub name: String,
    pub albums: Vec<LibraryAlbum>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryGenre {
    pub name: String,
    pub tracks: usize,
    pub bytes: u64,
}
```

- New `DaemonEvent` variants:

```rust
    /// Aggregated library index for the Choose Music browser. Never
    /// per-track. `scanned_at_unix_secs: None` (serialized null) = never
    /// scanned — the UI shows its scan-prompt empty state.
    LibraryUpdate {
        source_root: Option<String>,
        scanned_at_unix_secs: Option<u64>,
        artists: Vec<LibraryArtist>,
        genres: Vec<LibraryGenre>,
        total_tracks: usize,
        total_bytes: u64,
    },
    SelectionUpdate {
        mode: SelectionMode,
        rules: Vec<SelectionRule>,
    },
    /// Reply to preview_selection: hypothetical impact vs the manifest.
    /// bytes are SOURCE sizes (an estimate of on-iPod size — label it "~").
    SelectionPreview {
        selected_tracks: usize,
        selected_bytes: u64,
        adds: usize,
        removes: usize,
    },
```

(Note: `LibraryUpdate.scanned_at_unix_secs` must serialize as `null`, so NO `skip_serializing_if` on it — the test pins this.)

- New `DaemonCommand` variants:

```rust
    /// Reply: library_update from the cached index (may be never-scanned).
    GetLibrary,
    /// Spawn a --scan-library subprocess under the shared sync guard.
    /// No-op (log + drop) if busy or no source configured.
    ScanLibrary,
    GetSelection,
    SaveSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
    /// Pure computation; nothing persists. Reply: selection_preview.
    PreviewSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
```

(`SelectionRule` needs `Deserialize` too — it already has it from Task 1. `DaemonEvent` derives `Serialize` only, `DaemonCommand` `Deserialize` only — the selection types derive both, so they slot into either.)

Also update the doc comment at the top of the file: protocol `1.3.0` → `1.4.0`.

- [ ] **Step 4: Update `docs/ipc-protocol.md`** — append after the "Daemon v1.3.0" section:

```markdown
## Daemon v1.4.0 — Library selection: browse, scan, choose what syncs (2026-07-14)

The daemon now emits `hello` with `protocol_version = "1.4.0"`. Purely
additive over v1.3.0: five new commands, three new events, one new
`status_update.state` value, and a semantics clarification for
`library_count`. See
`docs/superpowers/specs/2026-07-14-library-selection-design.md`.

### New commands (UI → daemon)

| Type | Fields | Behavior |
|---|---|---|
| `get_library` | (none) | Replies `library_update` from the cached library index. Never-scanned → `scanned_at_unix_secs: null` + empty collections. |
| `scan_library` | (none) | Spawns `classick --ipc-mode --scan-library` under the same state-machine guard as `trigger_sync`/`backfill_rockbox` (no-op, log + drop, if busy or no source configured). Progress arrives as forwarded subprocess events; on finish the daemon reloads the index and broadcasts a fresh `library_update`. |
| `get_selection` | (none) | Replies `selection_update`. |
| `save_selection` | `mode`, `rules` | Persists selection.json atomically; replies `selection_update`; broadcasts a refreshed `status_update`. |
| `preview_selection` | `mode`, `rules` | Pure computation, no persistence. Replies `selection_preview`. |

`mode` is `"all" | "include" | "exclude"`. Each rule is one of:
`{"kind":"artist","name":…}`, `{"kind":"album","artist":…,"album":…}`,
`{"kind":"genre","name":…}`. Matching is case-insensitive; "artist" means
album_artist falling back to track artist; empty strings are the
Unknown-Artist / No-Genre buckets.

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `library_update` | `source_root` (str?), `scanned_at_unix_secs` (u64 \| null; null = never scanned), `artists[]` — `{name, albums[]: {name, genre?, tracks, bytes}}` — `genres[]: {name, tracks, bytes}`, `total_tracks`, `total_bytes`. Aggregated, never per-track. An album's `genre` is display-only (most common among its tracks; omitted on tie/absence); genre rules match per-track. |
| `selection_update` | `mode`, `rules` — mirror of selection.json. |
| `selection_preview` | `selected_tracks`, `selected_bytes` (source bytes — an estimate of on-iPod size), `adds`, `removes` (vs the manifest). |

### `status_update.state` gains `"scanning"`

Emitted while a library scan subprocess runs. **Clients MUST treat unknown
`state` values as `idle`** — this is the standing rule for all future state
additions, matching §2's unknown-message tolerance.

### `library_count` semantics

`status_update.library_count` is now the **selected** track count — the "Y"
in "X of Y synced" is what the current selection wants on the iPod, not the
raw folder count. Under `mode: "all"` the value is unchanged.
```

Also update the compatibility matrix (§11 daemon note) if the implementer touches it — the daemon-namespace note at the top of the v1.1.0 daemon section says "as of this writing the daemon protocol is at 1.3.0"; bump that sentence to 1.4.0.

- [ ] **Step 5: Run tests, commit**

`cargo test -p classick ipc_daemon::` — PASS (existing `hello_serializes_with_protocol_version` test asserts `1.3.0`; update it to `1.4.0`).

```bash
git add crates/classick/src/ipc_daemon.rs docs/ipc-protocol.md
git commit -m "feat(ipc): daemon protocol v1.4.0 — library/selection commands + events, scanning state"
```

---

### Task 7: `daemon/library.rs` — aggregation, preview, selected count

**Files:**
- Create: `crates/classick/src/daemon/library.rs`
- Modify: `crates/classick/src/daemon/mod.rs` (add `pub mod library;`)

**Interfaces:**
- Produces:
  - `aggregate(index: &LibraryIndex) -> (Vec<LibraryArtist>, Vec<LibraryGenre>, usize, u64)` — artists sorted case-insensitively, albums grouped by `(effective_artist, album)`, album `genre` = most-common (None on tie), genres aggregated per-track
  - `build_library_update(config_path: &Path) -> DaemonEvent` — resolves source from persisted config, loads index from `library_index::default_index_path()`, aggregates
  - `preview(index: &LibraryIndex, manifest: &Manifest, mode: SelectionMode, rules: &[SelectionRule]) -> (usize, u64, usize, usize)` — `(selected_tracks, selected_bytes, adds, removes)`; adds = selected paths not in the manifest (source_known), removes = source_known manifest paths not selected
  - `selected_library_count(config_path: &Path) -> Option<usize>` — None when mode=all (caller falls back to the walk cache) or index unavailable
- Consumes: `crate::selection::{Selection, SelectionMode, SelectionRule, load_or_all, default_selection_path}`, `crate::library_index`, `crate::manifest::Manifest`, `crate::ipc_daemon::{DaemonEvent, LibraryArtist, LibraryAlbum, LibraryGenre}`

- [ ] **Step 1: Write the failing tests** (pure functions — this is the macOS-runnable daemon coverage)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_index::{IndexedTrack, LibraryIndex};
    use crate::selection::{SelectionMode, SelectionRule};
    use std::path::PathBuf;

    fn track(artist: &str, album_artist: &str, album: &str, genre: &str, size: u64) -> IndexedTrack {
        IndexedTrack {
            mtime: 1, size,
            artist: artist.into(), album_artist: album_artist.into(),
            album: album.into(), genre: genre.into(),
            title: String::new(), duration_ms: 0,
        }
    }

    fn index_with(tracks: Vec<(&str, IndexedTrack)>) -> LibraryIndex {
        let mut idx = LibraryIndex::empty(PathBuf::from("/m"));
        for (path, t) in tracks {
            idx.files.insert(PathBuf::from(path), t);
        }
        idx
    }

    #[test]
    fn aggregate_groups_by_effective_artist_and_album() {
        let idx = index_with(vec![
            ("/m/a1.flac", track("Aphex Twin", "", "Drukqs", "IDM", 10)),
            ("/m/a2.flac", track("Aphex Twin", "", "Drukqs", "IDM", 20)),
            ("/m/c1.flac", track("Track Artist", "Various Artists", "Comp", "Pop", 5)),
        ]);
        let (artists, genres, total_tracks, total_bytes) = aggregate(&idx);
        assert_eq!(total_tracks, 3);
        assert_eq!(total_bytes, 35);
        assert_eq!(artists.len(), 2);
        // sorted case-insensitively: Aphex Twin < Various Artists
        assert_eq!(artists[0].name, "Aphex Twin");
        assert_eq!(artists[0].albums.len(), 1);
        assert_eq!(artists[0].albums[0].tracks, 2);
        assert_eq!(artists[0].albums[0].bytes, 30);
        assert_eq!(artists[1].name, "Various Artists", "album_artist wins grouping");
        assert_eq!(genres.iter().find(|g| g.name == "IDM").unwrap().tracks, 2);
    }

    #[test]
    fn aggregate_album_genre_is_majority_none_on_tie() {
        let idx = index_with(vec![
            ("/m/1.flac", track("X", "", "Mixed", "Rock", 1)),
            ("/m/2.flac", track("X", "", "Mixed", "Rock", 1)),
            ("/m/3.flac", track("X", "", "Mixed", "Pop", 1)),
            ("/m/4.flac", track("Y", "", "Tied", "A", 1)),
            ("/m/5.flac", track("Y", "", "Tied", "B", 1)),
        ]);
        let (artists, _, _, _) = aggregate(&idx);
        let mixed = &artists.iter().find(|a| a.name == "X").unwrap().albums[0];
        assert_eq!(mixed.genre.as_deref(), Some("Rock"));
        let tied = &artists.iter().find(|a| a.name == "Y").unwrap().albums[0];
        assert_eq!(tied.genre, None, "tie → no display genre");
    }

    #[test]
    fn aggregate_empty_tags_bucket_as_empty_string_on_wire() {
        let idx = index_with(vec![("/m/u.flac", track("", "", "", "", 7))]);
        let (artists, genres, _, _) = aggregate(&idx);
        assert_eq!(artists[0].name, "", "wire carries raw empty; UI renders 'Unknown Artist'");
        assert_eq!(genres[0].name, "");
    }

    #[test]
    fn preview_counts_adds_and_removes_against_manifest() {
        use crate::manifest::{Manifest, ManifestEntry};
        let idx = index_with(vec![
            ("/m/on_ipod_kept.flac", track("Keep", "", "K", "IDM", 10)),
            ("/m/on_ipod_dropped.flac", track("Drop", "", "D", "Rock", 20)),
            ("/m/new_selected.flac", track("Keep", "", "K2", "IDM", 30)),
        ]);
        let entry = |p: &str| ManifestEntry {
            source_path: PathBuf::from(p), source_mtime: 1, source_size: 1,
            source_fingerprint: String::new(), ipod_dbid: 1,
            ipod_relpath: String::new(), source_known: true,
            audio_fingerprint: String::new(), encoder: "unknown".into(),
            encoder_version: String::new(), source_format: "flac".into(),
        };
        let manifest = Manifest {
            version: 1, ipod_serial: None, last_source_root: None,
            tracks: vec![entry("/m/on_ipod_kept.flac"), entry("/m/on_ipod_dropped.flac")],
        };
        let rules = vec![SelectionRule::Artist { name: "Keep".into() }];
        let (selected_tracks, selected_bytes, adds, removes) =
            preview(&idx, &manifest, SelectionMode::Include, &rules);
        assert_eq!(selected_tracks, 2);
        assert_eq!(selected_bytes, 40);
        assert_eq!(adds, 1, "new_selected.flac isn't on the iPod yet");
        assert_eq!(removes, 1, "on_ipod_dropped.flac is deselected");
    }

    #[test]
    fn preview_mode_all_selects_everything_removes_nothing() {
        use crate::manifest::Manifest;
        let idx = index_with(vec![("/m/a.flac", track("X", "", "A", "G", 10))]);
        let manifest = Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks: vec![] };
        let (selected, _, adds, removes) = preview(&idx, &manifest, SelectionMode::All, &[]);
        assert_eq!(selected, 1);
        assert_eq!(adds, 1);
        assert_eq!(removes, 0);
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p classick daemon::library::` → COMPILE ERROR.

- [ ] **Step 3: Implement**

```rust
//! Daemon-side library services: aggregate the tag index for the Choose
//! Music browser, evaluate selection previews, and derive the selected
//! library count. All rule evaluation delegates to `crate::selection` —
//! the ONE evaluator the sync filter also uses.

use crate::ipc_daemon::{DaemonEvent, LibraryAlbum, LibraryArtist, LibraryGenre};
use crate::library_index::{self, LibraryIndex};
use crate::manifest::Manifest;
use crate::selection::{self, Selection, SelectionMode, SelectionRule};
use std::collections::BTreeMap;
use std::path::Path;

/// Aggregate the per-file index into the browser's artist→album + genre
/// shape. Sorted case-insensitively. Empty tags stay empty strings on the
/// wire (the UI renders "Unknown Artist" / "No Genre").
pub fn aggregate(index: &LibraryIndex) -> (Vec<LibraryArtist>, Vec<LibraryGenre>, usize, u64) {
    // (artist_lower, album_lower) -> (display names, per-genre counts, tracks, bytes)
    struct AlbumAgg {
        artist: String,
        album: String,
        genre_counts: BTreeMap<String, usize>,
        tracks: usize,
        bytes: u64,
    }
    let mut albums: BTreeMap<(String, String), AlbumAgg> = BTreeMap::new();
    let mut genres: BTreeMap<String, (String, usize, u64)> = BTreeMap::new();
    let mut total_tracks = 0usize;
    let mut total_bytes = 0u64;

    for rec in index.files.values() {
        let facts = rec.facts();
        let artist = facts.effective_artist().to_string();
        total_tracks += 1;
        total_bytes += rec.size;

        let key = (artist.to_lowercase(), rec.album.to_lowercase());
        let agg = albums.entry(key).or_insert_with(|| AlbumAgg {
            artist: artist.clone(),
            album: rec.album.clone(),
            genre_counts: BTreeMap::new(),
            tracks: 0,
            bytes: 0,
        });
        agg.tracks += 1;
        agg.bytes += rec.size;
        *agg.genre_counts.entry(rec.genre.clone()).or_insert(0) += 1;

        let g = genres.entry(rec.genre.to_lowercase())
            .or_insert_with(|| (rec.genre.clone(), 0, 0));
        g.1 += 1;
        g.2 += rec.size;
    }

    let mut by_artist: BTreeMap<String, LibraryArtist> = BTreeMap::new();
    for ((artist_key, _), agg) in albums {
        let display_genre = majority_genre(&agg.genre_counts);
        let entry = by_artist.entry(artist_key).or_insert_with(|| LibraryArtist {
            name: agg.artist.clone(),
            albums: Vec::new(),
        });
        entry.albums.push(LibraryAlbum {
            name: agg.album,
            genre: display_genre,
            tracks: agg.tracks,
            bytes: agg.bytes,
        });
    }
    let artists: Vec<LibraryArtist> = by_artist.into_values().collect();
    let genres: Vec<LibraryGenre> = genres.into_values()
        .map(|(name, tracks, bytes)| LibraryGenre { name, tracks, bytes })
        .collect();
    (artists, genres, total_tracks, total_bytes)
}

/// Most common genre in the album, None on tie or when all are empty-tag.
fn majority_genre(counts: &BTreeMap<String, usize>) -> Option<String> {
    let mut best: Option<(&String, usize)> = None;
    let mut tied = false;
    for (g, &n) in counts {
        match best {
            Some((_, bn)) if n > bn => { best = Some((g, n)); tied = false; }
            Some((_, bn)) if n == bn => { tied = true; }
            None => { best = Some((g, n)); }
            _ => {}
        }
    }
    match best {
        Some((g, _)) if !tied && !g.is_empty() => Some(g.clone()),
        _ => None,
    }
}

/// Build the get_library / post-scan broadcast payload from disk state.
pub fn build_library_update(config_path: &Path) -> DaemonEvent {
    let source = crate::config_file::load(config_path).ok().flatten().and_then(|c| c.source);
    let (index, scanned_at) = match (&source, library_index::default_index_path()) {
        (Some(root), Ok(idx_path)) => {
            let idx = library_index::load_or_empty(&idx_path, root);
            let ts = idx.scanned_at_unix_secs;
            (Some(idx), ts)
        }
        _ => (None, None),
    };
    match index {
        Some(idx) => {
            let (artists, genres, total_tracks, total_bytes) = aggregate(&idx);
            DaemonEvent::LibraryUpdate {
                source_root: source.map(|p| p.display().to_string()),
                scanned_at_unix_secs: scanned_at,
                artists, genres, total_tracks, total_bytes,
            }
        }
        None => DaemonEvent::LibraryUpdate {
            source_root: source.map(|p| p.display().to_string()),
            scanned_at_unix_secs: None,
            artists: Vec::new(), genres: Vec::new(), total_tracks: 0, total_bytes: 0,
        },
    }
}

/// Hypothetical impact of `(mode, rules)`: (selected_tracks, selected_bytes,
/// adds, removes). Bytes are source sizes (estimate). Adds/removes compare
/// against the manifest's source_known entries — the same set the diff
/// operates on.
pub fn preview(
    index: &LibraryIndex,
    manifest: &Manifest,
    mode: SelectionMode,
    rules: &[SelectionRule],
) -> (usize, u64, usize, usize) {
    let sel = Selection { version: selection::SELECTION_VERSION, mode, rules: rules.to_vec() };
    let mut selected_tracks = 0usize;
    let mut selected_bytes = 0u64;
    let mut selected_paths = std::collections::HashSet::new();
    for (path, rec) in &index.files {
        if sel.wants(&rec.facts()) {
            selected_tracks += 1;
            selected_bytes += rec.size;
            selected_paths.insert(path.clone());
        }
    }
    let manifest_paths: std::collections::HashSet<_> = manifest.tracks.iter()
        .filter(|e| e.source_known)
        .map(|e| e.source_path.clone())
        .collect();
    let adds = selected_paths.iter().filter(|p| !manifest_paths.contains(*p)).count();
    let removes = manifest_paths.iter().filter(|p| !selected_paths.contains(*p)).count();
    (selected_tracks, selected_bytes, adds, removes)
}

/// The selected library count (Y in "X of Y synced"). None when the mode is
/// All (caller keeps using its walk-based cache) or when no index/source is
/// available.
pub fn selected_library_count(config_path: &Path) -> Option<usize> {
    let sel_path = selection::default_selection_path().ok()?;
    let sel = selection::load_or_all(&sel_path);
    if sel.mode == SelectionMode::All {
        return None;
    }
    let source = crate::config_file::load(config_path).ok().flatten()?.source?;
    let idx_path = library_index::default_index_path().ok()?;
    let idx = library_index::load_or_empty(&idx_path, &source);
    Some(idx.files.values().filter(|rec| sel.wants(&rec.facts())).count())
}
```

Add `pub mod library;` to `crates/classick/src/daemon/mod.rs`.

- [ ] **Step 4: Run tests** — `cargo test -p classick daemon::library::` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/library.rs crates/classick/src/daemon/mod.rs
git commit -m "feat(daemon): library aggregation, selection preview, selected library count"
```

---

### Task 8: scan session plumbing — SessionKind + scan subprocess command

**Files:**
- Modify: `crates/classick/src/daemon/state.rs` (SessionKind on SyncSession)
- Modify: `crates/classick/src/daemon/sync_orchestrator.rs` (`build_scan_command`, `run_scan`)

**Interfaces:**
- Produces: `SessionKind { Sync, Scan }` (`SyncSession.kind`), `StateMachine::try_start_scan() -> TriggerOutcome`, `sync_orchestrator::build_scan_command(exe) -> Command`, `sync_orchestrator::run_scan(exe, cancel_rx, pause_rx, prompt_rx, event_tx) -> Result<OrchestratorOutcome>`
- Consumes: existing `base_command` / `drive_child` machinery

- [ ] **Step 1: Write the failing tests**

`state.rs` tests:

```rust
    #[test]
    fn scan_session_carries_scan_kind_and_shares_the_guard() {
        let mut sm = StateMachine::new();
        assert_eq!(sm.try_start_scan(), TriggerOutcome::Accepted);
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.kind, SessionKind::Scan);
        } else { panic!("expected Syncing (shared guard)"); }
        // A sync while scanning is dropped — the guard is shared.
        assert_eq!(sm.try_start_sync(SyncTrigger::Manual), TriggerOutcome::DroppedAlreadySyncing);
        sm.finish_sync();
        assert!(sm.is_idle());
    }

    #[test]
    fn sync_sessions_default_to_sync_kind() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::Manual);
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.kind, SessionKind::Sync);
        } else { panic!(); }
    }
```

`sync_orchestrator.rs` tests:

```rust
    #[test]
    fn build_scan_command_passes_scan_flag_without_ipod() {
        let cmd = build_scan_command(&PathBuf::from("classick"));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--scan-library"));
        assert!(!dbg.contains("--ipod"), "a scan involves no device");
        assert!(!dbg.contains("--apply"));
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p classick daemon::state:: daemon::sync_orchestrator::` → COMPILE ERROR.

- [ ] **Step 3: Implement**

`state.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Sync,
    Scan,
}
```

Add `pub kind: SessionKind,` to `SyncSession`. `try_start_sync_inner` gains a `kind: SessionKind` parameter; `try_start_sync`/`try_start_sync_for_device` pass `SessionKind::Sync`; add:

```rust
    /// A library scan occupies the same guard as a sync — they never run
    /// concurrently (no SMB contention, no index-file races).
    pub fn try_start_scan(&mut self) -> TriggerOutcome {
        self.try_start_sync_inner(SyncTrigger::Manual, None, None, SessionKind::Scan)
    }
```

Fix the existing tests' struct literals if any construct `SyncSession` directly (none do — they go through the methods).

`sync_orchestrator.rs` — refactor `base_command` to take the drive optionally, keeping the two existing builders' output identical:

```rust
fn base_command(exe: &std::path::Path, mode_flag: &str, drive: Option<&str>) -> Command {
    use crate::windows_proc::NoConsoleWindow;
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode").arg(mode_flag);
    if let Some(d) = drive {
        cmd.arg("--ipod").arg(d);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .no_console();
    cmd
}
```

Update the two call sites (`build_command`: `base_command(exe, "--apply", Some(drive))`; `build_backfill_command`: `base_command(exe, "--backfill-rockbox", Some(drive))`). Add:

```rust
/// Build the library-scan subprocess command. No --ipod: a scan only reads
/// the source tree and writes the index cache.
pub fn build_scan_command(exe: &std::path::Path) -> Command {
    base_command(exe, "--scan-library", None)
}

/// Run a --scan-library subprocess through the same drive-to-completion
/// machinery as syncs/backfills (event forwarding, cancel/pause, bail
/// threshold — mostly inert for a scan, but shared code is shared behavior).
pub async fn run_scan(
    exe: PathBuf,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<OrchestratorOutcome> {
    let cmd = build_scan_command(&exe);
    drive_child(exe, cmd, cancel_rx, pause_rx, prompt_decisions_rx, event_tx).await
}
```

- [ ] **Step 4: Run tests** — `cargo test -p classick daemon::` → PASS (including untouched existing command tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/state.rs crates/classick/src/daemon/sync_orchestrator.rs
git commit -m "feat(daemon): scan sessions share the sync guard; --scan-library subprocess command"
```

---

### Task 9: runtime wiring — command arms, ScanCompleted, scanning label, selected count

**Files:**
- Modify: `crates/classick/src/daemon/runtime.rs`

**Interfaces:**
- Produces: handled `DaemonCommand::{GetLibrary, ScanLibrary, GetSelection, SaveSelection, PreviewSelection}` arms; `InternalEvent::ScanCompleted`; `DaemonDeps.spawn_scan: SpawnFn`; scanning state label in every `StatusUpdate` emission; selection-aware `library_count`
- Consumes: Tasks 6–8; `daemon::library::{build_library_update, preview, selected_library_count}`; `selection::{load_or_all, save_atomic, default_selection_path}`

This task is wiring in an existing 1200-line file; there is no new unit-testable seam beyond one helper. Work in this order:

- [ ] **Step 1: Write the failing test for the one extractable pure helper**

```rust
    // In runtime.rs tests: the state→label mapping including scan kind.
    #[test]
    fn state_label_maps_scan_sessions_to_scanning() {
        let mut sm = StateMachine::new();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Idle));
        sm.try_start_scan();
        assert!(matches!(state_label(&sm), DaemonStateLabel::Scanning));
        sm.finish_sync();
        sm.try_start_sync(SyncTrigger::Manual);
        assert!(matches!(state_label(&sm), DaemonStateLabel::Syncing));
    }
```

- [ ] **Step 2: Run to verify failure** — COMPILE ERROR (`state_label` missing).

- [ ] **Step 3: Implement**

1. Extract the label mapping (both `broadcast_status` and the `GetStatus` arm currently inline it):

```rust
fn state_label(state: &StateMachine) -> DaemonStateLabel {
    match state.state() {
        DaemonState::Idle => DaemonStateLabel::Idle,
        DaemonState::Syncing(s) if s.kind == crate::daemon::state::SessionKind::Scan => {
            DaemonStateLabel::Scanning
        }
        DaemonState::Syncing(_) => DaemonStateLabel::Syncing,
    }
}
```

Replace both inline `match state.state()` label computations with `state_label(state)`. Import `SessionKind`.

2. `DaemonDeps` gains `pub spawn_scan: SpawnFn,` (drive argument is ignored by the closure). In `run_daemon()`:

```rust
    let exe_for_scan = std::env::current_exe()?;
    let event_tx_for_scan = event_tx.clone();
    let spawn_scan: SpawnFn = Arc::new(move |_drive: String, cancel_rx, pause_rx, prompt_rx| {
        let exe = exe_for_scan.clone();
        let event_tx = event_tx_for_scan.clone();
        Box::pin(async move {
            sync_orchestrator::run_scan(exe, cancel_rx, pause_rx, prompt_rx, event_tx).await
        })
    });
```

Pass it in `DaemonDeps { .. spawn_scan, .. }`. Update every `DaemonDeps` construction in tests (`tests/daemon_runtime_integration.rs` sandbox — Windows-gated; still must compile on Windows CI: give it a stub closure identical to `spawn_sync`'s fake).

3. `InternalEvent` gains:

```rust
    /// A --scan-library subprocess finished. No history entry — a scan is
    /// cache maintenance, not a sync.
    ScanCompleted {
        outcome: Result<OrchestratorOutcome>,
    },
```

4. New `start_scan_session` (mirrors `start_sync_session`, minus device/serial and history bookkeeping):

```rust
fn start_scan_session(
    state: &mut StateMachine,
    event_tx: &broadcast::Sender<DaemonEvent>,
    spawn_scan: &SpawnFn,
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    cancel_tx_holder: &mut Option<oneshot::Sender<()>>,
    prompt_tx_holder: &mut Option<mpsc::UnboundedSender<(u64, i32)>>,
    pause_tx_holder: &mut Option<oneshot::Sender<()>>,
    connected: &Option<DetectedIpod>,
    config_path: &std::path::Path,
    history: &HistoryService,
    library_count_cache: Option<usize>,
) {
    if state.try_start_scan() != TriggerOutcome::Accepted {
        return;
    }
    broadcast_status(event_tx, state, connected, config_path, history, library_count_cache);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    *cancel_tx_holder = Some(cancel_tx);
    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<(u64, i32)>();
    *prompt_tx_holder = Some(prompt_tx);
    let (pause_tx, pause_rx) = oneshot::channel::<()>();
    *pause_tx_holder = Some(pause_tx);

    let spawn_scan = spawn_scan.clone();
    let internal_tx = internal_tx.clone();
    tokio::spawn(async move {
        let outcome = (spawn_scan)(String::new(), cancel_rx, pause_rx, prompt_rx).await;
        let _ = internal_tx.send(InternalEvent::ScanCompleted { outcome });
    });
}
```

5. `handle_internal_event` — clear the holders for `ScanCompleted` too (extend the `is_sync_completion` check to `matches!(internal, InternalEvent::SyncCompleted { .. } | InternalEvent::ScanCompleted { .. })` in the select loop), and add the arm:

```rust
        InternalEvent::ScanCompleted { outcome } => {
            if let Err(e) = &outcome {
                tracing::warn!("daemon: library scan failed: {e:#}");
            }
            state.finish_sync();
            // Fresh index on disk: rebroadcast the library and a status
            // update (selection-aware count may have changed).
            let _ = event_tx.send(crate::daemon::library::build_library_update(config_path));
            broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
        }
```

(`handle_internal_event` needs `history: &HistoryService` — it already has it.)

6. `broadcast_status` + the `GetStatus` arm: selection-aware Y. Two exact spots: (a) at the top of `broadcast_status`'s body, shadow its `library_count: Option<usize>` parameter; (b) in the `GetStatus` arm, compute the same shadow before constructing the `StatusUpdate` (which currently reads `*library_count_cache`):

```rust
    let library_count = crate::daemon::library::selected_library_count(config_path)
        .or(library_count);            // (a); in (b): .or(*library_count_cache)
```

(`selected_library_count` returns `None` under mode=All → existing walk-cache behavior is untouched. Leave `start_sync_session`'s StatusUpdate as-is — it's a momentary "syncing started" push and the next broadcast corrects Y.)

7. New command arms in `handle_client_command` (before `Shutdown`):

```rust
        DaemonCommand::GetLibrary => {
            let _ = reply.send(crate::daemon::library::build_library_update(config_path));
        }
        DaemonCommand::ScanLibrary => {
            if !state.is_idle() {
                tracing::debug!("daemon: client {client_id} sent scan_library while busy; dropped");
                return false;
            }
            let has_source = config_file::load(config_path).ok().flatten()
                .and_then(|c| c.source).is_some();
            if !has_source {
                tracing::debug!("daemon: client {client_id} sent scan_library but no source configured; dropped");
                return false;
            }
            tracing::info!("daemon: client {client_id} triggered a library scan");
            start_scan_session(
                state, event_tx, spawn_scan, internal_tx,
                cancel_tx_holder, prompt_tx_holder, pause_tx_holder,
                connected, config_path, history, *library_count_cache,
            );
        }
        DaemonCommand::GetSelection => {
            let sel = crate::selection::default_selection_path()
                .map(|p| crate::selection::load_or_all(&p))
                .unwrap_or_else(|_| crate::selection::Selection::all());
            let _ = reply.send(DaemonEvent::SelectionUpdate { mode: sel.mode, rules: sel.rules });
        }
        DaemonCommand::SaveSelection { mode, rules } => {
            let sel = crate::selection::Selection {
                version: crate::selection::SELECTION_VERSION,
                mode,
                rules,
            };
            match crate::selection::default_selection_path() {
                Ok(path) => {
                    if let Err(e) = crate::selection::save_atomic(&path, &sel) {
                        tracing::error!("daemon: failed to save selection: {e:#}");
                        return false;
                    }
                }
                Err(e) => {
                    tracing::error!("daemon: cannot resolve selection path: {e:#}");
                    return false;
                }
            }
            let _ = reply.send(DaemonEvent::SelectionUpdate { mode: sel.mode, rules: sel.rules });
            // Y in "X of Y" likely changed; push a fresh status to everyone.
            broadcast_status(event_tx, state, connected, config_path, history, *library_count_cache);
        }
        DaemonCommand::PreviewSelection { mode, rules } => {
            let source = config_file::load(config_path).ok().flatten().and_then(|c| c.source);
            let index = match (source, crate::library_index::default_index_path()) {
                (Some(root), Ok(p)) => crate::library_index::load_or_empty(&p, &root),
                _ => crate::library_index::LibraryIndex::empty(std::path::PathBuf::new()),
            };
            let manifest = crate::config::default_manifest_path()
                .and_then(|p| crate::manifest::load_or_default(&p))
                .unwrap_or_else(|_| crate::manifest::Manifest::empty());
            let (selected_tracks, selected_bytes, adds, removes) =
                crate::daemon::library::preview(&index, &manifest, mode, &rules);
            let _ = reply.send(DaemonEvent::SelectionPreview {
                selected_tracks, selected_bytes, adds, removes,
            });
        }
```

`handle_client_command`'s signature gains `spawn_scan: &SpawnFn` (thread it from the select loop, next to `spawn_backfill`).

- [ ] **Step 4: Compile + test**

`cargo test -p classick` — green (the `state_label` test + all existing). On Windows CI the integration suite must also still compile: the sandbox's `DaemonDeps` needs the new `spawn_scan` field (stub like its `spawn_sync` fake). If developing on macOS, run `cargo check --tests` at minimum and note the Windows suite for CI.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/runtime.rs crates/classick/tests/daemon_runtime_integration.rs
git commit -m "feat(daemon): wire library/selection commands, scan sessions, selection-aware library count"
```

---

## Stage C — macOS app

### Task 10: Swift wire models — new commands/events + lenient state decode

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift`
- Test: `ui/macos/Tests/ClassickTests/WireCodecTests.swift`

**Interfaces:**
- Produces: `SelectionMode { all, include, exclude }`, `SelectionRule { artist(name:), album(artist:album:), genre(name:) }` (Codable on `kind`), `LibraryAlbum`, `LibraryArtist`, `LibraryGenre`, `LibraryInfo`, `SelectionPreviewInfo`; `DaemonCommand.{getLibrary, scanLibrary, getSelection, saveSelection(mode:rules:), previewSelection(mode:rules:)}`; `DaemonEvent.{libraryUpdate(LibraryInfo), selectionUpdate(mode:rules:), selectionPreview(SelectionPreviewInfo)}`; `StatusInfo.State.scanning` + unknown-state-→-idle decoding
- Consumes: wire shapes from Task 6 (field names verbatim)

- [ ] **Step 1: Write the failing tests** (append to `WireCodecTests.swift`, following its existing style)

```swift
    func testDecodesLibraryUpdate() throws {
        let line = #"{"type":"library_update","source_root":"/music","scanned_at_unix_secs":42,"artists":[{"name":"Aphex Twin","albums":[{"name":"Drukqs","genre":"IDM","tracks":30,"bytes":900}]}],"genres":[{"name":"IDM","tracks":30,"bytes":900}],"total_tracks":30,"total_bytes":900}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .libraryUpdate(info) = event else { return XCTFail("expected libraryUpdate, got \(event)") }
        XCTAssertEqual(info.sourceRoot, "/music")
        XCTAssertEqual(info.scannedAtUnixSecs, 42)
        XCTAssertEqual(info.artists.first?.name, "Aphex Twin")
        XCTAssertEqual(info.artists.first?.albums.first?.tracks, 30)
        XCTAssertEqual(info.genres.first?.name, "IDM")
    }

    func testDecodesLibraryUpdateNeverScanned() throws {
        let line = #"{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .libraryUpdate(info) = event else { return XCTFail() }
        XCTAssertNil(info.scannedAtUnixSecs, "null timestamp = never scanned")
    }

    func testDecodesSelectionUpdateAndPreview() throws {
        let upd = #"{"type":"selection_update","mode":"include","rules":[{"kind":"artist","name":"BoC"},{"kind":"album","artist":"Aphex Twin","album":"Drukqs"},{"kind":"genre","name":"Ambient"}]}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(upd.utf8))
        guard case let .selectionUpdate(mode, rules) = event else { return XCTFail() }
        XCTAssertEqual(mode, .include)
        XCTAssertEqual(rules, [
            .artist(name: "BoC"),
            .album(artist: "Aphex Twin", album: "Drukqs"),
            .genre(name: "Ambient"),
        ])

        let prev = #"{"type":"selection_preview","selected_tracks":2340,"selected_bytes":14200000000,"adds":120,"removes":214}"#
        let event2 = try JSONDecoder().decode(DaemonEvent.self, from: Data(prev.utf8))
        guard case let .selectionPreview(info) = event2 else { return XCTFail() }
        XCTAssertEqual(info.removes, 214)
    }

    func testEncodesSelectionCommands() throws {
        func encode(_ cmd: DaemonCommand) throws -> String {
            String(decoding: try JSONEncoder().encode(cmd), as: UTF8.self)
        }
        XCTAssertTrue(try encode(.getLibrary).contains(#""type":"get_library""#))
        XCTAssertTrue(try encode(.scanLibrary).contains(#""type":"scan_library""#))
        XCTAssertTrue(try encode(.getSelection).contains(#""type":"get_selection""#))
        let save = try encode(.saveSelection(mode: .include, rules: [.artist(name: "BoC")]))
        XCTAssertTrue(save.contains(#""type":"save_selection""#))
        XCTAssertTrue(save.contains(#""mode":"include""#))
        XCTAssertTrue(save.contains(#""kind":"artist""#))
        let preview = try encode(.previewSelection(mode: .exclude, rules: []))
        XCTAssertTrue(preview.contains(#""type":"preview_selection""#))
    }

    func testStatusUpdateScanningState() throws {
        let line = #"{"type":"status_update","state":"scanning","configured":true,"ipod_connected":false,"synced_count":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .statusUpdate(info) = event else { return XCTFail() }
        XCTAssertEqual(info.state, .scanning)
    }

    func testStatusUpdateUnknownStateDecodesAsIdle() throws {
        // Protocol rule: unknown state values MUST be treated as idle —
        // without this the whole status_update fails to decode and the
        // menu freezes on stale state when a newer daemon speaks.
        let line = #"{"type":"status_update","state":"defragging","configured":true,"ipod_connected":false,"synced_count":0}"#
        let event = try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
        guard case let .statusUpdate(info) = event else { return XCTFail("must not throw") }
        XCTAssertEqual(info.state, .idle)
    }
```

- [ ] **Step 2: Run to verify failure** — `cd ui/macos && swift test --filter WireCodecTests` → COMPILE ERROR.

- [ ] **Step 3: Implement in `WireModels.swift`**

New types (after `StatusInfo`):

```swift
// MARK: - Library selection (daemon protocol v1.4.0)

enum SelectionMode: String, Codable, Equatable, Sendable {
    case all, include, exclude
}

enum SelectionRule: Codable, Equatable, Hashable, Sendable {
    // Hashable is declared here (synthesized) rather than retroactively in
    // the test target — Swift 6 rejects cross-module retroactive
    // conformances without @retroactive, and the tests use Set([rules]).
    case artist(name: String)
    case album(artist: String, album: String)
    case genre(name: String)

    private enum CodingKeys: String, CodingKey {
        case kind, name, artist, album
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "artist": self = .artist(name: try c.decode(String.self, forKey: .name))
        case "album": self = .album(
            artist: try c.decode(String.self, forKey: .artist),
            album: try c.decode(String.self, forKey: .album))
        case "genre": self = .genre(name: try c.decode(String.self, forKey: .name))
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c,
                debugDescription: "unknown rule kind \(other)")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .artist(name):
            try c.encode("artist", forKey: .kind)
            try c.encode(name, forKey: .name)
        case let .album(artist, album):
            try c.encode("album", forKey: .kind)
            try c.encode(artist, forKey: .artist)
            try c.encode(album, forKey: .album)
        case let .genre(name):
            try c.encode("genre", forKey: .kind)
            try c.encode(name, forKey: .name)
        }
    }
}

struct LibraryAlbum: Codable, Equatable, Sendable {
    var name: String
    var genre: String?
    var tracks: Int
    var bytes: UInt64
}

struct LibraryArtist: Codable, Equatable, Sendable {
    var name: String
    var albums: [LibraryAlbum]
}

struct LibraryGenre: Codable, Equatable, Sendable {
    var name: String
    var tracks: Int
    var bytes: UInt64
}

struct LibraryInfo: Equatable, Sendable {
    var sourceRoot: String?
    var scannedAtUnixSecs: UInt64?
    var artists: [LibraryArtist]
    var genres: [LibraryGenre]
    var totalTracks: Int
    var totalBytes: UInt64
}

struct SelectionPreviewInfo: Equatable, Sendable {
    var selectedTracks: Int
    var selectedBytes: UInt64
    var adds: Int
    var removes: Int
}
```

`StatusInfo.State`: add `case scanning`.

`DaemonCommand`: add cases + encoding arms (extend `CodingKeys` with `mode`, `rules`):

```swift
    case getLibrary
    case scanLibrary
    case getSelection
    case saveSelection(mode: SelectionMode, rules: [SelectionRule])
    case previewSelection(mode: SelectionMode, rules: [SelectionRule])
```

```swift
        case .getLibrary:
            try container.encode("get_library", forKey: .type)
        case .scanLibrary:
            try container.encode("scan_library", forKey: .type)
        case .getSelection:
            try container.encode("get_selection", forKey: .type)
        case let .saveSelection(mode, rules):
            try container.encode("save_selection", forKey: .type)
            try container.encode(mode, forKey: .mode)
            try container.encode(rules, forKey: .rules)
        case let .previewSelection(mode, rules):
            try container.encode("preview_selection", forKey: .type)
            try container.encode(mode, forKey: .mode)
            try container.encode(rules, forKey: .rules)
```

`DaemonEvent`: add cases `libraryUpdate(LibraryInfo)`, `selectionUpdate(mode: SelectionMode, rules: [SelectionRule])`, `selectionPreview(SelectionPreviewInfo)`. Extend `CodingKeys` with `sourceRoot = "source_root"`, `scannedAtUnixSecs = "scanned_at_unix_secs"`, `artists`, `genres`, `totalTracks = "total_tracks"`, `totalBytes = "total_bytes"`, `mode`, `rules`, `selectedTracks = "selected_tracks"`, `selectedBytes = "selected_bytes"`, `adds`, `removes`. Decoder arms:

```swift
        case "library_update":
            self = .libraryUpdate(LibraryInfo(
                sourceRoot: try container.decodeIfPresent(String.self, forKey: .sourceRoot),
                scannedAtUnixSecs: try container.decodeIfPresent(UInt64.self, forKey: .scannedAtUnixSecs),
                artists: try container.decodeIfPresent([LibraryArtist].self, forKey: .artists) ?? [],
                genres: try container.decodeIfPresent([LibraryGenre].self, forKey: .genres) ?? [],
                totalTracks: try container.decodeIfPresent(Int.self, forKey: .totalTracks) ?? 0,
                totalBytes: try container.decodeIfPresent(UInt64.self, forKey: .totalBytes) ?? 0))
        case "selection_update":
            self = .selectionUpdate(
                mode: try container.decodeIfPresent(SelectionMode.self, forKey: .mode) ?? .all,
                rules: try container.decodeIfPresent([SelectionRule].self, forKey: .rules) ?? [])
        case "selection_preview":
            self = .selectionPreview(SelectionPreviewInfo(
                selectedTracks: try container.decodeIfPresent(Int.self, forKey: .selectedTracks) ?? 0,
                selectedBytes: try container.decodeIfPresent(UInt64.self, forKey: .selectedBytes) ?? 0,
                adds: try container.decodeIfPresent(Int.self, forKey: .adds) ?? 0,
                removes: try container.decodeIfPresent(Int.self, forKey: .removes) ?? 0))
```

**Lenient state decode** — in the `"status_update"` arm, replace
`let state = try container.decode(StatusInfo.State.self, forKey: .state)` with:

```swift
            // Unknown state values MUST decode as .idle (protocol §Daemon
            // v1.4.0) — a hard decode failure here would drop the whole
            // status_update and freeze the menu on stale state.
            let stateRaw = try container.decode(String.self, forKey: .state)
            let state = StatusInfo.State(rawValue: stateRaw) ?? .idle
```

- [ ] **Step 4: Run tests** — `cd ui/macos && swift test --filter WireCodecTests` → PASS; then full `swift test`.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift
git commit -m "feat(ui): Swift wire models for daemon v1.4.0 — selection commands/events, lenient state decode"
```

---

### Task 11: `SelectionDraft` — pure checkbox/tri-state/collapse logic

**Files:**
- Create: `ui/macos/Sources/Classick/Model/SelectionDraft.swift`
- Test: create `ui/macos/Tests/ClassickTests/SelectionDraftTests.swift`

**Interfaces:**
- Produces:
  ```swift
  struct SelectionDraft: Equatable, Sendable {
      var mode: SelectionMode
      var rules: [SelectionRule]
      enum CheckState: Equatable { case off, on, mixed }
      func artistState(_ artist: String, albums: [String]) -> CheckState
      func albumIsChecked(artist: String, album: String) -> Bool
      func genreIsChecked(_ name: String) -> Bool
      mutating func toggleArtist(_ artist: String, albums: [String])
      mutating func toggleAlbum(artist: String, album: String, siblingAlbums: [String])
      mutating func toggleGenre(_ name: String)
  }
  ```
  All name comparisons case-insensitive (`lowercased()`), mirroring the Rust matcher.
- Consumes: `SelectionMode`, `SelectionRule` from Task 10

- [ ] **Step 1: Write the failing tests**

```swift
import XCTest
@testable import Classick

final class SelectionDraftTests: XCTestCase {
    func testToggleArtistAddsAndRemovesArtistRule() {
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleArtist("Aphex Twin", albums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.artist(name: "Aphex Twin")])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .on)
        d.toggleArtist("Aphex Twin", albums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .off)
    }

    func testAlbumSubsetShowsMixedArtistState() {
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleAlbum(artist: "Aphex Twin", album: "Drukqs", siblingAlbums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.album(artist: "Aphex Twin", album: "Drukqs")])
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro"]), .mixed)
        XCTAssertTrue(d.albumIsChecked(artist: "Aphex Twin", album: "Drukqs"))
        XCTAssertFalse(d.albumIsChecked(artist: "Aphex Twin", album: "Syro"))
    }

    func testCheckingLastAlbumCollapsesToArtistRule() {
        // iTunes intuition: hand-checking every album == checking the artist,
        // which auto-includes FUTURE albums too. Deliberate & documented.
        var d = SelectionDraft(mode: .include, rules: [])
        d.toggleAlbum(artist: "Aphex Twin", album: "Drukqs", siblingAlbums: ["Drukqs", "Syro"])
        d.toggleAlbum(artist: "Aphex Twin", album: "Syro", siblingAlbums: ["Drukqs", "Syro"])
        XCTAssertEqual(d.rules, [.artist(name: "Aphex Twin")],
            "all albums checked must collapse to one artist rule")
    }

    func testUncheckingAlbumUnderArtistRuleExpands() {
        var d = SelectionDraft(mode: .include, rules: [.artist(name: "Aphex Twin")])
        d.toggleAlbum(artist: "Aphex Twin", album: "Syro", siblingAlbums: ["Drukqs", "Syro", "SAW II"])
        XCTAssertEqual(Set(d.rules), Set([
            .album(artist: "Aphex Twin", album: "Drukqs"),
            .album(artist: "Aphex Twin", album: "SAW II"),
        ]), "artist rule expands into explicit albums minus the unchecked one")
        XCTAssertEqual(d.artistState("Aphex Twin", albums: ["Drukqs", "Syro", "SAW II"]), .mixed)
    }

    func testGenreToggleRoundTrips() {
        var d = SelectionDraft(mode: .exclude, rules: [])
        d.toggleGenre("Podcast")
        XCTAssertTrue(d.genreIsChecked("Podcast"))
        XCTAssertTrue(d.genreIsChecked("podcast"), "case-insensitive, mirrors the Rust matcher")
        d.toggleGenre("PODCAST")
        XCTAssertFalse(d.genreIsChecked("Podcast"))
    }

    func testModeSwitchKeepsRules() {
        var d = SelectionDraft(mode: .include, rules: [.genre(name: "Ambient")])
        d.mode = .exclude
        XCTAssertEqual(d.rules, [.genre(name: "Ambient")],
            "flipping mode preserves checkbox state; only the meaning flips")
    }
}
```

(`SelectionRule` is declared `Hashable` in `WireModels.swift` (Task 10) so `Set(d.rules)` works here — do NOT add a retroactive conformance in the test target.)

- [ ] **Step 2: Run to verify failure** — `swift test --filter SelectionDraftTests` → COMPILE ERROR.

- [ ] **Step 3: Implement `SelectionDraft.swift`**

```swift
import Foundation

/// The Choose Music window's in-memory draft of {mode, rules}. Pure value
/// logic — no I/O, no daemon — so the tri-state/collapse behavior is fully
/// unit-testable. Name comparisons are case-insensitive to mirror the Rust
/// matcher (crates/classick/src/selection.rs).
struct SelectionDraft: Equatable, Sendable {
    var mode: SelectionMode
    var rules: [SelectionRule]

    enum CheckState: Equatable { case off, on, mixed }

    private func hasArtistRule(_ artist: String) -> Bool {
        rules.contains {
            if case let .artist(name) = $0 { return name.lowercased() == artist.lowercased() }
            return false
        }
    }

    func albumIsChecked(artist: String, album: String) -> Bool {
        if hasArtistRule(artist) { return true }
        return rules.contains {
            if case let .album(a, al) = $0 {
                return a.lowercased() == artist.lowercased() && al.lowercased() == album.lowercased()
            }
            return false
        }
    }

    func artistState(_ artist: String, albums: [String]) -> CheckState {
        if hasArtistRule(artist) { return .on }
        let checked = albums.filter { albumIsChecked(artist: artist, album: $0) }.count
        if checked == 0 { return .off }
        return checked == albums.count ? .on : .mixed
    }

    func genreIsChecked(_ name: String) -> Bool {
        rules.contains {
            if case let .genre(n) = $0 { return n.lowercased() == name.lowercased() }
            return false
        }
    }

    mutating func toggleArtist(_ artist: String, albums: [String]) {
        switch artistState(artist, albums: albums) {
        case .on:
            removeArtistAndAlbumRules(artist: artist)
        case .off, .mixed:
            removeArtistAndAlbumRules(artist: artist)
            rules.append(.artist(name: artist))
        }
    }

    mutating func toggleAlbum(artist: String, album: String, siblingAlbums: [String]) {
        if hasArtistRule(artist) {
            // Unchecking one album under a whole-artist check: expand the
            // artist rule into explicit album rules minus this one.
            removeArtistAndAlbumRules(artist: artist)
            for sibling in siblingAlbums where sibling.lowercased() != album.lowercased() {
                rules.append(.album(artist: artist, album: sibling))
            }
            return
        }
        if albumIsChecked(artist: artist, album: album) {
            rules.removeAll {
                if case let .album(a, al) = $0 {
                    return a.lowercased() == artist.lowercased() && al.lowercased() == album.lowercased()
                }
                return false
            }
        } else {
            rules.append(.album(artist: artist, album: album))
            // Collapse: every album now checked -> one artist rule, which
            // also auto-includes future albums (iTunes intuition).
            let allChecked = siblingAlbums.allSatisfy { albumIsChecked(artist: artist, album: $0) }
            if allChecked {
                removeArtistAndAlbumRules(artist: artist)
                rules.append(.artist(name: artist))
            }
        }
    }

    mutating func toggleGenre(_ name: String) {
        if genreIsChecked(name) {
            rules.removeAll {
                if case let .genre(n) = $0 { return n.lowercased() == name.lowercased() }
                return false
            }
        } else {
            rules.append(.genre(name: name))
        }
    }

    private mutating func removeArtistAndAlbumRules(artist: String) {
        rules.removeAll {
            switch $0 {
            case let .artist(name): return name.lowercased() == artist.lowercased()
            case let .album(a, _): return a.lowercased() == artist.lowercased()
            case .genre: return false
            }
        }
    }
}
```

- [ ] **Step 4: Run tests** — `swift test --filter SelectionDraftTests` → PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/SelectionDraft.swift ui/macos/Tests/ClassickTests/SelectionDraftTests.swift
git commit -m "feat(ui): SelectionDraft — tri-state checkbox logic with artist-rule collapse/expand"
```

---

### Task 12: AppModel — library/selection/preview state + scanning phase

**Files:**
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Test: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift` (append)

**Interfaces:**
- Produces: `AppModel.library: LibraryInfo?`, `AppModel.selection: (mode: SelectionMode, rules: [SelectionRule])?` (store as a small struct `SelectionState { mode, rules }` for Equatable ease), `AppModel.selectionPreview: SelectionPreviewInfo?`, `Phase.scanning(current: Int, total: Int)`, scan-aware `trackStart` routing
- Consumes: Task 10 events

- [ ] **Step 1: Write the failing tests** (append to `AppModelReducerTests.swift`, matching its existing helper style — construct events by decoding JSON lines, as the wire tests do, or directly if the suite constructs enums; follow whichever pattern the file already uses)

```swift
    @MainActor
    func testLibraryAndSelectionEventsPopulateModel() throws {
        let model = AppModel()
        model.apply(try decode(#"{"type":"library_update","source_root":"/m","scanned_at_unix_secs":1,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0}"#))
        XCTAssertEqual(model.library?.sourceRoot, "/m")
        model.apply(try decode(#"{"type":"selection_update","mode":"include","rules":[{"kind":"genre","name":"IDM"}]}"#))
        XCTAssertEqual(model.selection?.mode, .include)
        XCTAssertEqual(model.selection?.rules, [.genre(name: "IDM")])
        model.apply(try decode(#"{"type":"selection_preview","selected_tracks":10,"selected_bytes":100,"adds":2,"removes":3}"#))
        XCTAssertEqual(model.selectionPreview?.removes, 3)
    }

    @MainActor
    func testScanningStatusMakesScanningPhaseAndRoutesTrackStart() throws {
        let model = AppModel()
        // device + config so computePhase doesn't fall into noDevice/notConfigured
        model.apply(try decode(#"{"type":"device_connected","serial":"S","model_label":"iPod","drive":"/Volumes/IPOD"}"#))
        model.apply(try decode(#"{"type":"config_update","source":"/m","ipod":{"serial":"S","model_label":"iPod"}}"#))
        model.apply(try decode(#"{"type":"status_update","state":"scanning","configured":true,"ipod_connected":true,"synced_count":0}"#))
        guard case .scanning = model.phase else { return XCTFail("expected scanning, got \(model.phase)") }

        // Forwarded scan progress must update .scanning, NOT flip to .syncing.
        model.apply(try decode(#"{"type":"sync_event","line":"{\"type\":\"track_start\",\"current\":5,\"total\":100,\"label\":\"x.flac\"}"}"#))
        guard case let .scanning(current, total) = model.phase else {
            return XCTFail("track_start during a scan must stay in scanning; got \(model.phase)")
        }
        XCTAssertEqual(current, 5)
        XCTAssertEqual(total, 100)
    }

    private func decode(_ line: String) throws -> DaemonEvent {
        try JSONDecoder().decode(DaemonEvent.self, from: Data(line.utf8))
    }
```

(Adapt the helper to the file's existing conventions if it already has one.)

- [ ] **Step 2: Run to verify failure** — `swift test --filter AppModelReducerTests` → COMPILE ERROR / FAIL.

- [ ] **Step 3: Implement in `AppModel.swift`**

- `Phase` gains `case scanning(current: Int, total: Int)`.
- New stored state:

```swift
    struct SelectionState: Equatable, Sendable {
        var mode: SelectionMode
        var rules: [SelectionRule]
    }
    private(set) var library: LibraryInfo?
    private(set) var selection: SelectionState?
    private(set) var selectionPreview: SelectionPreviewInfo?
    private var isScanning = false
    /// Raw device capacity for the Choose Music footer's capacity bar
    /// (storageText is display-only). Set beside storageText in the
    /// deviceConnected arm from the same `storageFor(drive:)` call;
    /// cleared on deviceDisconnected.
    private(set) var deviceStorage: (free: Int64, total: Int64)?
```

- `apply` new arms:

```swift
        case let .libraryUpdate(info):
            library = info

        case let .selectionUpdate(mode, rules):
            selection = SelectionState(mode: mode, rules: rules)

        case let .selectionPreview(info):
            selectionPreview = info
```

- `statusUpdate` arm: set `isScanning = (info.state == .scanning)`; treat `.scanning` as a distinct target:

```swift
            switch info.state {
            case .syncing: targetSyncing = true
            case .idle, .scanning: targetSyncing = false
            }
            if info.state == .scanning {
                // Preserve in-flight scan progress across status rebroadcasts.
                if case .scanning = phase {} else { phase = .scanning(current: 0, total: 0) }
            } else {
                phase = computePhase(targetSyncing: targetSyncing)
            }
```

- `applySyncEvent`'s `trackStart` arm routes by scan state:

```swift
        case let .trackStart(current, total, label):
            if isScanning {
                phase = .scanning(current: current, total: total)
            } else {
                phase = .syncing(current: current, total: total, label: label)
            }
```

- `applySyncEvent`'s `.finish` arm: when `isScanning`, return to `computePhase(targetSyncing: false)` (the daemon's post-scan Idle status will confirm; don't leave a stale `.scanning`).
- `ClassickApp.swift`'s `menuBarSystemImage` needs a `.scanning` arm (use `"magnifyingglass"`); `MenuContent.phaseContent` needs a `.scanning` case (Text("Scanning library… \(current) of \(total)")) — compile errors will point at both; fix them in this task so the build stays green.

- [ ] **Step 4: Run tests** — `swift test` (full) → PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift
git commit -m "feat(ui): library/selection/preview state + scanning phase in the reducer"
```

---

### Task 13: Choose Music window + app wiring + manual verification

**Files:**
- Create: `ui/macos/Sources/Classick/Views/ChooseMusicWindow.swift`
- Create: `ui/macos/Sources/Classick/Views/ChooseMusicWindowController.swift`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift` (present + save/scan/preview actions), `ui/macos/Sources/Classick/Views/MenuContent.swift` ("Choose Music…" row + selection line)

**Interfaces:**
- Consumes: `AppModel.{library, selection, selectionPreview, phase}`, `SelectionDraft`, `DaemonClient.send(.getLibrary/.getSelection/.scanLibrary/.saveSelection/.previewSelection/.triggerSync)`
- Produces: user-facing window; no new testable seams beyond what Tasks 10–12 covered (view logic stays thin over `SelectionDraft`)

- [ ] **Step 1: `ChooseMusicWindowController.swift`** — same pattern as `SetupWindowController`:

```swift
import AppKit
import SwiftUI

/// Hosts `ChooseMusicWindow` in an AppKit `NSWindow` owned by the app
/// delegate — same deterministic-presentation rationale as
/// `SetupWindowController`.
@MainActor
final class ChooseMusicWindowController {
    private var window: NSWindow?

    func show(
        model: AppModel,
        onAppear: @escaping () -> Void,
        onScan: @escaping () -> Void,
        onPreview: @escaping (SelectionMode, [SelectionRule]) -> Void,
        onSave: @escaping (SelectionMode, [SelectionRule]) -> Void
    ) {
        NSApp.activate(ignoringOtherApps: true)
        if let window {
            window.makeKeyAndOrderFront(nil)
            return
        }
        let root = ChooseMusicWindow(
            model: model,
            onAppear: onAppear,
            onScan: onScan,
            onPreview: onPreview,
            onSave: onSave,
            onClose: { [weak self] in self?.window?.close() })
        let hosting = NSHostingController(rootView: root)
        let win = NSWindow(contentViewController: hosting)
        win.title = "Choose Music"
        win.styleMask = [.titled, .closable, .resizable]
        win.setContentSize(NSSize(width: 560, height: 620))
        win.isReleasedWhenClosed = false
        win.center()
        window = win
        win.makeKeyAndOrderFront(nil)
    }
}
```

- [ ] **Step 2: `ChooseMusicWindow.swift`** — the approved outline layout. Structure (write it fully; this is the sketch of required parts, all bindings through `SelectionDraft`):

```swift
import SwiftUI

/// The Choose Music browser: mode picker + Artists/Genres tabs + outline
/// checkboxes + live impact footer. Edits a local SelectionDraft; nothing
/// persists until Save. Layout per the approved design (spec §5).
struct ChooseMusicWindow: View {
    var model: AppModel
    var onAppear: () -> Void
    var onScan: () -> Void
    var onPreview: (SelectionMode, [SelectionRule]) -> Void
    var onSave: (SelectionMode, [SelectionRule]) -> Void
    var onClose: () -> Void

    @State private var draft = SelectionDraft(mode: .all, rules: [])
    @State private var seededFromModel = false
    @State private var tab: Tab = .artists
    @State private var search = ""
    @State private var previewTask: Task<Void, Never>?

    enum Tab: String, CaseIterable { case artists = "Artists", genres = "Genres" }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .onAppear {
            onAppear()  // sends get_library + get_selection
        }
        .onChange(of: model.selection) { _, sel in
            // Seed the draft ONCE from the persisted selection; later
            // selection_update echoes (e.g. our own save) must not clobber
            // in-progress edits.
            guard !seededFromModel, let sel else { return }
            draft = SelectionDraft(mode: sel.mode, rules: sel.rules)
            seededFromModel = true
        }
        .onChange(of: draft) { _, d in
            schedulePreview(d)
        }
    }

    private var header: some View {
        VStack(spacing: 8) {
            Picker("Sync", selection: $draft.mode) {
                Text("Entire library").tag(SelectionMode.all)
                Text("Only selected").tag(SelectionMode.include)
                Text("All except selected").tag(SelectionMode.exclude)
            }
            .pickerStyle(.segmented)
            if draft.mode == .exclude {
                Text("Checked items will NOT be synced.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                Picker("", selection: $tab) {
                    ForEach(Tab.allCases, id: \.self) { Text($0.rawValue) }
                }
                .pickerStyle(.segmented)
                .frame(width: 180)
                TextField("Search", text: $search)
                    .textFieldStyle(.roundedBorder)
            }
        }
        .padding(12)
        .disabled(draft.mode == .all)  // grayed out, state kept (spec §5)
    }

    @ViewBuilder
    private var content: some View {
        if let library = model.library, library.scannedAtUnixSecs != nil {
            browser(library)
        } else {
            emptyState
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Spacer()
            Text("Classick needs to read your library's tags once")
                .font(.headline)
            if case let .scanning(current, total) = model.phase {
                ProgressView(value: total > 0 ? Double(current) / Double(total) : 0)
                    .frame(maxWidth: 260)
                Text("Scanning… \(current) of \(total)")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                Button("Scan Library", action: onScan)
                    .keyboardShortcut(.defaultAction)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }

    private func browser(_ library: LibraryInfo) -> some View {
        List {
            switch tab {
            case .artists:
                ForEach(filteredArtists(library), id: \.name) { artist in
                    artistRow(artist)
                }
            case .genres:
                ForEach(filteredGenres(library), id: \.name) { genre in
                    genreRow(genre)
                }
            }
        }
        .listStyle(.inset)
        .disabled(draft.mode == .all)
    }

    private func artistRow(_ artist: LibraryArtist) -> some View {
        let albumNames = artist.albums.map(\.name)
        return DisclosureGroup {
            ForEach(artist.albums, id: \.name) { album in
                Toggle(isOn: Binding(
                    get: { draft.albumIsChecked(artist: artist.name, album: album.name) },
                    set: { _ in draft.toggleAlbum(artist: artist.name, album: album.name, siblingAlbums: albumNames) }
                )) {
                    HStack {
                        Text(album.name.isEmpty ? "Unknown Album" : album.name)
                        Spacer()
                        Text("\(album.tracks) tracks · \(formatBytes(album.bytes))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        } label: {
            Toggle(isOn: Binding(
                get: { draft.artistState(artist.name, albums: albumNames) != .off },
                set: { _ in draft.toggleArtist(artist.name, albums: albumNames) }
            )) {
                HStack {
                    Text(artist.name.isEmpty ? "Unknown Artist" : artist.name)
                        .fontWeight(.medium)
                    if draft.artistState(artist.name, albums: albumNames) == .mixed {
                        Text("–").foregroundStyle(.tint)  // mixed marker
                    }
                    Spacer()
                    Text("\(artist.albums.count) albums")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func genreRow(_ genre: LibraryGenre) -> some View {
        Toggle(isOn: Binding(
            get: { draft.genreIsChecked(genre.name) },
            set: { _ in draft.toggleGenre(genre.name) }
        )) {
            HStack {
                Text(genre.name.isEmpty ? "No Genre" : genre.name)
                Spacer()
                Text("\(genre.tracks) tracks · \(formatBytes(genre.bytes))")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                if let p = model.selectionPreview, draft.mode != .all {
                    Text("\(p.selectedTracks) of \(model.library?.totalTracks ?? 0) tracks · ~\(formatBytes(p.selectedBytes))")
                    Text("next sync: +\(p.adds) / −\(p.removes)")
                        .foregroundStyle(.secondary)
                } else if let lib = model.library {
                    Text("\(lib.totalTracks) tracks · \(formatBytes(lib.totalBytes))")
                }
                Spacer()
                if let scanned = model.library?.scannedAtUnixSecs {
                    Text("Scanned \(relativeDate(scanned))")
                        .foregroundStyle(.secondary)
                    Button("Rescan", action: onScan)
                        .disabled(isBusy)
                }
            }
            // Capacity bar vs the connected iPod (spec §5): warn — don't
            // block Save — when the selection won't fit; sync handles
            // disk-full anyway.
            if let storage = model.deviceStorage {
                let selected = selectedBytesForBar
                let over = selected > UInt64(storage.total)
                ProgressView(value: min(Double(selected), Double(storage.total)),
                             total: Double(storage.total))
                    .tint(over ? .red : .accentColor)
                if over {
                    Text("Selection (~\(formatBytes(selected))) exceeds this iPod's capacity (\(formatBytes(UInt64(storage.total)))).")
                        .font(.caption).foregroundStyle(.red)
                }
            }
            HStack {
                Spacer()
                Button("Cancel", action: onClose)
                Button("Save") {
                    onSave(draft.mode, draft.rules)
                    onClose()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .font(.callout)
        .padding(12)
    }

    /// Bytes driving the capacity bar: preview when a filter is active,
    /// whole library otherwise. Source bytes — an estimate of on-iPod size.
    private var selectedBytesForBar: UInt64 {
        if draft.mode != .all, let p = model.selectionPreview { return p.selectedBytes }
        return model.library?.totalBytes ?? 0
    }

    /// Busy = daemon is scanning or syncing (Rescan would be dropped anyway).
    private var isBusy: Bool {
        switch model.phase {
        case .scanning, .syncing: return true
        default: return false
        }
    }

    private func relativeDate(_ unixSecs: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unixSecs))
        return date.formatted(.relative(presentation: .named))
    }

    private func schedulePreview(_ d: SelectionDraft) {
        previewTask?.cancel()
        guard d.mode != .all else { return }
        previewTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            onPreview(d.mode, d.rules)
        }
    }

    private func filteredArtists(_ library: LibraryInfo) -> [LibraryArtist] {
        guard !search.isEmpty else { return library.artists }
        let q = search.lowercased()
        return library.artists.compactMap { artist in
            if artist.name.lowercased().contains(q) { return artist }
            let albums = artist.albums.filter { $0.name.lowercased().contains(q) }
            return albums.isEmpty ? nil : LibraryArtist(name: artist.name, albums: albums)
        }
    }

    private func filteredGenres(_ library: LibraryInfo) -> [LibraryGenre] {
        guard !search.isEmpty else { return library.genres }
        return library.genres.filter { $0.name.lowercased().contains(search.lowercased()) }
    }
}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}
```

(Implementer notes: if the artist-row Toggle + DisclosureGroup interplay fights on macOS, fall back to a plain checkbox image button — `Image(systemName: state == .mixed ? "minus.square.fill" : checked ? "checkmark.square.fill" : "square")` in a `Button` — visual polish is secondary to correct draft mutation. Also honor the spec's help copy: add a one-line footnote under the header when a whole artist is checked: "Checked artists include their future albums.")

- [ ] **Step 3: Wire in `ClassickApp.swift`**

`AppDelegate` gains:

```swift
    private let chooseMusicController = ChooseMusicWindowController()

    func presentChooseMusic() {
        chooseMusicController.show(
            model: model,
            onAppear: { [weak self] in
                Task {
                    await self?.daemonClient.send(.getLibrary)
                    await self?.daemonClient.send(.getSelection)
                }
            },
            onScan: { [weak self] in
                Task { await self?.daemonClient.send(.scanLibrary) }
            },
            onPreview: { [weak self] mode, rules in
                Task { await self?.daemonClient.send(.previewSelection(mode: mode, rules: rules)) }
            },
            onSave: { [weak self] mode, rules in
                self?.saveSelection(mode: mode, rules: rules)
            })
    }

    private func saveSelection(mode: SelectionMode, rules: [SelectionRule]) {
        let preview = model.selectionPreview
        Task { await daemonClient.send(.saveSelection(mode: mode, rules: rules)) }
        // Offer an immediate sync when the selection changes what's on the
        // iPod and a device is present (spec §5 Save flow).
        if let preview, preview.adds + preview.removes > 0, model.device != nil {
            let alert = NSAlert()
            alert.messageText = "Sync now?"
            alert.informativeText =
                "This selection will add \(preview.adds) and remove \(preview.removes) track(s) at the next sync."
            alert.addButton(withTitle: "Sync Now")
            alert.addButton(withTitle: "Later")
            if alert.runModal() == .alertFirstButtonReturn {
                syncNow()
            }
        }
    }
```

`MenuContent.swift`: add `var onChooseMusic: () -> Void = {}` and render in the `.idle` case (after the storage/last-sync block, before the `Divider`): `Button("Choose Music…", action: onChooseMusic)`. When a filter is active (`model.selection.map { $0.mode != .all } ?? false`), also show `Text("Selection active — \(model.libraryCount ?? 0) tracks")` — `libraryCount` is already the selected Y after Task 9. Pass `onChooseMusic: appDelegate.presentChooseMusic` from `ClassickApp`.

(To keep selection state warm for that menu line, `DaemonClient.handleLine`'s post-hello block sends `getStatus`/`getConfig` — add `await send(.getSelection)` beside them.)

- [ ] **Step 4: Build + test + run**

```bash
cd ui/macos && swift test          # all suites green
cd ../.. && cargo build --release  # core the app embeds
ui/macos/bundle.sh                 # -> ui/macos/Classick.app for manual run
```

- [ ] **Step 5: Manual verification checklist** (on-device where possible; record outcomes in the PR/commit message)

1. Fresh state (no `selection.json`, no `library-index.json`): menu behaves exactly as before; a sync plans identically (mode=all fast path).
2. Open Choose Music → never-scanned empty state → Scan Library → progress counts up → browser populates; menu icon shows the scanning glyph during the scan.
3. Include mode: check one artist → footer preview shows sensible counts; Save → "Sync now?" → sync adds only that artist's tracks; menu shows "X of Y" with Y = selected count.
4. Deselect an on-iPod album → preview shows `−N` → sync removes exactly those tracks (review flow surfaces `remove: N` in Review mode).
5. Add a new album folder for a checked artist → sync picks it up WITHOUT rescanning (inline probe), and it appears in the browser after the next scan.
6. Exclude mode round-trip; mode flip preserves checkboxes; Entire-library mode grays the browser but keeps state.
7. Kill `selection.json` mid-life (corrupt it) → next sync logs a warning and syncs everything.

- [ ] **Step 6: Commit**

```bash
git add ui/macos/Sources/Classick/Views/ChooseMusicWindow.swift ui/macos/Sources/Classick/Views/ChooseMusicWindowController.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/Ipc/DaemonClient.swift
git commit -m "feat(ui): Choose Music window — artist/album/genre selection browser"
```

---

## Post-implementation

- Add a `LEARNINGS.md` bullet for anything non-obvious discovered during implementation (candidates: DisclosureGroup+Toggle interplay, lofty quirks on odd tag encodings, the lenient-state-decode gotcha).
- The Windows UI intentionally ignores all of this (unknown events are dropped per protocol rules) — a `ui/windows` follow-up is out of scope.
- Follow-up candidates (not in this plan): genre delimiter-splitting, per-track checkboxes, remembering per-iPod selections.
