//! Playlists: manual (ordered track lists) and smart (rule-based). Manual
//! playlists are portable `.m3u8` files with SOURCE-RELATIVE paths; smart
//! playlists are `.rules.json` files evaluated host-side at sync time
//! (evaluator itself lives in `playlist_rules`, Task 2). Store root is
//! `<config>/classick/playlists/` — see `docs/superpowers/specs/
//! 2026-07-17-library-playlists-devices-design.md` §1.

use crate::playlist_rules::SmartRules;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Lowercase, alphanumerics kept, runs of anything else collapse to a single
/// `-`, leading/trailing `-` trimmed. An empty result (e.g. all-non-ASCII or
/// all-punctuation input) falls back to `"playlist"`. Does NOT guarantee
/// uniqueness against other playlists — that's `PlaylistStore::unique_slug`'s
/// job.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = true; // suppresses a leading '-'
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "playlist".to_string()
    } else {
        out
    }
}

/// A manual playlist: an ordered, user-curated track list. `tracks` are
/// SOURCE-RELATIVE paths (portable across machines / re-rooted libraries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualPlaylist {
    pub slug: String,
    pub name: String,
    pub tracks: Vec<PathBuf>,
    /// Number of lines dropped by `parse_m3u8` for failing
    /// `is_safe_relative` (absolute path or `..` component). Parse
    /// metadata only — not persisted by `render_m3u8`.
    pub skipped_unsafe: usize,
}

/// True if `p` is safe to join onto a trusted root: relative (no Windows
/// prefix component, e.g. `C:\`), and free of `..` (`ParentDir`)
/// components. `.` (`CurDir`) components are tolerated. Playlist track
/// paths cross a trust boundary — they come from user-editable `.m3u8`
/// files that also travel between machines via the device mirror — so
/// this is checked before ever joining onto `source_root`.
pub(crate) fn is_safe_relative(p: &Path) -> bool {
    use std::path::Component;
    for component in p.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return false,
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    true
}

/// A smart playlist: declarative rules, evaluated host-side against the
/// library index at sync/preview time into a static device playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmartPlaylist {
    pub slug: String,
    pub name: String,
    pub rules: SmartRules,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playlist {
    Manual(ManualPlaylist),
    Smart(SmartPlaylist),
}

impl Playlist {
    pub fn name(&self) -> &str {
        match self {
            Playlist::Manual(m) => &m.name,
            Playlist::Smart(s) => &s.name,
        }
    }

    pub fn slug(&self) -> &str {
        match self {
            Playlist::Manual(m) => &m.slug,
            Playlist::Smart(s) => &s.slug,
        }
    }
}

/// Parse an `.m3u8` playlist. Tolerates a leading BOM, CRLF line endings,
/// `#EXTINF` (and any other `#`-prefixed directive besides `#PLAYLIST:`,
/// which is ignored), and blank lines. `#PLAYLIST:<name>` sets the display
/// name; absent, `slug` is used as the fallback name. Backslashes in track
/// lines are normalized to forward slashes so playlists authored on Windows
/// round-trip on macOS and vice versa. Never fails on well-formed UTF-8
/// text; returns `Result` for symmetry with the rest of the store API and to
/// leave room for stricter validation later.
pub fn parse_m3u8(text: &str, slug: &str) -> Result<ManualPlaylist> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut name: Option<String> = None;
    let mut tracks = Vec::new();
    let mut skipped_unsafe = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#PLAYLIST:") {
            name = Some(rest.trim().to_string());
            continue;
        }
        if line.starts_with('#') {
            continue; // #EXTM3U, #EXTINF, or any other directive: ignored
        }
        let track = PathBuf::from(line.replace('\\', "/"));
        if !is_safe_relative(&track) {
            tracing::warn!("playlist {slug}: skipped unsafe path {line:?}");
            skipped_unsafe += 1;
            continue;
        }
        tracks.push(track);
    }
    Ok(ManualPlaylist {
        slug: slug.to_string(),
        name: name.unwrap_or_else(|| slug.to_string()),
        tracks,
        skipped_unsafe,
    })
}

/// Render a manual playlist back to `.m3u8` text: `#EXTM3U` + `#PLAYLIST:`
/// header, then one forward-slash-normalized relative path per line.
pub fn render_m3u8(p: &ManualPlaylist) -> String {
    let mut out = format!("#EXTM3U\n#PLAYLIST:{}\n", p.name);
    for track in &p.tracks {
        out.push_str(&track.to_string_lossy().replace('\\', "/"));
        out.push('\n');
    }
    out
}

