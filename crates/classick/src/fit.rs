//! Album-atomic first-fit plan filter (pure, no I/O).
//!
//! Once `manifest::diff` has produced a plan (`Vec<Action>`), the apply loop
//! may not have room on the device for every `Add`. This module decides
//! which `Add`s to keep vs. defer to a later sync, at *album* granularity —
//! an album either syncs in full or not at all, never half-copied. `Remove`,
//! `Modify`, `Unchanged`, and `MetadataOnly` actions always pass through
//! untouched; they don't consume the fit budget (space they free/require is
//! the caller's concern — see `plan_fit`'s doc comment).
//!
//! Deliberately pure: `album_tag_of` is injected by the caller (backed by a
//! `library_index` lookup in the apply loop, a bare closure in tests) so
//! this module never touches disk.

use crate::manifest::Action;
use std::collections::HashMap;
use std::path::Path;

/// Album grouping key for a source path: the album tag if non-empty, else
/// the lossy string of the path's parent directory.
pub fn album_key(source_path: &Path, album_tag: Option<&str>) -> String {
    if let Some(tag) = album_tag {
        if !tag.is_empty() {
            return tag.to_string();
        }
    }
    source_path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// An album that didn't fit the budget this run. Whole-album counts, not
/// per-track — `plan_fit` never splits an album across kept/deferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredAlbum {
    pub key: String,
    pub tracks: usize,
    pub bytes: u64,
}

pub struct FitOutcome {
    pub kept: Vec<Action>,
    pub deferred: Vec<DeferredAlbum>,
}

/// Filter `actions` down to what fits `budget_bytes`, deferring whole albums
/// (never partial) when they don't.
///
/// `budget_bytes: None` means free space couldn't be queried; everything is
/// kept unconditionally (fail open — we'd rather attempt a sync that later
/// hits a real "disk full" error than silently drop tracks on unqueryable
/// devices). Non-`Add` actions (`Remove`/`Modify`/`Unchanged`/`MetadataOnly`)
/// always pass through regardless of budget — only `Add`s compete for space.
///
/// `Add`s are grouped by [`album_key`] in first-seen order, then walked
/// first-fit: an album is kept in full if its summed `SourceEntry` sizes fit
/// the budget remaining *after* previously-kept albums, else it (and every
/// track in it) is deferred whole and a smaller album further down the list
/// still gets its chance at the leftover space.
///
/// Budget semantics belong to the caller: `budget_bytes` here is the final
/// number to spend on `Add`s. This function does NOT add back space that a
/// `Remove`/`Modify` in the same `actions` list frees up — the caller
/// (apply loop, per the trust-package plan's Task 8) is expected to have
/// already folded `Σ(remove entry sizes)` and the [`reserve_bytes`] safety
/// margin into `budget_bytes` before calling in.
pub fn plan_fit(
    actions: Vec<Action>,
    budget_bytes: Option<u64>,
    album_tag_of: impl Fn(&Path) -> Option<String>,
) -> FitOutcome {
    let Some(budget) = budget_bytes else {
        return FitOutcome {
            kept: actions,
            deferred: Vec::new(),
        };
    };

    // Pass 1: compute each action's album key up front (Adds only — one
    // `album_tag_of` call per Add, regardless of how many passes follow).
    let keys: Vec<Option<String>> = actions
        .iter()
        .map(|action| match action {
            Action::Add(entry) => {
                let tag = album_tag_of(&entry.path);
                Some(album_key(&entry.path, tag.as_deref()))
            }
            _ => None,
        })
        .collect();

    // Pass 2: group by key, preserving first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut totals: HashMap<String, (u64, usize)> = HashMap::new();
    for (action, key) in actions.iter().zip(&keys) {
        if let (Action::Add(entry), Some(key)) = (action, key) {
            let slot = totals.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                (0, 0)
            });
            slot.0 += entry.size;
            slot.1 += 1;
        }
    }

    // Pass 3: first-fit each album, in first-seen order.
    let mut remaining = budget;
    let mut fits: HashMap<String, bool> = HashMap::new();
    let mut deferred = Vec::new();
    for key in &order {
        let (bytes, tracks) = totals[key];
        if bytes <= remaining {
            remaining -= bytes;
            fits.insert(key.clone(), true);
        } else {
            fits.insert(key.clone(), false);
            deferred.push(DeferredAlbum {
                key: key.clone(),
                tracks,
                bytes,
            });
        }
    }

    // Pass 4: rebuild the kept list in the original relative order — every
    // non-Add passes through, every Add is kept iff its album fit.
    let mut kept = Vec::with_capacity(actions.len());
    for (action, key) in actions.into_iter().zip(keys) {
        match key {
            Some(k) => {
                if fits.get(&k).copied().unwrap_or(false) {
                    kept.push(action);
                }
            }
            None => kept.push(action),
        }
    }

    FitOutcome { kept, deferred }
}

