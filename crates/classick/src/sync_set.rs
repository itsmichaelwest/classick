//! Union planner: device content = scope selection ∪ subscribed playlists
//! (spec §3). `compute` scope-filters the walk with the existing selection
//! semantics, resolves every subscribed playlist against the SAME walk, and
//! unions the two by absolute path.
//!
//! The walk is the existence oracle: a playlist can only contribute tracks
//! the walk actually saw. This is what keeps the source directory read-only
//! from the playlist's point of view — a manual playlist that lists a path
//! outside (or no longer in) the walked library can never fabricate a
//! `SourceEntry` for it, it just counts toward `missing_playlist_tracks`.

use crate::device_config::Subscriptions;
use crate::library_index::LibraryIndex;
use crate::playlist::{Playlist, PlaylistStore};
use crate::selection::Selection;
use crate::source::SourceEntry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The resolved sync set for one run: the union of the scope selection and
/// every subscribed playlist, plus enough per-playlist detail for the
/// device-playlist reconcile (Task 6) and for progress logging.
#[derive(Debug)]
pub struct EffectiveSet {
    /// Scope-filtered walk, in walk order, with playlist-only additions
    /// appended (subscription order, then playlist-internal order),
    /// deduped by absolute path.
    pub sources: Vec<SourceEntry>,
    /// Per-playlist RESOLVED absolute paths, in subscription order. Each
    /// playlist lists ALL of its resolved members, including ones already in
    /// scope — Task 6's device-playlist reconcile needs the full membership,
    /// not just the out-of-scope delta.
    pub playlist_tracks: Vec<(String, Vec<PathBuf>)>,
    /// Count of playlist track references that resolved (manual: existed +
    /// safe; smart: matched a rule) but aren't in the walk, so were dropped
    /// rather than fabricated.
    pub missing_playlist_tracks: usize,
    /// `(slug, message)` for every subscribed slug that couldn't be
    /// resolved at all (unknown slug, or a store load error) — logged, but
    /// never fatal to the sync.
    pub playlist_errors: Vec<(String, String)>,
}