/// Resolve a manual playlist's source-relative tracks against `source_root`,
/// keeping only tracks that still exist. `existing` is an injected
/// existence check so tests don't need a real filesystem. Returns the
/// resolved absolute paths (order preserved) plus a count of tracks skipped
/// because they no longer exist or because they failed `is_safe_relative` —
/// never an error; a stale or hostile playlist entry is expected steady-
/// state, not a failure. The safety check here is belt-and-braces: `parse_m3u8`
/// already filters unsafe lines, but `ManualPlaylist` values can also be
/// constructed directly (e.g. from a device mirror), so this is the last
/// line of defense before `source_root.join(rel)`.
pub fn resolve_manual(
    p: &ManualPlaylist,
    source_root: &Path,
    existing: &dyn Fn(&Path) -> bool,
) -> (Vec<PathBuf>, usize) {
    let mut found = Vec::with_capacity(p.tracks.len());
    let mut missing = 0;
    for rel in &p.tracks {
        if !is_safe_relative(rel) {
            missing += 1;
            continue;
        }
        let abs = source_root.join(rel);
        if existing(&abs) {
            found.push(abs);
        } else {
            missing += 1;
        }
    }
    (found, missing)
}

/// File-backed store of playlists under a root directory: one `<slug>.m3u8`
/// per manual playlist, one `<slug>.rules.json` per smart playlist.
pub struct PlaylistStore {
    root: PathBuf,
    /// Files skipped by the last `list()` call, with the read/parse error
    /// that caused the skip. Reset (cleared and repopulated) on every
    /// `list()` call. Exposed as an owned snapshot rather than a borrowed
    /// slice: the field lives behind a `RefCell` (interior mutability so
    /// `list(&self)` doesn't need `&mut self`), and a `RefCell` cannot hand
    /// out a `&[..]` tied to `&self` without `unsafe` — which is off the
    /// table for anything above `ipod/` per repo convention.
    last_errors: RefCell<Vec<(PathBuf, String)>>,
}

impl PlaylistStore {
    /// Open (creating on demand) a playlist store rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)
            .with_context(|| format!("create playlists dir {}", root.display()))?;
        Ok(Self { root, last_errors: RefCell::new(Vec::new()) })
    }

    /// `<config dir>/classick/playlists/` — beside `selection.json` and
    /// `config.toml`.
    pub fn default_root() -> Result<PathBuf> {
        let dir = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("could not resolve config dir"))?;
        Ok(dir.join(crate::PROJECT_DIR).join("playlists"))
    }

    /// Snapshot of the errors recorded by the most recent `list()` call.
    pub fn last_errors(&self) -> Vec<(PathBuf, String)> {
        self.last_errors.borrow().clone()
    }

    fn manual_path(&self, slug: &str) -> PathBuf {
        self.root.join(format!("{slug}.m3u8"))
    }

    fn smart_path(&self, slug: &str) -> PathBuf {
        self.root.join(format!("{slug}.rules.json"))
    }

    fn slug_taken(&self, slug: &str) -> bool {
        self.manual_path(slug).exists() || self.smart_path(slug).exists()
    }

    /// Slugify `name`, then disambiguate against files already on disk
    /// (either kind) with a `-2`, `-3`, ... suffix.
    pub fn unique_slug(&self, name: &str) -> Result<String> {
        let base = slugify(name);
        if !self.slug_taken(&base) {
            return Ok(base);
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base}-{n}");
            if !self.slug_taken(&candidate) {
                return Ok(candidate);
            }
            n += 1;
        }
    }

    /// List every playlist in the store (`*.m3u8` + `*.rules.json`).
    /// A file that can't be read or parsed is skipped rather than failing
    /// the whole listing; see `last_errors()` for what was skipped and why.
    pub fn list(&self) -> Result<Vec<Playlist>> {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let entries = std::fs::read_dir(&self.root)
            .with_context(|| format!("read playlists dir {}", self.root.display()))?;
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push((self.root.clone(), format!("{e:#}")));
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if let Some(slug) = file_name.strip_suffix(".m3u8") {
                match Self::read_manual(&path, slug) {
                    Ok(p) => out.push(Playlist::Manual(p)),
                    Err(e) => errors.push((path, format!("{e:#}"))),
                }
            } else if file_name.strip_suffix(".rules.json").is_some() {
                match Self::read_smart(&path) {
                    Ok(p) => out.push(Playlist::Smart(p)),
                    Err(e) => errors.push((path, format!("{e:#}"))),
                }
            }
        }
        *self.last_errors.borrow_mut() = errors;
        Ok(out)
    }

    /// Load one playlist by slug, if either kind exists on disk.
    pub fn load(&self, slug: &str) -> Result<Option<Playlist>> {
        let manual_path = self.manual_path(slug);
        if manual_path.exists() {
            return Ok(Some(Playlist::Manual(Self::read_manual(&manual_path, slug)?)));
        }
        let smart_path = self.smart_path(slug);
        if smart_path.exists() {
            return Ok(Some(Playlist::Smart(Self::read_smart(&smart_path)?)));
        }
        Ok(None)
    }

    /// Save a playlist, atomically (tmp file + rename). Manual playlists
    /// write `<slug>.m3u8`; smart playlists write `<slug>.rules.json`.
    pub fn save(&self, p: &Playlist) -> Result<()> {
        match p {
            Playlist::Manual(m) => atomic_write(&self.manual_path(&m.slug), render_m3u8(m).as_bytes()),
            Playlist::Smart(s) => {
                let json = serde_json::to_string_pretty(s)?;
                atomic_write(&self.smart_path(&s.slug), json.as_bytes())
            }
        }
    }

    /// Delete a playlist by slug (either kind). Returns whether anything was
    /// actually removed.
    pub fn delete(&self, slug: &str) -> Result<bool> {
        let mut deleted = false;
        let manual_path = self.manual_path(slug);
        if manual_path.exists() {
            std::fs::remove_file(&manual_path)
                .with_context(|| format!("delete {}", manual_path.display()))?;
            deleted = true;
        }
        let smart_path = self.smart_path(slug);
        if smart_path.exists() {
            std::fs::remove_file(&smart_path)
                .with_context(|| format!("delete {}", smart_path.display()))?;
            deleted = true;
        }
        Ok(deleted)
    }

    fn read_manual(path: &Path, slug: &str) -> Result<ManualPlaylist> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        parse_m3u8(&text, slug)
    }

    fn read_smart(path: &Path) -> Result<SmartPlaylist> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }
}

