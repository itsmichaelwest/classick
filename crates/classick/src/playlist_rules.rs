//! Smart-playlist rule evaluation.
//!
//! Declarative match rules (artist/album/genre/year, is/contains/gte/lte),
//! limits, and ordering, evaluated host-side against the library index at
//! sync/preview time into a static device playlist. See `docs/superpowers/
//! specs/2026-07-17-library-playlists-devices-design.md` §1 for the file
//! shape this mirrors.

use crate::library_index::{IndexedTrack, LibraryIndex};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub const RULES_VERSION: u32 = 1;

/// How the rule list combines: AND (`All`) or OR (`Any`). An empty rule
/// list is vacuously true under `All` (matches everything) and vacuously
/// false under `Any` (matches nothing) — ordinary boolean-logic semantics,
/// not a special case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Artist,
    Album,
    Genre,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Is,
    Contains,
    Gte,
    Lte,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    pub value: String,
}

/// `Bytes`/`Tracks` applied to the already-ordered result as a straight
/// prefix cut — see `apply_limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    Bytes(u64),
    Tracks(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    RecentlyModified,
    RandomStable,
    #[default]
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmartRules {
    pub version: u32,
    pub matching: Match,
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub limit: Option<Limit>,
    #[serde(default)]
    pub order: Order,
    #[serde(default)]
    pub seed: u64,
}

/// Evaluate `rules` against `index`: filter, order, then apply the limit.
/// Infallible — an empty index (or a rule set that matches nothing) simply
/// produces an empty `Vec`.
pub fn evaluate(rules: &SmartRules, index: &LibraryIndex) -> Vec<PathBuf> {
    let mut matched: Vec<(&PathBuf, &IndexedTrack)> =
        index.files.iter().filter(|(_, track)| matches_rules(rules, track)).collect();

    order_matches(&mut matched, rules.order, rules.seed);

    apply_limit(matched, rules.limit)
}

fn matches_rules(rules: &SmartRules, track: &IndexedTrack) -> bool {
    match rules.matching {
        Match::All => rules.rules.iter().all(|r| rule_matches(r, track)),
        Match::Any => rules.rules.iter().any(|r| rule_matches(r, track)),
    }
}

fn rule_matches(rule: &Rule, track: &IndexedTrack) -> bool {
    match rule.field {
        Field::Artist => text_matches(rule.op, &rule.value, &track.artist),
        Field::Album => text_matches(rule.op, &rule.value, &track.album),
        Field::Genre => text_matches(rule.op, &rule.value, &track.genre),
        Field::Year => year_matches(rule.op, &rule.value, track.year),
    }
}

/// Case-insensitive text comparison. `Gte`/`Lte` compare lexicographically
/// (case-folded) — mainly useful for `Is`/`Contains`; string range queries
/// are an edge case the type system doesn't forbid but the UI isn't
/// expected to expose for text fields.
fn text_matches(op: Op, rule_value: &str, track_value: &str) -> bool {
    let track_value = track_value.to_lowercase();
    let rule_value = rule_value.to_lowercase();
    match op {
        Op::Is => track_value == rule_value,
        Op::Contains => track_value.contains(&rule_value),
        Op::Gte => track_value >= rule_value,
        Op::Lte => track_value <= rule_value,
    }
}

/// Numeric year comparison. A track with no cached year (`None`) never
/// matches any year rule — absent from year-filtered playlists is the
/// honest outcome, not a guessed match. A non-numeric rule `value` matches
/// nothing at all (every track fails), rather than silently degrading to
/// "match everything" or panicking.
fn year_matches(op: Op, rule_value: &str, track_year: Option<i32>) -> bool {
    let Some(track_year) = track_year else { return false };
    let Ok(rule_year) = rule_value.trim().parse::<i32>() else { return false };
    match op {
        Op::Is | Op::Contains => track_year == rule_year,
        Op::Gte => track_year >= rule_year,
        Op::Lte => track_year <= rule_year,
    }
}

fn order_matches(items: &mut [(&PathBuf, &IndexedTrack)], order: Order, seed: u64) {
    match order {
        Order::Alpha => items.sort_by(|a, b| a.0.cmp(b.0)),
        Order::RecentlyModified => {
            items.sort_by(|a, b| b.1.mtime.cmp(&a.1.mtime).then_with(|| a.0.cmp(b.0)))
        }
        Order::RandomStable => {
            items.sort_by_key(|(path, _)| stable_hash(seed, path));
        }
    }
}

/// Deterministic (same seed + path -> same value, every run, every
/// process) but not cryptographic: `DefaultHasher::new()` starts from
/// fixed keys, unlike `RandomState`'s per-process randomization, so the
/// ordering a `random_stable` playlist produces today is exactly the
/// ordering it produces on the next sync — no device-side churn.
fn stable_hash(seed: u64, path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    path.hash(&mut hasher);
    hasher.finish()
}

/// Cut the already-ordered result to `limit`. `Tracks(n)` keeps the first
/// `n`. `Bytes(budget)` walks the order and keeps a straight prefix,
/// stopping at the first track that would push the running total over
/// budget — track granularity, NOT album-atomic. Album/show atomicity (so a
/// smart playlist never ships half an album to the device) is the fit
/// engine's job at sync time, not this evaluator's.
fn apply_limit(items: Vec<(&PathBuf, &IndexedTrack)>, limit: Option<Limit>) -> Vec<PathBuf> {
    match limit {
        None => items.into_iter().map(|(p, _)| p.clone()).collect(),
        Some(Limit::Tracks(n)) => items.into_iter().take(n).map(|(p, _)| p.clone()).collect(),
        Some(Limit::Bytes(budget)) => {
            let mut used = 0u64;
            let mut out = Vec::new();
            for (path, track) in items {
                let next = used.saturating_add(track.size);
                if next > budget {
                    break;
                }
                used = next;
                out.push(path.clone());
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn track(artist: &str, genre: &str, year: Option<i32>, mtime: i64, size: u64) -> IndexedTrack {
        IndexedTrack {
            mtime,
            size,
            artist: artist.to_string(),
            album_artist: String::new(),
            album: "Album".to_string(),
            genre: genre.to_string(),
            title: String::new(),
            duration_ms: 0,
            year,
        }
    }

    fn index_with(entries: Vec<(&str, IndexedTrack)>) -> LibraryIndex {
        let mut idx = LibraryIndex::empty(PathBuf::from("/music"));
        for (path, t) in entries {
            idx.files.insert(PathBuf::from(path), t);
        }
        idx
    }

    fn rules(matching: Match, rules: Vec<Rule>) -> SmartRules {
        SmartRules { version: RULES_VERSION, matching, rules, limit: None, order: Order::Alpha, seed: 0 }
    }

    #[test]
    fn all_vs_any_matching() {
        let index = index_with(vec![
            ("/music/both.flac", track("Brian Eno", "Ambient", None, 1, 1)),
            ("/music/artist_only.flac", track("Brian Eno", "Rock", None, 1, 1)),
            ("/music/genre_only.flac", track("Bowie", "Ambient", None, 1, 1)),
            ("/music/neither.flac", track("Bowie", "Rock", None, 1, 1)),
        ]);
        let two_rules = vec![
            Rule { field: Field::Genre, op: Op::Is, value: "Ambient".into() },
            Rule { field: Field::Artist, op: Op::Contains, value: "eno".into() },
        ];

        let all = evaluate(&rules(Match::All, two_rules.clone()), &index);
        assert_eq!(all, vec![PathBuf::from("/music/both.flac")]);

        let any = evaluate(&rules(Match::Any, two_rules), &index);
        assert_eq!(
            any,
            vec![
                PathBuf::from("/music/artist_only.flac"),
                PathBuf::from("/music/both.flac"),
                PathBuf::from("/music/genre_only.flac"),
            ]
        );
    }

    #[test]
    fn year_gte_and_non_numeric_rule_matches_nothing() {
        let index = index_with(vec![
            ("/music/old.flac", track("A", "G", Some(1995), 1, 1)),
            ("/music/boundary.flac", track("A", "G", Some(2000), 1, 1)),
            ("/music/new.flac", track("A", "G", Some(2010), 1, 1)),
            ("/music/unknown.flac", track("A", "G", None, 1, 1)),
        ]);

        let gte = rules(Match::All, vec![Rule { field: Field::Year, op: Op::Gte, value: "2000".into() }]);
        assert_eq!(
            evaluate(&gte, &index),
            vec![PathBuf::from("/music/boundary.flac"), PathBuf::from("/music/new.flac")],
            "years >= 2000 only; the yearless track never matches"
        );

        let non_numeric =
            rules(Match::All, vec![Rule { field: Field::Year, op: Op::Gte, value: "abc".into() }]);
        assert!(evaluate(&non_numeric, &index).is_empty(), "non-numeric rule value matches nothing");
    }

    #[test]
    fn random_stable_is_deterministic_and_seed_sensitive() {
        let entries: Vec<(&str, IndexedTrack)> = (0..10)
            .map(|i| {
                let path: &'static str = Box::leak(format!("/music/t{i}.flac").into_boxed_str());
                (path, track("A", "G", None, 1, 1))
            })
            .collect();
        let index = index_with(entries);

        let mut seed42a = rules(Match::All, vec![]);
        seed42a.order = Order::RandomStable;
        seed42a.seed = 42;
        let seed42b = seed42a.clone();
        let mut seed7 = seed42a.clone();
        seed7.seed = 7;

        let run1 = evaluate(&seed42a, &index);
        let run2 = evaluate(&seed42b, &index);
        assert_eq!(run1, run2, "same seed must reproduce the same order every time");
        assert_eq!(run1.len(), 10);

        let run3 = evaluate(&seed7, &index);
        assert_ne!(run1, run3, "a different seed must (for 10 tracks) produce a different order");
    }

    #[test]
    fn byte_limit_takes_prefix_in_order() {
        let index = index_with(vec![
            ("/music/a.flac", track("A", "G", None, 1, 100)),
            ("/music/b.flac", track("A", "G", None, 1, 100)),
            ("/music/c.flac", track("A", "G", None, 1, 100)),
        ]);
        let mut r = rules(Match::All, vec![]);
        r.limit = Some(Limit::Bytes(250));

        assert_eq!(
            evaluate(&r, &index),
            vec![PathBuf::from("/music/a.flac"), PathBuf::from("/music/b.flac")],
            "alpha order, 250-byte budget fits exactly the first two 100-byte tracks"
        );
    }

    #[test]
    fn rules_json_round_trip_with_defaults() {
        let minimal = r#"{"version":1,"matching":"all","rules":[]}"#;
        let decoded: SmartRules = serde_json::from_str(minimal).unwrap();
        assert_eq!(decoded.limit, None);
        assert_eq!(decoded.order, Order::Alpha);
        assert_eq!(decoded.seed, 0);

        let encoded = serde_json::to_string(&decoded).unwrap();
        let round_tripped: SmartRules = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, round_tripped);
    }

    #[test]
    fn tracks_limit_takes_prefix_count() {
        let index = index_with(vec![
            ("/music/a.flac", track("A", "G", None, 1, 1)),
            ("/music/b.flac", track("A", "G", None, 1, 1)),
            ("/music/c.flac", track("A", "G", None, 1, 1)),
        ]);
        let mut r = rules(Match::All, vec![]);
        r.limit = Some(Limit::Tracks(2));
        assert_eq!(
            evaluate(&r, &index),
            vec![PathBuf::from("/music/a.flac"), PathBuf::from("/music/b.flac")]
        );
    }

    #[test]
    fn recently_modified_orders_mtime_desc() {
        let index = index_with(vec![
            ("/music/oldest.flac", track("A", "G", None, 1, 1)),
            ("/music/newest.flac", track("A", "G", None, 300, 1)),
            ("/music/middle.flac", track("A", "G", None, 150, 1)),
        ]);
        let mut r = rules(Match::All, vec![]);
        r.order = Order::RecentlyModified;
        assert_eq!(
            evaluate(&r, &index),
            vec![
                PathBuf::from("/music/newest.flac"),
                PathBuf::from("/music/middle.flac"),
                PathBuf::from("/music/oldest.flac"),
            ]
        );
    }

    #[test]
    fn empty_index_evaluates_to_empty() {
        let index = LibraryIndex::empty(PathBuf::from("/music"));
        let r = rules(Match::All, vec![Rule { field: Field::Artist, op: Op::Is, value: "X".into() }]);
        assert!(evaluate(&r, &index).is_empty());
    }
}