/// Safety margin to hold back below reported free space: `max(FIT_RESERVE_MIN_BYTES,
/// total * FIT_RESERVE_FRACTION)`. See the constants' doc comments in `lib.rs`
/// for the FAT32-at-100% rationale.
pub fn reserve_bytes(total_bytes: u64) -> u64 {
    let fraction = (total_bytes as f64 * crate::FIT_RESERVE_FRACTION) as u64;
    crate::FIT_RESERVE_MIN_BYTES.max(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestEntry;
    use crate::source::SourceEntry;
    use crate::FIT_RESERVE_MIN_BYTES;
    use std::path::PathBuf;

    fn src(path: &str, size: u64) -> SourceEntry {
        SourceEntry {
            path: PathBuf::from(path),
            mtime: 1_700_000_000,
            size,
        }
    }

    fn entry(path: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            source_path: PathBuf::from(path),
            source_mtime: 1_700_000_000,
            source_size: size,
            source_fingerprint: "blake3:aa".to_string(),
            ipod_dbid: 1,
            ipod_relpath: r"iPod_Control\Music\F01\AAAA.m4a".to_string(),
            source_known: true,
            audio_fingerprint: String::new(),
            encoder: "unknown".to_string(),
            encoder_version: String::new(),
            source_format: "flac".to_string(),
        }
    }

    /// Identifying path for an action — used only so tests can assert on
    /// ordering/membership without requiring `Action: PartialEq`.
    fn action_path(a: &Action) -> PathBuf {
        match a {
            Action::Add(s) => s.path.clone(),
            Action::Modify(s, _) => s.path.clone(),
            Action::Remove(e) => e.source_path.clone(),
            Action::Unchanged(e) => e.source_path.clone(),
            Action::MetadataOnly { source, .. } => source.path.clone(),
        }
    }

    fn no_tags(_: &Path) -> Option<String> {
        None
    }

    #[test]
    fn no_budget_keeps_everything() {
        let actions = vec![
            Action::Add(src("/m/A/Album/01.flac", 40)),
            Action::Remove(entry("/m/A/Old/02.flac", 40)),
            Action::Add(src("/m/B/Album/01.flac", 40)),
        ];
        let input_paths: Vec<_> = actions.iter().map(action_path).collect();
        let outcome = plan_fit(actions, None, no_tags);
        assert!(outcome.deferred.is_empty());
        let kept_paths: Vec<_> = outcome.kept.iter().map(action_path).collect();
        assert_eq!(kept_paths, input_paths);
    }

    #[test]
    fn album_never_splits() {
        // Album A: 3 tracks x 40 bytes = 120; budget 100 -> whole album
        // deferred, NOT 2 tracks kept + 1 deferred.
        let actions = vec![
            Action::Add(src("/m/Artist/Album/01.flac", 40)),
            Action::Add(src("/m/Artist/Album/02.flac", 40)),
            Action::Add(src("/m/Artist/Album/03.flac", 40)),
        ];
        let outcome = plan_fit(actions, Some(100), no_tags);
        assert!(
            outcome.kept.is_empty(),
            "120 bytes must not fit a 100-byte budget"
        );
        assert_eq!(outcome.deferred.len(), 1);
        assert_eq!(outcome.deferred[0].key, "/m/Artist/Album");
        assert_eq!(outcome.deferred[0].tracks, 3);
        assert_eq!(outcome.deferred[0].bytes, 120);
    }

    #[test]
    fn first_fit_skips_big_album_but_keeps_later_small_one() {
        // Order: A(120), B(60), C(50), D(30); budget 100 ->
        // A deferred, B kept (40 left), C deferred (50 > 40), D kept (30 <= 40).
        let actions = vec![
            Action::Add(src("/m/A/01.flac", 120)),
            Action::Add(src("/m/B/01.flac", 60)),
            Action::Add(src("/m/C/01.flac", 50)),
            Action::Add(src("/m/D/01.flac", 30)),
        ];
        let outcome = plan_fit(actions, Some(100), no_tags);

        let kept_paths: Vec<_> = outcome.kept.iter().map(action_path).collect();
        assert_eq!(
            kept_paths,
            vec![PathBuf::from("/m/B/01.flac"), PathBuf::from("/m/D/01.flac")]
        );

        let deferred_keys: Vec<_> = outcome.deferred.iter().map(|d| d.key.clone()).collect();
        assert_eq!(deferred_keys, vec!["/m/A".to_string(), "/m/C".to_string()]);
    }

    #[test]
    fn removes_and_modifies_always_kept() {
        let actions = vec![
            Action::Remove(entry("/m/Old/01.flac", 1_000_000)),
            Action::Modify(
                src("/m/Changed/01.flac", 1_000_000),
                entry("/m/Changed/01.flac", 900_000),
            ),
            Action::Unchanged(entry("/m/Same/01.flac", 500_000)),
            Action::MetadataOnly {
                source: src("/m/Tag/01.flac", 500_000),
                entry: entry("/m/Tag/01.flac", 500_000),
            },
            Action::Add(src("/m/Big/Album/01.flac", 1000)), // won't fit the tiny budget below
        ];
        let outcome = plan_fit(actions, Some(10), no_tags);

        assert_eq!(
            outcome.kept.len(),
            4,
            "all non-Add actions kept regardless of budget"
        );
        assert!(outcome.kept.iter().all(|a| !matches!(a, Action::Add(_))));
        assert_eq!(outcome.deferred.len(), 1);
    }

    #[test]
    fn album_key_prefers_tag_falls_back_to_parent_dir() {
        assert_eq!(
            album_key(Path::new("/m/Artist/Album X/01.flac"), Some("Album X")),
            "Album X"
        );
        assert_eq!(
            album_key(Path::new("/m/Artist/Album X/01.flac"), None),
            "/m/Artist/Album X"
        );
    }

    /// An empty (but present) album tag is treated the same as no tag at
    /// all — `album_key` must fall back to the parent-dir key, not key on
    /// the empty string, so e.g. two same-named albums under different
    /// parents with blank tags don't collapse into one bucket.
    #[test]
    fn album_key_treats_empty_tag_as_absent_and_falls_back_to_parent_dir() {
        assert_eq!(
            album_key(Path::new("/m/Artist/Album X/01.flac"), Some("")),
            "/m/Artist/Album X"
        );
    }

    /// Exact-fit boundary: an album whose total size equals exactly what's
    /// left in the budget (`bytes <= remaining`, not `<`) must be kept, not
    /// deferred — off-by-one here would defer albums that fit perfectly.
    #[test]
    fn album_exactly_matching_remaining_budget_is_kept() {
        let actions = vec![Action::Add(src("/m/Artist/Album/01.flac", 100))];
        let outcome = plan_fit(actions, Some(100), no_tags);
        assert!(
            outcome.deferred.is_empty(),
            "exact-fit album must not be deferred"
        );
        assert_eq!(outcome.kept.len(), 1);
    }

    #[test]
    fn reserve_floor_and_fraction() {
        assert_eq!(
            reserve_bytes(10 * 1024 * 1024 * 1024),
            FIT_RESERVE_MIN_BYTES
        ); // 2% of 10GB < 512MB
        assert_eq!(
            reserve_bytes(100 * 1024 * 1024 * 1024),
            (100.0 * 1024.0 * 1024.0 * 1024.0 * 0.02) as u64
        );
    }

    #[test]
    fn first_seen_order_governs_grouping_and_deferral_priority() {
        // Two paths under different directories share an album tag; a third,
        // untagged path forms its own album. First-seen order (by which
        // action introduces a key) must decide who gets first crack at the
        // budget, and same-key totals must accumulate across paths.
        let tag_of = |p: &Path| -> Option<String> {
            if p.to_string_lossy().contains("trackA") {
                Some("Album Two".to_string())
            } else {
                None
            }
        };
        let actions = vec![
            Action::Add(src("/m/dirX/trackA-1.flac", 60)), // "Album Two", seen first
            Action::Add(src("/m/dirY/01.flac", 60)),       // "/m/dirY", seen second
            Action::Add(src("/m/dirX/trackA-2.flac", 60)), // "Album Two" again (same key)
        ];
        let outcome = plan_fit(actions, Some(100), tag_of);

        // "Album Two" totals 120 bytes and was seen first -> deferred whole.
        assert_eq!(outcome.deferred.len(), 1);
        assert_eq!(outcome.deferred[0].key, "Album Two");
        assert_eq!(outcome.deferred[0].tracks, 2);
        assert_eq!(outcome.deferred[0].bytes, 120);

        // "/m/dirY" (60 bytes), seen second, still gets its shot at the
        // untouched 100-byte budget and fits.
        let kept_paths: Vec<_> = outcome.kept.iter().map(action_path).collect();
        assert_eq!(kept_paths, vec![PathBuf::from("/m/dirY/01.flac")]);
    }

    #[test]
    fn kept_actions_preserve_original_relative_order() {
        let actions = vec![
            Action::Remove(entry("/m/Old/01.flac", 10)),
            Action::Add(src("/m/Album1/01.flac", 20)),
            Action::Unchanged(entry("/m/Same/01.flac", 10)),
            Action::Add(src("/m/Album2/01.flac", 20)),
        ];
        let expected: Vec<_> = actions.iter().map(action_path).collect();
        // Budget large enough that both Adds fit -> nothing deferred, order
        // must exactly mirror the input order (non-Adds interleaved as-is).
        let outcome = plan_fit(actions, Some(1000), no_tags);
        assert!(outcome.deferred.is_empty());
        let kept_paths: Vec<_> = outcome.kept.iter().map(action_path).collect();
        assert_eq!(kept_paths, expected);
    }
}
