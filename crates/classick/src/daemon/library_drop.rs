use crate::library_index::{IndexedTrack, LibraryIndex};
use crate::playlist::ManualPlaylist;
use crate::selection::{Selection, SelectionMode, SelectionRule, SELECTION_VERSION};
use anyhow::{bail, Result};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;

pub(crate) const MAX_DROP_RULES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceSelectionMutation {
    pub selection: Selection,
    pub matched_paths: Vec<String>,
    pub selection_changed: bool,
}

pub(crate) fn validate_drop_rules(rules: &[SelectionRule]) -> Result<Vec<SelectionRule>> {
    if rules.is_empty() {
        bail!("a library drop must contain at least one rule");
    }
    if rules.len() > MAX_DROP_RULES {
        bail!("a library drop may contain at most {MAX_DROP_RULES} rules");
    }
    canonicalize(rules)
}

pub(crate) fn add_rules_to_selection(
    current: &Selection,
    rules: &[SelectionRule],
    index: &LibraryIndex,
) -> Result<DeviceSelectionMutation> {
    let dropped = validate_drop_rules(rules)?;
    let matched_paths = resolved_relative_paths(index, &dropped);
    let selection = match current.mode {
        SelectionMode::All => current.clone(),
        SelectionMode::Include => {
            let mut combined = current.rules.clone();
            combined.extend(dropped);
            Selection {
                version: SELECTION_VERSION,
                mode: SelectionMode::Include,
                rules: canonicalize(&combined)?,
            }
        }
        SelectionMode::Exclude => Selection {
            version: SELECTION_VERSION,
            mode: SelectionMode::Exclude,
            rules: relaxed_exclusions(&current.rules, &matched_paths, index)?,
        },
    };
    Ok(DeviceSelectionMutation {
        selection_changed: selection != *current,
        selection,
        matched_paths,
    })
}

pub(crate) fn append_rules_to_manual(
    current: &ManualPlaylist,
    rules: &[SelectionRule],
    index: &LibraryIndex,
) -> Result<(ManualPlaylist, Vec<String>)> {
    let rules = validate_drop_rules(rules)?;
    let existing = current
        .tracks
        .iter()
        .map(|path| portable(path).to_lowercase())
        .collect::<HashSet<_>>();
    let appended = resolved_relative_paths(index, &rules)
        .into_iter()
        .filter(|path| !existing.contains(&path.to_lowercase()))
        .collect::<Vec<_>>();
    let mut next = current.clone();
    next.tracks.extend(appended.iter().map(PathBuf::from));
    Ok((next, appended))
}

fn relaxed_exclusions(
    current: &[SelectionRule],
    dropped_paths: &[String],
    index: &LibraryIndex,
) -> Result<Vec<SelectionRule>> {
    let current = canonicalize(current)?;
    let dropped = dropped_paths
        .iter()
        .map(|path| path.to_lowercase())
        .collect::<HashSet<_>>();
    let mut next = Vec::new();
    for rule in current {
        let matching = indexed_matches(index, &rule);
        if !matching
            .iter()
            .any(|(path, _)| dropped.contains(&path.to_lowercase()))
        {
            next.push(rule);
            continue;
        }
        if matches!(rule, SelectionRule::Album { .. }) {
            continue;
        }
        let mut albums = matching
            .iter()
            .filter(|(_, track)| {
                !matching.iter().any(|(path, candidate)| {
                    same_album(track, candidate) && dropped.contains(&path.to_lowercase())
                })
            })
            .map(|(_, track)| SelectionRule::Album {
                artist: track.facts().effective_artist().trim().to_string(),
                album: track.album.trim().to_string(),
            })
            .collect::<Vec<_>>();
        next.append(&mut albums);
    }
    canonicalize(&next)
}

fn indexed_matches<'a>(
    index: &'a LibraryIndex,
    rule: &SelectionRule,
) -> Vec<(String, &'a IndexedTrack)> {
    let selection = Selection {
        version: SELECTION_VERSION,
        mode: SelectionMode::Include,
        rules: vec![rule.clone()],
    };
    index
        .files
        .iter()
        .filter(|(_, track)| selection.wants(&track.facts()))
        .filter_map(|(path, track)| {
            path.strip_prefix(&index.source_root)
                .ok()
                .map(|relative| (portable(relative), track))
        })
        .collect()
}

fn resolved_relative_paths(index: &LibraryIndex, rules: &[SelectionRule]) -> Vec<String> {
    let selection = Selection {
        version: SELECTION_VERSION,
        mode: SelectionMode::Include,
        rules: rules.to_vec(),
    };
    let mut paths = index
        .files
        .iter()
        .filter(|(_, track)| selection.wants(&track.facts()))
        .filter_map(|(path, _)| path.strip_prefix(&index.source_root).ok())
        .map(portable)
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| natural_path_cmp(left, right));
    paths.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    paths
}