/// Compute this run's effective sync set. `walk` is the freshly-walked
/// source tree (the existence oracle); `index` is the cached tag index used
/// both for scope matching and for smart-playlist rule evaluation.
///
/// Scope matching reuses `selection::filter`'s exact semantics (freshness
/// check + `Selection::wants`) against a local clone of `index` — `compute`
/// never re-probes the filesystem itself (that's the library scan's job);
/// an entry that isn't already indexed with a matching (mtime, size) fails
/// open (kept), same as a live probe failure would.
pub fn compute(
    walk: Vec<SourceEntry>,
    selection: &Selection,
    subs: &Subscriptions,
    store: &PlaylistStore,
    index: &LibraryIndex,
    source_root: &Path,
) -> EffectiveSet {
    let walk_map: HashMap<PathBuf, SourceEntry> =
        walk.iter().map(|e| (e.path.clone(), e.clone())).collect();

    let mut index_local = index.clone();
    let (mut sources, _dirty) = crate::selection::filter(walk, selection, &mut index_local, |_p| {
        Err(anyhow::anyhow!("sync_set: no live filesystem probe available"))
    });
    let mut present: HashSet<PathBuf> = sources.iter().map(|e| e.path.clone()).collect();

    let mut playlist_tracks = Vec::with_capacity(subs.playlists.len());
    let mut missing = 0usize;
    let mut errors = Vec::new();

    for slug in &subs.playlists {
        let resolved = match store.load(slug) {
            Ok(Some(Playlist::Manual(manual))) => {
                let (found, miss) =
                    crate::playlist::resolve_manual(&manual, source_root, &|p| walk_map.contains_key(p));
                missing += miss;
                found
            }
            Ok(Some(Playlist::Smart(smart))) => {
                let mut found = Vec::new();
                for path in crate::playlist_rules::evaluate(&smart.rules, index) {
                    if walk_map.contains_key(&path) {
                        found.push(path);
                    } else {
                        // evaluate() reads the index, which may be stale
                        // relative to the walk — the walk is still the
                        // oracle, so a rule match the walk never saw is
                        // dropped, not fabricated.
                        missing += 1;
                    }
                }
                found
            }
            Ok(None) => {
                errors.push((slug.clone(), format!("playlist '{slug}' not found")));
                continue;
            }
            Err(e) => {
                errors.push((slug.clone(), format!("{e:#}")));
                continue;
            }
        };

        for path in &resolved {
            if present.insert(path.clone()) {
                if let Some(src) = walk_map.get(path) {
                    sources.push(src.clone());
                }
            }
        }
        playlist_tracks.push((slug.clone(), resolved));
    }

    EffectiveSet { sources, playlist_tracks, missing_playlist_tracks: missing, playlist_errors: errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_config::Subscriptions;
    use crate::library_index::IndexedTrack;
    use crate::playlist::ManualPlaylist;
    use crate::selection::{SelectionMode, SelectionRule};

    fn tempdir_under_target(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("sync-set-{label}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn store_with_manual(root: &Path, slug: &str, tracks: &[&str]) -> PlaylistStore {
        let store = PlaylistStore::open(root.to_path_buf()).unwrap();
        let manual = ManualPlaylist {
            slug: slug.to_string(),
            name: slug.to_string(),
            tracks: tracks.iter().map(PathBuf::from).collect(),
            skipped_unsafe: 0,
        };
        store.save(&Playlist::Manual(manual)).unwrap();
        store
    }

    fn src(path: &str) -> SourceEntry {
        SourceEntry { path: PathBuf::from(path), mtime: 1, size: 10 }
    }

    fn indexed(artist: &str) -> IndexedTrack {
        IndexedTrack {
            mtime: 1,
            size: 10,
            artist: artist.to_string(),
            album_artist: String::new(),
            album: "Album".to_string(),
            genre: "G".to_string(),
            title: String::new(),
            duration_ms: 0,
            year: None,
        }
    }

    fn subs(slugs: &[&str]) -> Subscriptions {
        Subscriptions { version: 1, playlists: slugs.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn subscribed_tracks_outside_include_scope_still_sync() {
        let root = tempdir_under_target("outside-scope");
        let store = store_with_manual(&root, "mix", &["B/song.flac"]);

        let source_root = Path::new("/music");
        let walk = vec![src("/music/A/song.flac"), src("/music/B/song.flac")];
        let mut index = LibraryIndex::empty(source_root.to_path_buf());
        index.files.insert(PathBuf::from("/music/A/song.flac"), indexed("Artist A"));
        index.files.insert(PathBuf::from("/music/B/song.flac"), indexed("Artist B"));

        let selection = Selection {
            version: 1,
            mode: SelectionMode::Include,
            rules: vec![SelectionRule::Artist { name: "Artist A".into() }],
        };

        let effective = compute(walk, &selection, &subs(&["mix"]), &store, &index, source_root);

        let paths: HashSet<_> = effective.sources.iter().map(|e| e.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("/music/A/song.flac")), "in-scope track stays");
        assert!(paths.contains(&PathBuf::from("/music/B/song.flac")), "playlist track outside scope still syncs");
        assert_eq!(effective.sources.len(), 2);
        assert!(effective.playlist_errors.is_empty());
        assert_eq!(effective.missing_playlist_tracks, 0);
    }

    #[test]
    fn playlists_only_device_empty_include_plus_subscription() {
        let root = tempdir_under_target("playlists-only");
        let store = store_with_manual(&root, "mix", &["A/1.flac", "B/2.flac"]);

        let source_root = Path::new("/music");
        let walk = vec![src("/music/A/1.flac"), src("/music/B/2.flac")];
        let mut index = LibraryIndex::empty(source_root.to_path_buf());
        index.files.insert(PathBuf::from("/music/A/1.flac"), indexed("Artist A"));
        index.files.insert(PathBuf::from("/music/B/2.flac"), indexed("Artist B"));

        // Include mode with zero rules matches nothing — scope is empty.
        let selection = Selection { version: 1, mode: SelectionMode::Include, rules: vec![] };

        let effective = compute(walk, &selection, &subs(&["mix"]), &store, &index, source_root);

        assert_eq!(
            effective.sources.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
            vec![PathBuf::from("/music/A/1.flac"), PathBuf::from("/music/B/2.flac")],
            "empty scope + one playlist -> sources == playlist tracks exactly"
        );
    }

    #[test]
    fn union_dedups_and_keeps_walk_order_then_appends() {
        let root = tempdir_under_target("dedup-order");
        // Playlist order: C (already in scope), B, D (both out of scope).
        let store = store_with_manual(&root, "mix", &["C.flac", "B.flac", "D.flac"]);

        let source_root = Path::new("/music");
        let walk = vec![src("/music/A.flac"), src("/music/B.flac"), src("/music/C.flac"), src("/music/D.flac")];
        let mut index = LibraryIndex::empty(source_root.to_path_buf());
        for (p, artist) in [
            ("/music/A.flac", "Keep"),
            ("/music/B.flac", "Skip"),
            ("/music/C.flac", "Keep"),
            ("/music/D.flac", "Skip"),
        ] {
            index.files.insert(PathBuf::from(p), indexed(artist));
        }

        let selection = Selection {
            version: 1,
            mode: SelectionMode::Include,
            rules: vec![SelectionRule::Artist { name: "Keep".into() }],
        };

        let effective = compute(walk, &selection, &subs(&["mix"]), &store, &index, source_root);

        assert_eq!(
            effective.sources.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
            vec![
                PathBuf::from("/music/A.flac"),
                PathBuf::from("/music/C.flac"),
                PathBuf::from("/music/B.flac"),
                PathBuf::from("/music/D.flac"),
            ],
            "walk order for in-scope tracks, then playlist-only additions in playlist order, deduped"
        );
    }

    #[test]
    fn playlist_track_absent_from_walk_counts_missing_never_invents_source() {
        let root = tempdir_under_target("missing-track");
        let store = store_with_manual(&root, "mix", &["A.flac", "gone.flac"]);

        let source_root = Path::new("/music");
        let walk = vec![src("/music/A.flac")]; // "gone.flac" was never walked
        let mut index = LibraryIndex::empty(source_root.to_path_buf());
        index.files.insert(PathBuf::from("/music/A.flac"), indexed("Artist"));

        let effective =
            compute(walk, &Selection::all(), &subs(&["mix"]), &store, &index, source_root);

        assert_eq!(effective.sources.len(), 1);
        assert_eq!(effective.sources[0].path, PathBuf::from("/music/A.flac"));
        assert_eq!(effective.missing_playlist_tracks, 1);
        assert!(
            !effective.sources.iter().any(|e| e.path.ends_with("gone.flac")),
            "a playlist entry absent from the walk must never fabricate a SourceEntry"
        );
    }

    #[test]
    fn unknown_slug_is_error_not_failure() {
        let root = tempdir_under_target("unknown-slug");
        let store = PlaylistStore::open(root.clone()).unwrap(); // no playlists saved

        let source_root = Path::new("/music");
        let walk = vec![src("/music/A.flac")];
        let index = LibraryIndex::empty(source_root.to_path_buf());

        let effective = compute(
            walk,
            &Selection::all(),
            &subs(&["does-not-exist"]),
            &store,
            &index,
            source_root,
        );

        assert_eq!(effective.sources.len(), 1, "sync proceeds despite the bad subscription");
        assert_eq!(effective.playlist_errors.len(), 1);
        assert_eq!(effective.playlist_errors[0].0, "does-not-exist");
        assert!(effective.playlist_tracks.is_empty());
        assert_eq!(effective.missing_playlist_tracks, 0);
    }
}
