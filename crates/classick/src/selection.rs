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