fn canonicalize(rules: &[SelectionRule]) -> Result<Vec<SelectionRule>> {
    let mut normalized = rules
        .iter()
        .map(normalize_rule)
        .collect::<Result<Vec<_>>>()?;
    normalized.sort_by(rule_cmp);
    normalized.dedup_by(|left, right| rule_key(left) == rule_key(right));
    let artists = normalized
        .iter()
        .filter_map(|rule| match rule {
            SelectionRule::Artist { name } => Some(name.to_lowercase()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    normalized.retain(|rule| match rule {
        SelectionRule::Album { artist, .. } => !artists.contains(&artist.to_lowercase()),
        _ => true,
    });
    Ok(normalized)
}

fn normalize_rule(rule: &SelectionRule) -> Result<SelectionRule> {
    let component = |value: &str| -> Result<String> {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 256 {
            bail!("drop rule components must contain 1 to 256 Unicode scalars");
        }
        Ok(value.to_string())
    };
    Ok(match rule {
        SelectionRule::Artist { name } => SelectionRule::Artist {
            name: component(name)?,
        },
        SelectionRule::Album { artist, album } => SelectionRule::Album {
            artist: component(artist)?,
            album: component(album)?,
        },
        SelectionRule::Genre { name } => SelectionRule::Genre {
            name: component(name)?,
        },
    })
}

fn rule_key(rule: &SelectionRule) -> (u8, String, String) {
    match rule {
        SelectionRule::Artist { name } => (0, name.to_lowercase(), String::new()),
        SelectionRule::Album { artist, album } => (1, artist.to_lowercase(), album.to_lowercase()),
        SelectionRule::Genre { name } => (2, name.to_lowercase(), String::new()),
    }
}

fn rule_cmp(left: &SelectionRule, right: &SelectionRule) -> Ordering {
    rule_key(left)
        .cmp(&rule_key(right))
        .then_with(|| format!("{left:?}").cmp(&format!("{right:?}")))
}

fn same_album(left: &IndexedTrack, right: &IndexedTrack) -> bool {
    left.facts()
        .effective_artist()
        .eq_ignore_ascii_case(right.facts().effective_artist())
        && left.album.eq_ignore_ascii_case(&right.album)
}

fn portable(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn natural_path_cmp(left: &str, right: &str) -> Ordering {
    natural_fold_cmp(&left.to_lowercase(), &right.to_lowercase()).then_with(|| left.cmp(right))
}

fn natural_fold_cmp(left: &str, right: &str) -> Ordering {
    let (mut left, mut right) = (left.as_bytes(), right.as_bytes());
    while !left.is_empty() && !right.is_empty() {
        if left[0].is_ascii_digit() && right[0].is_ascii_digit() {
            let ln = left
                .iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(left.len());
            let rn = right
                .iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(right.len());
            let ltrim = left[..ln].iter().position(|b| *b != b'0').unwrap_or(ln - 1);
            let rtrim = right[..rn]
                .iter()
                .position(|b| *b != b'0')
                .unwrap_or(rn - 1);
            let ord = (ln - ltrim)
                .cmp(&(rn - rtrim))
                .then_with(|| left[ltrim..ln].cmp(&right[rtrim..rn]));
            if ord != Ordering::Equal {
                return ord;
            }
            left = &left[ln..];
            right = &right[rn..];
        } else {
            let ord = left[0].cmp(&right[0]);
            if ord != Ordering::Equal {
                return ord;
            }
            left = &left[1..];
            right = &right[1..];
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_index::IndexedTrack;

    fn artist(name: &str) -> SelectionRule {
        SelectionRule::Artist { name: name.into() }
    }
    fn album(artist: &str, album: &str) -> SelectionRule {
        SelectionRule::Album {
            artist: artist.into(),
            album: album.into(),
        }
    }
    fn genre(name: &str) -> SelectionRule {
        SelectionRule::Genre { name: name.into() }
    }
    fn track(artist: &str, album: &str, genre: &str) -> IndexedTrack {
        IndexedTrack {
            mtime: 1,
            size: 1,
            artist: artist.into(),
            album_artist: String::new(),
            album: album.into(),
            genre: genre.into(),
            title: String::new(),
            duration_ms: 0,
            year: None,
        }
    }
    fn index_with(entries: &[(&str, IndexedTrack)]) -> LibraryIndex {
        let mut index = LibraryIndex::empty(PathBuf::from("/music"));
        for (path, track) in entries {
            index.files.insert(PathBuf::from(path), track.clone());
        }
        index
    }
    fn index() -> LibraryIndex {
        index_with(&[
            ("/music/Birdy/Fire/01.flac", track("Birdy", "Fire", "Pop")),
            ("/music/Birdy/Fire/10.flac", track("Birdy", "Fire", "Pop")),
            ("/music/Birdy/Fire/02.flac", track("Birdy", "Fire", "Rock")),
            ("/music/Birdy/Young/01.flac", track("Birdy", "Young", "Pop")),
            ("/music/Adele/25/01.flac", track("Adele", "25", "Pop")),
        ])
    }
    fn selection(mode: SelectionMode, rules: Vec<SelectionRule>) -> Selection {
        Selection {
            version: SELECTION_VERSION,
            mode,
            rules,
        }
    }
    fn manual(paths: &[&str]) -> ManualPlaylist {
        ManualPlaylist {
            slug: "mix".into(),
            name: "Mix".into(),
            tracks: paths.iter().map(PathBuf::from).collect(),
            skipped_unsafe: 0,
        }
    }

    #[test]
    fn exclude_artist_expands_only_to_unaffected_albums() {
        let current = selection(SelectionMode::Exclude, vec![artist(" birdy ")]);
        let changed =
            add_rules_to_selection(&current, &[album("BIRDY", "Fire")], &index()).unwrap();
        assert_eq!(changed.selection.rules, vec![album("Birdy", "Young")]);
        assert_eq!(
            changed.matched_paths,
            [
                "Birdy/Fire/01.flac",
                "Birdy/Fire/02.flac",
                "Birdy/Fire/10.flac"
            ]
        );
    }

    #[test]
    fn all_is_unchanged_but_still_resolves_matches() {
        let current = Selection::all();
        let changed = add_rules_to_selection(&current, &[genre("pop")], &index()).unwrap();
        assert_eq!(changed.selection, current);
        assert!(!changed.selection_changed);
        assert_eq!(changed.matched_paths.len(), 4);
    }

    #[test]
    fn manual_append_deduplicates_and_naturally_orders_batch() {
        let current = manual(&["birdy/fire/01.flac", "Birdy/Fire/02.flac"]);
        let (next, appended) =
            append_rules_to_manual(&current, &[artist("Birdy")], &index()).unwrap();
        assert_eq!(appended, ["Birdy/Fire/10.flac", "Birdy/Young/01.flac"]);
        assert_eq!(next.tracks.len(), 4);
    }

    #[test]
    fn validation_normalizes_deduplicates_orders_and_rejects_bad_input() {
        assert!(validate_drop_rules(&[]).is_err());
        assert!(validate_drop_rules(&vec![artist("a"); 65]).is_err());
        assert!(validate_drop_rules(&[artist("  ")]).is_err());
        assert!(validate_drop_rules(&[genre(&"x".repeat(257))]).is_err());
        assert_eq!(
            validate_drop_rules(&[
                genre(" Pop "),
                artist(" birdy "),
                artist("BIRDY"),
                album("Birdy", "Fire")
            ])
            .unwrap(),
            [artist("BIRDY"), genre("Pop")]
        );
    }

    #[test]
    fn include_union_is_case_insensitive_and_drops_covered_albums() {
        let current = selection(
            SelectionMode::Include,
            vec![album("Birdy", "Fire"), genre("Jazz")],
        );
        let changed =
            add_rules_to_selection(&current, &[artist("birdy"), genre("jazz")], &index()).unwrap();
        assert_eq!(changed.selection.rules, [artist("birdy"), genre("Jazz")]);
    }

    #[test]
    fn genre_and_mixed_album_relaxation_preserve_only_unaffected_albums() {
        let current = selection(SelectionMode::Exclude, vec![genre("Pop")]);
        let changed =
            add_rules_to_selection(&current, &[album("Birdy", "Fire")], &index()).unwrap();
        assert_eq!(
            changed.selection.rules,
            [album("Adele", "25"), album("Birdy", "Young")]
        );
    }

    #[test]
    fn unmatched_rules_preserve_exclusions_and_append_nothing() {
        let current = selection(SelectionMode::Exclude, vec![artist("Birdy")]);
        let changed = add_rules_to_selection(&current, &[artist("Nobody")], &index()).unwrap();
        assert_eq!(changed.selection.rules, [artist("Birdy")]);
        let (next, appended) =
            append_rules_to_manual(&manual(&["old.flac"]), &[artist("Nobody")], &index()).unwrap();
        assert!(appended.is_empty());
        assert_eq!(next.tracks, [PathBuf::from("old.flac")]);
    }

    #[test]
    fn shuffled_index_insertion_has_deterministic_natural_output() {
        let entries = [
            ("/music/A/1/10.flac", track("A", "1", "G")),
            ("/music/A/1/2.flac", track("A", "1", "G")),
        ];
        let reversed = [entries[1].clone(), entries[0].clone()];
        let left = resolved_relative_paths(&index_with(&entries), &[artist("A")]);
        let right = resolved_relative_paths(&index_with(&reversed), &[artist("A")]);
        assert_eq!(left, ["A/1/2.flac", "A/1/10.flac"]);
        assert_eq!(left, right);
    }
}
