//! Daemon-side library services: aggregate the tag index for the Choose
//! Music browser, evaluate selection previews, derive the selected library
//! count, summarize playlists against the cached index (v1.6.0), and
//! compute the pure device-preview estimate (v1.6.0). All rule evaluation
//! delegates to `crate::selection` / `crate::playlist_rules` — the same
//! evaluators the sync filter and `sync_set::compute` use.

use crate::device_config::Subscriptions;
use crate::ipc_daemon::{
    DaemonEvent, LibraryAlbum, LibraryArtist, LibraryGenre, PlaylistKind, PlaylistSummary,
};
use crate::library_index::{self, LibraryIndex};
use crate::manifest::Manifest;
use crate::playlist::{Playlist, PlaylistStore};
use crate::selection::{self, Selection, SelectionMode, SelectionRule};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

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

        let g = genres
            .entry(rec.genre.to_lowercase())
            .or_insert_with(|| (rec.genre.clone(), 0, 0));
        g.1 += 1;
        g.2 += rec.size;
    }

    let mut by_artist: BTreeMap<String, LibraryArtist> = BTreeMap::new();
    for ((artist_key, _), agg) in albums {
        let display_genre = majority_genre(&agg.genre_counts);
        let entry = by_artist
            .entry(artist_key)
            .or_insert_with(|| LibraryArtist {
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
    let genres: Vec<LibraryGenre> = genres
        .into_values()
        .map(|(name, tracks, bytes)| LibraryGenre {
            name,
            tracks,
            bytes,
        })
        .collect();
    (artists, genres, total_tracks, total_bytes)
}

/// Most common genre in the album, None on tie or when all are empty-tag.
fn majority_genre(counts: &BTreeMap<String, usize>) -> Option<String> {
    let mut best: Option<(&String, usize)> = None;
    let mut tied = false;
    for (g, &n) in counts {
        match best {
            Some((_, bn)) if n > bn => {
                best = Some((g, n));
                tied = false;
            }
            Some((_, bn)) if n == bn => {
                tied = true;
            }
            None => {
                best = Some((g, n));
            }
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
    let source = crate::config_file::load(config_path)
        .ok()
        .flatten()
        .and_then(|c| c.source);
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
                artists,
                genres,
                total_tracks,
                total_bytes,
                acknowledged_request_id: None,
            }
        }
        None => DaemonEvent::LibraryUpdate {
            source_root: source.map(|p| p.display().to_string()),
            scanned_at_unix_secs: None,
            artists: Vec::new(),
            genres: Vec::new(),
            total_tracks: 0,
            total_bytes: 0,
            acknowledged_request_id: None,
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
    let sel = Selection {
        version: selection::SELECTION_VERSION,
        mode,
        rules: rules.to_vec(),
    };
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
    let manifest_paths: std::collections::HashSet<_> = manifest
        .tracks
        .iter()
        .filter(|e| e.source_known)
        .map(|e| e.source_path.clone())
        .collect();
    let adds = selected_paths
        .iter()
        .filter(|p| !manifest_paths.contains(*p))
        .count();
    let removes = manifest_paths
        .iter()
        .filter(|p| !selected_paths.contains(*p))
        .count();
    (selected_tracks, selected_bytes, adds, removes)
}

/// The selected library count (Y in "X of Y synced"). None when the mode is
/// All (caller keeps using its walk-based cache) or when no index/source is
/// available.
///
/// Reads the explicitly targeted device's per-device selection via
/// `selection::effective_device_selection_path` (v1.6.0) rather than the
/// deprecated `custom_selection`-gated `effective_selection_path`, so this
/// stays consistent with what `get_device_config`/`save_device_config` now
/// read and write.
pub fn selected_library_count(config_path: &Path, serial: &str) -> Option<usize> {
    let cfg = crate::config_file::load(config_path).ok().flatten()?;
    let config_root = config_path.parent().unwrap_or_else(|| Path::new("."));
    let sel_path = selection::effective_device_selection_path_in(config_root, serial).ok()?;
    let sel = selection::load_or_all(&sel_path);
    if sel.mode == SelectionMode::All {
        return None;
    }
    let source = cfg.source?;
    let idx_path = library_index::default_index_path().ok()?;
    let idx = library_index::load_or_empty(&idx_path, &source);
    Some(
        idx.files
            .values()
            .filter(|rec| sel.wants(&rec.facts()))
            .count(),
    )
}

/// Resolve one playlist's member tracks against the cached library index —
/// no filesystem walk. Manual playlists join their source-relative tracks
/// onto `index.source_root` and keep only entries the index already knows
/// about, using `playlist::resolve_manual`'s injectable existence check
/// with the cached index as the oracle instead of a live walk (mirrors
/// `sync_set::compute`'s "the walk is the existence oracle" contract, one
/// level more conservative since even the index may be stale). Smart
/// playlists evaluate their rules directly against the index.
pub(crate) fn resolve_playlist_against_index(
    playlist: &Playlist,
    index: &LibraryIndex,
) -> Vec<PathBuf> {
    match playlist {
        Playlist::Manual(m) => {
            let (found, _missing) = crate::playlist::resolve_manual(m, &index.source_root, &|p| {
                index.files.contains_key(p)
            });
            found
        }
        Playlist::Smart(s) => crate::playlist_rules::evaluate(&s.rules, index),
    }
}

/// Track count + summed byte size of a playlist's resolved members, per
/// `resolve_playlist_against_index`.
pub(crate) fn playlist_tracks_and_bytes(playlist: &Playlist, index: &LibraryIndex) -> (usize, u64) {
    let paths = resolve_playlist_against_index(playlist, index);
    let bytes = paths
        .iter()
        .filter_map(|p| index.files.get(p))
        .map(|t| t.size)
        .sum();
    (paths.len(), bytes)
}

fn summarize_playlist(playlist: &Playlist, index: &LibraryIndex) -> PlaylistSummary {
    let (tracks, bytes) = playlist_tracks_and_bytes(playlist, index);
    let kind = match playlist {
        Playlist::Manual(_) => PlaylistKind::Manual,
        Playlist::Smart(_) => PlaylistKind::Smart,
    };
    PlaylistSummary {
        slug: playlist.slug().to_string(),
        name: playlist.name().to_string(),
        kind,
        tracks,
        bytes,
        error: None,
    }
}

/// `(slug, kind)` inferred from a store file's name, for the
/// `store.last_errors()` stub-summary path below. `None` for anything that
/// isn't a recognized playlist file extension (e.g. the store root itself,
/// on a `read_dir` failure).
fn kind_from_error_path(path: &Path) -> Option<(String, PlaylistKind)> {
    let name = path.file_name()?.to_str()?;
    if let Some(slug) = name.strip_suffix(".m3u8") {
        Some((slug.to_string(), PlaylistKind::Manual))
    } else if let Some(slug) = name.strip_suffix(".rules.json") {
        Some((slug.to_string(), PlaylistKind::Smart))
    } else {
        None
    }
}

/// Every playlist in `store`, summarized against `index`, sorted by `slug`
/// for deterministic wire ordering. A file the store failed to parse still
/// surfaces as a stub summary (`tracks`/`bytes` zero, `error` set to the
/// failure) instead of silently vanishing from the list — see
/// `PlaylistStore::last_errors`.
pub(crate) fn build_playlist_summaries(
    store: &PlaylistStore,
    index: &LibraryIndex,
) -> Vec<PlaylistSummary> {
    let mut out: Vec<PlaylistSummary> = match store.list() {
        Ok(playlists) => playlists
            .iter()
            .map(|p| summarize_playlist(p, index))
            .collect(),
        Err(e) => {
            tracing::warn!("playlists: failed to list store ({e:#})");
            Vec::new()
        }
    };
    for (path, message) in store.last_errors() {
        if let Some((slug, kind)) = kind_from_error_path(&path) {
            out.push(PlaylistSummary {
                slug: slug.clone(),
                name: slug,
                kind,
                tracks: 0,
                bytes: 0,
                error: Some(message),
            });
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// Pure "what would this device sync" estimate for `preview_device`: no
/// filesystem walk, only the cached library index plus this device's
/// selection/subscriptions/playlist-store state.
///
/// `selected_*` mirrors `preview()`'s selection-scope math above.
/// `playlist_extra_*` is subscribed-playlist members NOT already in that
/// scope — the same union-delta idea as `sync_set::compute`, sized from the
/// index rather than a live walk; an unresolvable subscription (unknown
/// slug, store load error, or no store at all) is skipped from the
/// `playlist_extra_*` totals — matching `sync_set::compute`'s "never fatal
/// to the caller" contract — but its slug is collected into
/// `unresolved_subscriptions` (sorted) so the caller can still surface the
/// dangling subscription instead of it disappearing silently.
///
/// `current_free_bytes` is `Some` only when the target device is the one
/// currently connected (its live `StorageInfo::free_bytes`); `store` is
/// `Option` so a playlist-store-open failure degrades to "no playlist
/// extras" rather than failing the whole preview. `projected_free_bytes`
/// subtracts a "net-new" estimate from `current_free_bytes`: the source
/// bytes of selected + playlist-extra tracks NOT already on the device per
/// `already_synced` (the device manifest's source paths). A fully-synced
/// selection therefore projects no change — without this, the capacity bar
/// showed a phantom "pending" band for bytes that were already on disk.
/// Still an estimate (source FLAC size stands in for on-device ALAC size,
/// and removes aren't credited back); `None` in, `None` out.
pub(crate) fn compute_device_preview(
    index: &LibraryIndex,
    selection: &Selection,
    subs: &Subscriptions,
    store: Option<&PlaylistStore>,
    current_free_bytes: Option<u64>,
    already_synced: &HashSet<PathBuf>,
    serial: &str,
    acknowledged_request_id: String,
) -> DaemonEvent {
    let mut selected_tracks = 0usize;
    let mut selected_bytes = 0u64;
    let mut selected_paths: HashSet<&Path> = HashSet::new();
    for (path, rec) in &index.files {
        if selection.wants(&rec.facts()) {
            selected_tracks += 1;
            selected_bytes += rec.size;
            selected_paths.insert(path.as_path());
        }
    }

    let mut extra_paths: HashSet<PathBuf> = HashSet::new();
    let mut unresolved_subscriptions: Vec<String> = Vec::new();
    for slug in &subs.playlists {
        let resolved = match store {
            Some(store) => match store.load(slug) {
                Ok(Some(playlist)) => {
                    for path in resolve_playlist_against_index(&playlist, index) {
                        if !selected_paths.contains(path.as_path()) {
                            extra_paths.insert(path);
                        }
                    }
                    true
                }
                Ok(None) | Err(_) => false,
            },
            None => false,
        };
        if !resolved {
            unresolved_subscriptions.push(slug.clone());
        }
    }
    unresolved_subscriptions.sort();
    let playlist_extra_tracks = extra_paths.len();
    let playlist_extra_bytes: u64 = extra_paths
        .iter()
        .filter_map(|p| index.files.get(p))
        .map(|t| t.size)
        .sum();

    let net_new: u64 = selected_paths
        .iter()
        .copied()
        .filter(|p| !already_synced.contains(*p))
        .filter_map(|p| index.files.get(p))
        .map(|t| t.size)
        .chain(
            extra_paths
                .iter()
                .filter(|p| !already_synced.contains(p.as_path()))
                .filter_map(|p| index.files.get(p))
                .map(|t| t.size),
        )
        .sum();
    let projected_free_bytes = current_free_bytes.map(|free| free.saturating_sub(net_new));

    DaemonEvent::DevicePreview {
        serial: serial.to_string(),
        selected_tracks,
        selected_bytes,
        playlist_extra_tracks,
        playlist_extra_bytes,
        projected_free_bytes,
        unresolved_subscriptions,
        acknowledged_request_id,
    }
}

/// Expand `rules` into concrete source-relative track paths against the
/// cached library index — what `resolve_tracks` (v1.7.0) replies with so a
/// client that only has aggregate library data (artist/album/genre counts)
/// can turn a rule-based picker selection into concrete playlist entries.
///
/// Builds a throwaway `Selection { mode: Include, rules }` and reuses
/// `Selection::wants` — the SAME case-insensitive matcher the sync filter,
/// `preview`, and `compute_device_preview` all use — so this expands
/// exactly the tracks a saved selection with the same rules would keep. A
/// rule matching nothing contributes nothing (never an error); because the
/// match is a single OR-across-all-rules pass over the index (not a
/// per-rule expand-then-union), a track matched by more than one rule
/// appears exactly once. Paths are `index.source_root`-relative with
/// forward slashes (mirrors `playlist::render_m3u8`'s wire convention);
/// a path that somehow isn't under `source_root` is dropped rather than
/// panicking. Sorted lexicographically for deterministic wire ordering.
/// An empty/never-scanned `index` (no source configured yet) yields an
/// empty result, not an error.
pub fn resolve_tracks(index: &LibraryIndex, rules: &[SelectionRule]) -> Vec<String> {
    let sel = Selection {
        version: selection::SELECTION_VERSION,
        mode: SelectionMode::Include,
        rules: rules.to_vec(),
    };
    let mut out: Vec<String> = index
        .files
        .iter()
        .filter(|(_, rec)| sel.wants(&rec.facts()))
        .filter_map(|(path, _)| path.strip_prefix(&index.source_root).ok())
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_index::{IndexedTrack, LibraryIndex};
    use crate::selection::{SelectionMode, SelectionRule};
    use std::path::PathBuf;

    fn track(
        artist: &str,
        album_artist: &str,
        album: &str,
        genre: &str,
        size: u64,
    ) -> IndexedTrack {
        IndexedTrack {
            mtime: 1,
            size,
            artist: artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: genre.into(),
            title: String::new(),
            duration_ms: 0,
            year: None,
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
            (
                "/m/c1.flac",
                track("Track Artist", "Various Artists", "Comp", "Pop", 5),
            ),
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
        assert_eq!(
            artists[1].name, "Various Artists",
            "album_artist wins grouping"
        );
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
        assert_eq!(
            artists[0].name, "",
            "wire carries raw empty; UI renders 'Unknown Artist'"
        );
        assert_eq!(genres[0].name, "");
    }

    #[test]
    fn preview_counts_adds_and_removes_against_manifest() {
        use crate::manifest::{Manifest, ManifestEntry};
        let idx = index_with(vec![
            ("/m/on_ipod_kept.flac", track("Keep", "", "K", "IDM", 10)),
            (
                "/m/on_ipod_dropped.flac",
                track("Drop", "", "D", "Rock", 20),
            ),
            ("/m/new_selected.flac", track("Keep", "", "K2", "IDM", 30)),
        ]);
        let entry = |p: &str| ManifestEntry {
            source_path: PathBuf::from(p),
            source_mtime: 1,
            source_size: 1,
            source_fingerprint: String::new(),
            ipod_dbid: 1,
            ipod_relpath: String::new(),
            source_known: true,
            audio_fingerprint: String::new(),
            encoder: "unknown".into(),
            encoder_version: String::new(),
            source_format: "flac".into(),
        };
        let manifest = Manifest {
            version: 1,
            ipod_serial: None,
            last_source_root: None,
            tracks: vec![
                entry("/m/on_ipod_kept.flac"),
                entry("/m/on_ipod_dropped.flac"),
            ],
        };
        let rules = vec![SelectionRule::Artist {
            name: "Keep".into(),
        }];
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
        let manifest = Manifest {
            version: 1,
            ipod_serial: None,
            last_source_root: None,
            tracks: vec![],
        };
        let (selected, _, adds, removes) = preview(&idx, &manifest, SelectionMode::All, &[]);
        assert_eq!(selected, 1);
        assert_eq!(adds, 1);
        assert_eq!(removes, 0);
    }

    // --- v1.6.0: playlist summaries + device preview --------------------

    use crate::device_config::Subscriptions;
    use crate::playlist::{ManualPlaylist, Playlist, PlaylistStore, SmartPlaylist};
    use crate::playlist_rules::{Match, Order, SmartRules, RULES_VERSION};

    fn tempdir_under_target(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!(
                "daemon-library-{label}-{}-{}",
                std::process::id(),
                n
            ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn empty_rules() -> SmartRules {
        SmartRules {
            version: RULES_VERSION,
            matching: Match::All,
            rules: vec![],
            limit: None,
            order: Order::default(),
            seed: 0,
        }
    }

    #[test]
    fn build_playlist_summaries_sorted_and_sizes_against_index() {
        let root = tempdir_under_target("summaries");
        let store = PlaylistStore::open(root.clone()).unwrap();
        store
            .save(&Playlist::Manual(ManualPlaylist {
                slug: "gym".into(),
                name: "Gym".into(),
                tracks: vec![PathBuf::from("a1.flac"), PathBuf::from("missing.flac")],
                skipped_unsafe: 0,
            }))
            .unwrap();
        let mut smart_rules = empty_rules();
        smart_rules.rules = vec![crate::playlist_rules::Rule {
            field: crate::playlist_rules::Field::Genre,
            op: crate::playlist_rules::Op::Is,
            value: "IDM".into(),
        }];
        store
            .save(&Playlist::Smart(SmartPlaylist {
                slug: "chill".into(),
                name: "Chill".into(),
                rules: smart_rules,
            }))
            .unwrap();

        let idx = index_with(vec![
            ("/m/a1.flac", track("Aphex Twin", "", "Drukqs", "IDM", 100)),
            ("/m/a2.flac", track("Aphex Twin", "", "Drukqs", "IDM", 50)),
        ]);

        let summaries = build_playlist_summaries(&store, &idx);
        assert_eq!(summaries.len(), 2);
        // sorted by slug: "chill" < "gym"
        assert_eq!(summaries[0].slug, "chill");
        assert_eq!(summaries[0].kind, PlaylistKind::Smart);
        assert_eq!(
            summaries[0].tracks, 2,
            "both IDM tracks match the smart rule"
        );
        assert_eq!(summaries[0].bytes, 150);
        assert!(summaries[0].error.is_none());

        assert_eq!(summaries[1].slug, "gym");
        assert_eq!(summaries[1].kind, PlaylistKind::Manual);
        assert_eq!(
            summaries[1].tracks, 1,
            "missing.flac isn't in the index, so it's dropped"
        );
        assert_eq!(summaries[1].bytes, 100);
    }

    #[test]
    fn build_playlist_summaries_surfaces_corrupt_file_as_error_stub() {
        let root = tempdir_under_target("corrupt");
        let store = PlaylistStore::open(root.clone()).unwrap();
        std::fs::write(root.join("broken.rules.json"), b"{ not json").unwrap();

        let idx = LibraryIndex::empty(PathBuf::from("/m"));
        let summaries = build_playlist_summaries(&store, &idx);

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].slug, "broken");
        assert_eq!(summaries[0].kind, PlaylistKind::Smart);
        assert_eq!(summaries[0].tracks, 0);
        assert_eq!(summaries[0].bytes, 0);
        assert!(
            summaries[0].error.is_some(),
            "corrupt file surfaces with its parse error, not silently dropped"
        );
    }

    #[test]
    fn compute_device_preview_math_with_playlist_extra_and_projected_free() {
        let root = tempdir_under_target("preview");
        let store = PlaylistStore::open(root.clone()).unwrap();
        // "extra" is entirely outside the selection scope below.
        store
            .save(&Playlist::Manual(ManualPlaylist {
                slug: "extra".into(),
                name: "Extra".into(),
                tracks: vec![PathBuf::from("out_of_scope.flac")],
                skipped_unsafe: 0,
            }))
            .unwrap();

        let idx = index_with(vec![
            ("/m/in_scope.flac", track("Keep", "", "Album", "G", 1_000)),
            (
                "/m/out_of_scope.flac",
                track("Drop", "", "Album2", "G", 500),
            ),
        ]);
        let selection = Selection {
            version: crate::selection::SELECTION_VERSION,
            mode: SelectionMode::Include,
            rules: vec![SelectionRule::Artist {
                name: "Keep".into(),
            }],
        };
        let subs = Subscriptions {
            version: 1,
            playlists: vec!["extra".into()],
        };

        let event = compute_device_preview(
            &idx,
            &selection,
            &subs,
            Some(&store),
            Some(10_000),
            &HashSet::new(),
            "serial-a",
            "request-a".into(),
        );
        match event {
            DaemonEvent::DevicePreview {
                selected_tracks,
                selected_bytes,
                playlist_extra_tracks,
                playlist_extra_bytes,
                projected_free_bytes,
                unresolved_subscriptions,
                ..
            } => {
                assert_eq!(selected_tracks, 1);
                assert_eq!(selected_bytes, 1_000);
                assert_eq!(
                    playlist_extra_tracks, 1,
                    "out_of_scope.flac is pulled in only by the subscription"
                );
                assert_eq!(playlist_extra_bytes, 500);
                assert_eq!(projected_free_bytes, Some(10_000 - 1_000 - 500));
                assert!(
                    unresolved_subscriptions.is_empty(),
                    "the one subscription resolved cleanly"
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    /// Regression (2026-07-18): the projection used to assume "syncing from
    /// empty", so a fully-synced selection still showed a phantom pending
    /// band on the capacity bar. Tracks already in the device manifest must
    /// not count toward net-new; a partially-synced selection counts only
    /// the missing tracks.
    #[test]
    fn compute_device_preview_already_synced_tracks_project_no_change() {
        let idx = index_with(vec![
            ("/m/a.flac", track("X", "", "A", "G", 1_000)),
            ("/m/b.flac", track("X", "", "B", "G", 300)),
        ]);
        let subs = Subscriptions::default();

        let all_synced: HashSet<PathBuf> = [PathBuf::from("/m/a.flac"), PathBuf::from("/m/b.flac")]
            .into_iter()
            .collect();
        match compute_device_preview(
            &idx,
            &Selection::all(),
            &subs,
            None,
            Some(10_000),
            &all_synced,
            "serial-a",
            "request-a".into(),
        ) {
            DaemonEvent::DevicePreview {
                projected_free_bytes,
                selected_bytes,
                ..
            } => {
                assert_eq!(
                    selected_bytes, 1_300,
                    "scope totals still report the full selection"
                );
                assert_eq!(
                    projected_free_bytes,
                    Some(10_000),
                    "nothing new to sync — free space unchanged"
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }

        let partial: HashSet<PathBuf> = [PathBuf::from("/m/a.flac")].into_iter().collect();
        match compute_device_preview(
            &idx,
            &Selection::all(),
            &subs,
            None,
            Some(10_000),
            &partial,
            "serial-a",
            "request-a".into(),
        ) {
            DaemonEvent::DevicePreview {
                projected_free_bytes,
                ..
            } => {
                assert_eq!(
                    projected_free_bytes,
                    Some(10_000 - 300),
                    "only the missing track counts"
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    #[test]
    fn compute_device_preview_no_current_free_projects_none() {
        let idx = index_with(vec![("/m/a.flac", track("X", "", "A", "G", 10))]);
        let selection = Selection::all();
        let subs = Subscriptions::default();

        let event = compute_device_preview(
            &idx,
            &selection,
            &subs,
            None,
            None,
            &HashSet::new(),
            "serial-a",
            "request-a".into(),
        );
        match event {
            DaemonEvent::DevicePreview {
                selected_tracks,
                projected_free_bytes,
                ..
            } => {
                assert_eq!(selected_tracks, 1);
                assert_eq!(
                    projected_free_bytes, None,
                    "no live StorageInfo baseline to project from"
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    #[test]
    fn compute_device_preview_unresolvable_subscription_is_skipped_not_fatal() {
        let root = tempdir_under_target("preview-unresolvable");
        let store = PlaylistStore::open(root.clone()).unwrap(); // no playlists saved
        let idx = index_with(vec![("/m/a.flac", track("X", "", "A", "G", 10))]);
        let subs = Subscriptions {
            version: 1,
            playlists: vec!["does-not-exist".into()],
        };

        let event = compute_device_preview(
            &idx,
            &Selection::all(),
            &subs,
            Some(&store),
            None,
            &HashSet::new(),
            "serial-a",
            "request-a".into(),
        );
        match event {
            DaemonEvent::DevicePreview {
                playlist_extra_tracks,
                playlist_extra_bytes,
                unresolved_subscriptions,
                ..
            } => {
                assert_eq!(playlist_extra_tracks, 0);
                assert_eq!(playlist_extra_bytes, 0);
                assert_eq!(
                    unresolved_subscriptions,
                    vec!["does-not-exist".to_string()],
                    "the unknown slug surfaces instead of vanishing silently"
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    #[test]
    fn compute_device_preview_unresolved_subscriptions_sorted_and_skips_resolved() {
        let root = tempdir_under_target("preview-unresolved-mixed");
        let store = PlaylistStore::open(root.clone()).unwrap();
        store
            .save(&Playlist::Manual(ManualPlaylist {
                slug: "gym".into(),
                name: "Gym".into(),
                tracks: vec![PathBuf::from("a.flac")],
                skipped_unsafe: 0,
            }))
            .unwrap();
        let idx = index_with(vec![("/m/a.flac", track("X", "", "A", "G", 10))]);
        // "zzz" and "aaa" are both unresolvable; "gym" resolves. Insertion
        // order here is deliberately not alphabetical to prove the output
        // is sorted, not just insertion-order.
        let subs = Subscriptions {
            version: 1,
            playlists: vec!["zzz".into(), "gym".into(), "aaa".into()],
        };

        let event = compute_device_preview(
            &idx,
            &Selection::all(),
            &subs,
            Some(&store),
            None,
            &HashSet::new(),
            "serial-a",
            "request-a".into(),
        );
        match event {
            DaemonEvent::DevicePreview {
                unresolved_subscriptions,
                ..
            } => {
                assert_eq!(
                    unresolved_subscriptions,
                    vec!["aaa".to_string(), "zzz".to_string()]
                );
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    #[test]
    fn compute_device_preview_no_store_marks_all_subscriptions_unresolved() {
        let idx = index_with(vec![("/m/a.flac", track("X", "", "A", "G", 10))]);
        let subs = Subscriptions {
            version: 1,
            playlists: vec!["gym".into()],
        };

        let event = compute_device_preview(
            &idx,
            &Selection::all(),
            &subs,
            None,
            None,
            &HashSet::new(),
            "serial-a",
            "request-a".into(),
        );
        match event {
            DaemonEvent::DevicePreview {
                unresolved_subscriptions,
                ..
            } => {
                assert_eq!(unresolved_subscriptions, vec!["gym".to_string()]);
            }
            other => panic!("expected DevicePreview, got {other:?}"),
        }
    }

    // --- v1.7.0: resolve_tracks ------------------------------------------

    #[test]
    fn resolve_tracks_artist_rule_expands_to_all_its_files() {
        let idx = index_with(vec![
            (
                "/m/Radiohead/OK Computer/01.flac",
                track("Radiohead", "", "OK Computer", "Rock", 1),
            ),
            (
                "/m/Radiohead/OK Computer/02.flac",
                track("Radiohead", "", "OK Computer", "Rock", 1),
            ),
            (
                "/m/Burial/Untrue/01.flac",
                track("Burial", "", "Untrue", "Dubstep", 1),
            ),
        ]);
        let rules = vec![SelectionRule::Artist {
            name: "Radiohead".into(),
        }];
        let tracks = resolve_tracks(&idx, &rules);
        assert_eq!(
            tracks,
            vec![
                "Radiohead/OK Computer/01.flac".to_string(),
                "Radiohead/OK Computer/02.flac".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_tracks_album_rule_expands_to_that_album_only() {
        let idx = index_with(vec![
            (
                "/m/Aphex Twin/Drukqs/01.flac",
                track("Aphex Twin", "", "Drukqs", "IDM", 1),
            ),
            (
                "/m/Aphex Twin/Syro/01.flac",
                track("Aphex Twin", "", "Syro", "IDM", 1),
            ),
        ]);
        let rules = vec![SelectionRule::Album {
            artist: "Aphex Twin".into(),
            album: "Drukqs".into(),
        }];
        let tracks = resolve_tracks(&idx, &rules);
        assert_eq!(tracks, vec!["Aphex Twin/Drukqs/01.flac".to_string()]);
    }

    #[test]
    fn resolve_tracks_genre_rule_expands_across_artists() {
        let idx = index_with(vec![
            ("/m/A/X/01.flac", track("A", "", "X", "IDM", 1)),
            ("/m/B/Y/01.flac", track("B", "", "Y", "IDM", 1)),
            ("/m/C/Z/01.flac", track("C", "", "Z", "Rock", 1)),
        ]);
        let rules = vec![SelectionRule::Genre { name: "IDM".into() }];
        let tracks = resolve_tracks(&idx, &rules);
        assert_eq!(
            tracks,
            vec!["A/X/01.flac".to_string(), "B/Y/01.flac".to_string()]
        );
    }

    #[test]
    fn resolve_tracks_matches_case_insensitively() {
        let idx = index_with(vec![(
            "/m/Radiohead/OK Computer/01.flac",
            track("Radiohead", "", "OK Computer", "Rock", 1),
        )]);
        let rules = vec![SelectionRule::Artist {
            name: "radiohead".into(),
        }];
        let tracks = resolve_tracks(&idx, &rules);
        assert_eq!(tracks, vec!["Radiohead/OK Computer/01.flac".to_string()]);
    }

    #[test]
    fn resolve_tracks_unmatched_rule_contributes_nothing() {
        let idx = index_with(vec![(
            "/m/Radiohead/OK Computer/01.flac",
            track("Radiohead", "", "OK Computer", "Rock", 1),
        )]);
        let rules = vec![SelectionRule::Artist {
            name: "Nobody".into(),
        }];
        assert!(resolve_tracks(&idx, &rules).is_empty());
    }

    #[test]
    fn resolve_tracks_mixed_rules_dedup_a_track_matched_by_two_rules() {
        let idx = index_with(vec![
            (
                "/m/Radiohead/OK Computer/01.flac",
                track("Radiohead", "", "OK Computer", "Rock", 1),
            ),
            (
                "/m/Burial/Untrue/01.flac",
                track("Burial", "", "Untrue", "Dubstep", 1),
            ),
        ]);
        // Both the artist rule AND the genre rule match the Radiohead track;
        // it must appear exactly once in the result.
        let rules = vec![
            SelectionRule::Artist {
                name: "Radiohead".into(),
            },
            SelectionRule::Genre {
                name: "Rock".into(),
            },
        ];
        let tracks = resolve_tracks(&idx, &rules);
        assert_eq!(tracks, vec!["Radiohead/OK Computer/01.flac".to_string()]);
    }

    #[test]
    fn resolve_tracks_is_deterministically_sorted() {
        let idx = index_with(vec![
            (
                "/m/Z Artist/Album/01.flac",
                track("Z Artist", "", "Album", "Rock", 1),
            ),
            (
                "/m/A Artist/Album/01.flac",
                track("A Artist", "", "Album", "Rock", 1),
            ),
            (
                "/m/M Artist/Album/01.flac",
                track("M Artist", "", "Album", "Rock", 1),
            ),
        ]);
        let rules = vec![SelectionRule::Genre {
            name: "Rock".into(),
        }];
        let tracks = resolve_tracks(&idx, &rules);
        let mut sorted = tracks.clone();
        sorted.sort();
        assert_eq!(tracks, sorted, "must already be sorted");
        assert_eq!(
            tracks,
            vec![
                "A Artist/Album/01.flac".to_string(),
                "M Artist/Album/01.flac".to_string(),
                "Z Artist/Album/01.flac".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_tracks_empty_index_yields_empty_tracks() {
        let idx = LibraryIndex::empty(PathBuf::from("/m"));
        let rules = vec![SelectionRule::Artist {
            name: "Anyone".into(),
        }];
        assert!(resolve_tracks(&idx, &rules).is_empty());
    }

    #[test]
    fn resolve_tracks_no_rules_yields_empty_tracks() {
        let idx = index_with(vec![("/m/A/X/01.flac", track("A", "", "X", "G", 1))]);
        assert!(resolve_tracks(&idx, &[]).is_empty());
    }
}