/// Write `bytes` to `path` atomically: tmp file + fsync + rename, same
/// pattern as `manifest::save_atomic` / `selection::save_atomic`.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let f = std::fs::File::create(&tmp).with_context(|| format!("create temp file {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, bytes)?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir under `target/`, per the `device_state.rs` counter
    /// pattern (PID alone collides under parallel test execution).
    fn tempdir_under_target(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("playlist-{label}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn slugify_basics() {
        assert_eq!(slugify("Favorites"), "favorites");
        assert_eq!(slugify("Bla Bla Bla!"), "bla-bla-bla");
        assert_eq!(slugify("日本語のみ"), "playlist"); // non-ascii-alnum collapses away
        assert_eq!(slugify("  --  "), "playlist");
    }

    #[test]
    fn m3u8_round_trip_preserves_order_and_name() {
        let p = ManualPlaylist {
            slug: "gym".into(),
            name: "Gym".into(),
            tracks: vec!["Artist/Album/01.flac".into(), "B/C/02.flac".into()],
            skipped_unsafe: 0,
        };
        let parsed = parse_m3u8(&render_m3u8(&p), "gym").unwrap();
        assert_eq!(parsed.name, "Gym");
        assert_eq!(parsed.tracks, p.tracks);
        assert_eq!(parsed.skipped_unsafe, 0);
    }

    #[test]
    fn m3u8_parse_tolerates_bom_crlf_extinf_and_backslashes() {
        let text = "\u{feff}#EXTM3U\r\n#PLAYLIST:Mix\r\n#EXTINF:123,Artist - Title\r\nA\\B\\01.flac\r\n\r\n";
        let p = parse_m3u8(text, "mix").unwrap();
        assert_eq!(p.name, "Mix");
        assert_eq!(p.tracks, vec![PathBuf::from("A/B/01.flac")]);
    }

    #[test]
    fn m3u8_parse_falls_back_to_slug_when_no_playlist_header() {
        let p = parse_m3u8("#EXTM3U\nA/1.flac\n", "no-header").unwrap();
        assert_eq!(p.name, "no-header");
        assert_eq!(p.tracks, vec![PathBuf::from("A/1.flac")]);
        assert_eq!(p.skipped_unsafe, 0);
    }

    #[test]
    fn is_safe_relative_rejects_absolute_and_parent_dir() {
        // Load-bearing checks: absolute (`RootDir`/`Prefix`) and `..`
        // (`ParentDir`) components are rejected outright.
        assert!(is_safe_relative(Path::new("a/b.flac")));
        assert!(is_safe_relative(Path::new("./a/b.flac")));
        assert!(!is_safe_relative(Path::new("../x")));
        assert!(!is_safe_relative(Path::new("a/../../x")));
        assert!(!is_safe_relative(Path::new("/etc/passwd")));
        // On Unix, `C:\x` isn't parsed as a Windows prefix — it's just a
        // single weird-but-relative `Normal("C:\\x")` component, so this
        // passes here. It's included as documentation of that platform
        // difference, not as a security guarantee: the ParentDir/absolute
        // checks above are what actually carries the load on Unix, and on
        // Windows itself `Component::Prefix` would catch this case.
        assert!(is_safe_relative(Path::new("C:\\x")));
    }

    #[test]
    fn m3u8_parse_skips_unsafe_paths() {
        let text = "#EXTM3U\n../evil.flac\n/etc/passwd\nok/1.flac\n";
        let p = parse_m3u8(text, "hostile").unwrap();
        assert_eq!(p.tracks, vec![PathBuf::from("ok/1.flac")]);
        assert_eq!(p.skipped_unsafe, 2);
    }

    #[test]
    fn resolve_manual_skips_missing_and_counts() {
        let p = ManualPlaylist {
            slug: "x".into(),
            name: "X".into(),
            tracks: vec!["a/1.flac".into(), "gone/2.flac".into()],
            skipped_unsafe: 0,
        };
        let (found, missing) = resolve_manual(&p, Path::new("/src"), &|p| !p.starts_with("/src/gone"));
        assert_eq!(found, vec![PathBuf::from("/src/a/1.flac")]);
        assert_eq!(missing, 1);
    }

    #[test]
    fn resolve_manual_all_present_has_zero_missing() {
        let p = ManualPlaylist {
            slug: "x".into(),
            name: "X".into(),
            tracks: vec!["a/1.flac".into()],
            skipped_unsafe: 0,
        };
        let (found, missing) = resolve_manual(&p, Path::new("/src"), &|_| true);
        assert_eq!(found, vec![PathBuf::from("/src/a/1.flac")]);
        assert_eq!(missing, 0);
    }

    #[test]
    fn resolve_manual_never_escapes_source_root_for_hostile_tracks() {
        // A ManualPlaylist constructed directly (not via parse_m3u8), as it
        // would be if a hostile playlist arrived via the device mirror
        // rather than a hand-edited .m3u8 file. `existing` always returns
        // true so the only thing keeping paths inside source_root is
        // resolve_manual's own is_safe_relative filter.
        let p = ManualPlaylist {
            slug: "hostile".into(),
            name: "Hostile".into(),
            tracks: vec![
                PathBuf::from("../../etc/passwd"),
                PathBuf::from("/etc/passwd"),
                PathBuf::from("ok/1.flac"),
            ],
            skipped_unsafe: 0,
        };
        let (found, missing) = resolve_manual(&p, Path::new("/src"), &|_| true);
        assert_eq!(found, vec![PathBuf::from("/src/ok/1.flac")]);
        assert!(found.iter().all(|p| p.starts_with("/src")));
        assert_eq!(missing, 2);
    }

    #[test]
    fn store_saves_lists_loads_deletes_and_uniquifies() {
        let root = tempdir_under_target("roundtrip");
        let store = PlaylistStore::open(root.clone()).unwrap();

        // Manual playlist takes the plain slug.
        let slug1 = store.unique_slug("Gym").unwrap();
        assert_eq!(slug1, "gym");
        let manual =
            ManualPlaylist { slug: slug1, name: "Gym".into(), tracks: vec!["A/1.flac".into()], skipped_unsafe: 0 };
        store.save(&Playlist::Manual(manual.clone())).unwrap();

        // A second playlist named "Gym" (this time smart) collides on slug
        // with the manual one and must uniquify against it.
        let slug2 = store.unique_slug("Gym").unwrap();
        assert_eq!(slug2, "gym-2");
        let smart = SmartPlaylist {
            slug: slug2,
            name: "Gym".into(),
            rules: SmartRules {
                version: crate::playlist_rules::RULES_VERSION,
                matching: crate::playlist_rules::Match::All,
                rules: vec![],
                limit: None,
                order: crate::playlist_rules::Order::Alpha,
                seed: 0,
            },
        };
        store.save(&Playlist::Smart(smart.clone())).unwrap();

        let mut listed = store.list().unwrap();
        listed.sort_by(|a, b| a.slug().cmp(b.slug()));
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0], Playlist::Manual(manual.clone()));
        assert_eq!(listed[1], Playlist::Smart(smart));
        assert!(store.last_errors().is_empty());

        assert_eq!(store.load("gym").unwrap(), Some(Playlist::Manual(manual)));
        assert_eq!(store.load("does-not-exist").unwrap(), None);

        assert!(store.delete("gym").unwrap());
        assert_eq!(store.load("gym").unwrap(), None);
        assert!(!store.delete("gym").unwrap(), "second delete is a no-op");

        // A corrupt file on disk is skipped, not fatal to the listing.
        let broken = root.join("broken.rules.json");
        std::fs::write(&broken, b"{ not json").unwrap();
        let listed_with_corrupt = store.list().unwrap();
        assert_eq!(listed_with_corrupt.len(), 1, "only gym-2 remains valid");
        let errors = store.last_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, broken);
    }

    #[test]
    fn default_root_is_under_config_dir_playlists() {
        let root = PlaylistStore::default_root().unwrap();
        assert!(root.ends_with(std::path::Path::new("classick").join("playlists")));
    }
}
