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
    let cfg = crate::config_file::load(config_path).ok().flatten()?;
    let sel_path = selection::effective_selection_path(cfg.ipod_identity.as_ref()).ok()?;
    let sel = selection::load_or_all(&sel_path);
    if sel.mode == SelectionMode::All {
        return None;
    }
    let source = cfg.source?;
    let idx_path = library_index::default_index_path().ok()?;
    let idx = library_index::load_or_empty(&idx_path, &source);
    Some(idx.files.values().filter(|rec| sel.wants(&rec.facts())).count())
}

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
