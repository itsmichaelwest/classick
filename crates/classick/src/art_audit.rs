//! `--verify-artwork` audit mode: diagnostic + permanent regression harness
//! for the cover-art pipeline bugs documented in LEARNINGS.md ("macOS
//! Artwork Root Cause", the `itdb_write`-deletes-ithmb finding). For every
//! manifest entry with a known source, cross-checks three independent
//! signals and reports only the INCONSISTENT combinations:
//!
//!  1. does the source file have embedded cover art (same lofty-backed probe
//!     `apply_loop` uses via [`crate::transcode::probe`] /
//!     [`crate::transcode::has_embedded_art`]);
//!  2. does the matching DB track (by `ipod_dbid`) have `has_artwork` set;
//!  3. does the expected on-disk ithmb thumbnail file exist under
//!     `iPod_Control/Artwork` (only checked for models whose cover-art
//!     format id is verified — see [`expected_ithmb_basenames`]).
//!
//! A track whose source has no art AND whose DB track has no art is
//! consistent (OK), not a failure — only mismatches are reported.
//!
//! Read-only: opens the device's iTunesDB via [`OwnedDb::open`] but never
//! calls `.write()` — per LEARNINGS.md, `itdb_write` on a rewrite path can
//! delete ithmb thumbnails, which would make this diagnostic tool corrupt
//! the very state it's trying to inspect.

use crate::config::Config;
use crate::device_state;
use crate::ffi;
use crate::ipod::db::OwnedDb;
use crate::ipod::device;
use crate::preflight;
use crate::progress::Progress;
use crate::transcode;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct ArtAuditReport {
    pub checked: usize,
    pub ok: usize,
    pub failures: Vec<ArtAuditFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtAuditFailure {
    pub source_path: PathBuf,
    pub reason: String,
}

/// Classify one track's three-signal state into `None` (consistent) or
/// `Some(reason)` (inconsistency found). Pure and exhaustively tested — see
/// the truth table in the test module. `ithmb_missing` is only consulted
/// when both `source_has_art` and `db_has_art` are true; it names the first
/// expected ithmb file that wasn't found on the mount.
pub fn classify(
    source_has_art: bool,
    db_has_art: bool,
    ithmb_missing: Option<&str>,
) -> Option<String> {
    if !source_has_art && db_has_art {
        return Some("source has no embedded art".to_string());
    }
    if source_has_art && !db_has_art {
        return Some("db track has_artwork=0".to_string());
    }
    if source_has_art && db_has_art {
        if let Some(name) = ithmb_missing {
            return Some(format!("ithmb file missing: {name}"));
        }
    }
    None
}

/// Pure aggregation: fold a sequence of already-computed per-track signals
/// into an `ArtAuditReport` via [`classify`]. Kept separate from
/// `verify_artwork`'s I/O (mount resolution, FFI, filesystem checks) so the
/// aggregation logic — the part most worth regression-testing — needs no
/// hardware or fake-mount harness to exercise.
pub fn build_report<'a, I>(entries: I) -> ArtAuditReport
where
    I: IntoIterator<Item = (&'a Path, bool, bool, Option<&'a str>)>,
{
    let mut report = ArtAuditReport::default();
    for (source_path, source_has_art, db_has_art, ithmb_missing) in entries {
        report.checked += 1;
        match classify(source_has_art, db_has_art, ithmb_missing) {
            Some(reason) => report.failures.push(ArtAuditFailure {
                source_path: source_path.to_path_buf(),
                reason,
            }),
            None => report.ok += 1,
        }
    }
    report
}

/// Cover-art ithmb basenames a device model is known to expect under
/// `iPod_Control/Artwork`. Only the iPod Classic Late-2009 (MC293/MC297)
/// cover-art format (F1069) is verified on-device (see LEARNINGS.md "macOS
/// Artwork Root Cause"). `sysinfo_provision`'s per-model `TABLE` ships
/// `ImageSpecifications` with several `FormatId`s per model (photo/video
/// thumbnail sizes as well as cover art), and we don't have a verified
/// mapping of which one libgpod actually uses for track cover art on those
/// other models — guessing wrong would produce false-positive failures
/// against unverified data, so unmapped models simply skip the ithmb check
/// (the source/db-only checks still run for them).
pub fn expected_ithmb_basenames(model_num_str: &str) -> Vec<&'static str> {
    match model_num_str {
        "MC293" | "MC297" => vec!["F1069_1.ithmb"],
        _ => Vec::new(),
    }
}

/// First name in `expected` whose file doesn't exist under `artwork_dir`,
/// or `None` if `expected` is empty or every file is present.
fn missing_ithmb<'a>(artwork_dir: &Path, expected: &[&'a str]) -> Option<&'a str> {
    expected
        .iter()
        .copied()
        .find(|name| !artwork_dir.join(name).exists())
}

/// Resolve the iPod mount, non-interactively (no retry prompt — this is a
/// one-shot diagnostic run, not an interactive sync). Mirrors
/// `preflight::resolve_ipod_mount`'s two branches minus the Retry/Abort loop.
fn resolve_mount(config: &Config) -> Result<String> {
    match &config.ipod {
        Some(m) => {
            let p = preflight::ensure_trailing_separator(m);
            if crate::ipod::layout::itunes_db_path(Path::new(&p)).exists() {
                Ok(p)
            } else {
                Err(anyhow!(
                    "explicit --ipod {p} does not contain iPod_Control/iTunes/iTunesDB"
                ))
            }
        }
        None => crate::ipod::detect_ipod_mount(),
    }
}

/// Walk the DB's track list once and build a `dbid -> has_artwork` map.
/// `has_artwork == 1` is libgpod's tri-state "yes"; anything else (0 =
/// unknown, 2 = explicitly no) reads as "no art" here — see `examples/art-audit.rs`.
fn collect_has_artwork(db: &OwnedDb) -> HashMap<u64, bool> {
    let mut map = HashMap::new();
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            let dbid = (*t).dbid as u64;
            let has = (*t).has_artwork == 1;
            map.insert(dbid, has);
            node = (*node).next;
        }
    }
    map
}

/// Run the full audit against the connected (or explicitly `--ipod`) device:
/// resolve the per-device manifest exactly as `apply_loop::run` does
/// (identity -> `device_state` paths), open the DB read-only, cross-check
/// every `source_known` manifest entry, log each failure + a summary line,
/// and return the aggregated report. Callers decide how to surface a
/// non-empty `failures` list (the orchestrator returns `Err` so the process
/// exit code is non-zero — see `orchestrator::orchestrate`).
pub fn verify_artwork(config: &Config, progress: &Progress) -> Result<ArtAuditReport> {
    let mount = resolve_mount(config)?;
    let identity = device::resolve_libgpod_identity(Path::new(&mount))
        .context("resolving device identity for artwork audit")?;
    let serial = device_state::sanitize_serial(&identity.firewire_guid);
    let source_location = crate::apply_loop::configured_source_location(config)?;
    let loaded_manifest = crate::apply_loop::manifest_store(config, Path::new(&mount), &serial)?
        .load(&source_location)?
        .manifest;

    let db = OwnedDb::open(Path::new(&mount)).context("opening iTunesDB for artwork audit")?;
    let artwork_by_dbid = collect_has_artwork(&db);
    let artwork_dir = Path::new(&mount).join("iPod_Control").join("Artwork");
    let expected_ithmb = expected_ithmb_basenames(&identity.model_num_str);

    let mut report = ArtAuditReport::default();
    let mut resolved: Vec<(PathBuf, bool, bool, Option<&str>)> = Vec::new();

    for entry in loaded_manifest.tracks.iter().filter(|e| e.source_known) {
        let probe = transcode::probe(&entry.source_path, &config.ffmpeg);
        let source_has_art = match probe {
            Ok(p) => transcode::has_embedded_art(&p),
            Err(e) => {
                report.checked += 1;
                report.failures.push(ArtAuditFailure {
                    source_path: entry.source_path.clone(),
                    reason: format!("source probe failed: {e:#}"),
                });
                continue;
            }
        };
        let db_has_art = artwork_by_dbid
            .get(&entry.ipod_dbid)
            .copied()
            .unwrap_or(false);
        let ithmb_missing = if source_has_art && db_has_art {
            missing_ithmb(&artwork_dir, &expected_ithmb)
        } else {
            None
        };
        resolved.push((
            entry.source_path.clone(),
            source_has_art,
            db_has_art,
            ithmb_missing,
        ));
    }

    let sub = build_report(
        resolved
            .iter()
            .map(|(p, s, d, i)| (p.as_path(), *s, *d, *i)),
    );
    report.checked += sub.checked;
    report.ok += sub.ok;
    report.failures.extend(sub.failures);

    for failure in &report.failures {
        progress.log(format!(
            "verify-artwork: {} - {}",
            failure.source_path.display(),
            failure.reason
        ));
    }
    progress.log(format!(
        "verify-artwork: checked={} ok={} failures={}",
        report.checked,
        report.ok,
        report.failures.len(),
    ));

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify: full truth table -------------------------------------

    #[test]
    fn classify_both_absent_is_ok() {
        assert_eq!(classify(false, false, None), None);
        // ithmb_missing is irrelevant when the source/db pair is already
        // consistent-absent — never consulted, never surfaces a failure.
        assert_eq!(classify(false, false, Some("F1069_1.ithmb")), None);
    }

    #[test]
    fn classify_db_has_art_but_source_does_not() {
        assert_eq!(
            classify(false, true, None),
            Some("source has no embedded art".to_string())
        );
        assert_eq!(
            classify(false, true, Some("F1069_1.ithmb")),
            Some("source has no embedded art".to_string())
        );
    }

    #[test]
    fn classify_source_has_art_but_db_does_not() {
        assert_eq!(
            classify(true, false, None),
            Some("db track has_artwork=0".to_string())
        );
        assert_eq!(
            classify(true, false, Some("F1069_1.ithmb")),
            Some("db track has_artwork=0".to_string())
        );
    }

    #[test]
    fn classify_both_present_and_ithmb_present_is_ok() {
        assert_eq!(classify(true, true, None), None);
    }

    #[test]
    fn classify_both_present_but_ithmb_missing() {
        assert_eq!(
            classify(true, true, Some("F1069_1.ithmb")),
            Some("ithmb file missing: F1069_1.ithmb".to_string())
        );
    }

    // --- build_report: aggregation ---------------------------------------

    #[test]
    fn build_report_counts_ok_and_failures() {
        let p1 = PathBuf::from("/music/a.flac");
        let p2 = PathBuf::from("/music/b.flac");
        let p3 = PathBuf::from("/music/c.flac");
        let entries = vec![
            (p1.as_path(), true, true, None),                  // ok
            (p2.as_path(), true, false, None),                 // failure
            (p3.as_path(), true, true, Some("F1069_1.ithmb")), // failure
        ];
        let report = build_report(entries);
        assert_eq!(report.checked, 3);
        assert_eq!(report.ok, 1);
        assert_eq!(report.failures.len(), 2);
        assert_eq!(report.failures[0].source_path, p2);
        assert_eq!(report.failures[0].reason, "db track has_artwork=0");
        assert_eq!(report.failures[1].source_path, p3);
        assert_eq!(
            report.failures[1].reason,
            "ithmb file missing: F1069_1.ithmb"
        );
    }

    #[test]
    fn build_report_invariant_checked_equals_ok_plus_failures() {
        let p = PathBuf::from("/x.flac");
        let cases: Vec<(bool, bool, Option<&str>)> = vec![
            (false, false, None),
            (false, true, None),
            (true, false, None),
            (true, true, None),
            (true, true, Some("F1069_1.ithmb")),
        ];
        let entries: Vec<(&Path, bool, bool, Option<&str>)> = cases
            .iter()
            .map(|(s, d, i)| (p.as_path(), *s, *d, *i))
            .collect();
        let report = build_report(entries);
        assert_eq!(report.checked, report.ok + report.failures.len());
    }

    #[test]
    fn build_report_empty_is_all_zero() {
        let report = build_report(Vec::new());
        assert_eq!(report.checked, 0);
        assert_eq!(report.ok, 0);
        assert!(report.failures.is_empty());
    }

    // --- expected_ithmb_basenames -----------------------------------------

    #[test]
    fn expected_ithmb_basenames_known_classic_models() {
        assert_eq!(expected_ithmb_basenames("MC293"), vec!["F1069_1.ithmb"]);
        assert_eq!(expected_ithmb_basenames("MC297"), vec!["F1069_1.ithmb"]);
    }

    #[test]
    fn expected_ithmb_basenames_unmapped_model_is_empty() {
        assert!(expected_ithmb_basenames("MB029").is_empty());
        assert!(expected_ithmb_basenames("unknown-model").is_empty());
    }

    // --- missing_ithmb: real filesystem checks -----------------------------

    fn scratch_dir(label: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("classick-art-audit-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn missing_ithmb_none_when_expected_is_empty() {
        let dir = scratch_dir("empty-expected");
        assert_eq!(missing_ithmb(&dir, &[]), None);
    }

    #[test]
    fn missing_ithmb_detects_absent_file() {
        let dir = scratch_dir("absent");
        assert_eq!(
            missing_ithmb(&dir, &["F1069_1.ithmb"]),
            Some("F1069_1.ithmb")
        );
    }

    #[test]
    fn missing_ithmb_none_when_file_present() {
        let dir = scratch_dir("present");
        std::fs::write(dir.join("F1069_1.ithmb"), b"stub").unwrap();
        assert_eq!(missing_ithmb(&dir, &["F1069_1.ithmb"]), None);
    }

    #[test]
    fn missing_ithmb_reports_first_of_several_missing() {
        let dir = scratch_dir("several");
        std::fs::write(dir.join("F1069_1.ithmb"), b"stub").unwrap();
        assert_eq!(
            missing_ithmb(&dir, &["F1069_1.ithmb", "F1069_2.ithmb"]),
            Some("F1069_2.ithmb")
        );
    }

    // --- resolve_mount ------------------------------------------------------

    #[test]
    fn resolve_mount_explicit_ipod_missing_itunesdb_errors() {
        let dir = scratch_dir("no-itunesdb");
        let mut config = test_config();
        config.ipod = Some(dir.to_string_lossy().into_owned());
        let err = resolve_mount(&config).unwrap_err();
        assert!(
            err.to_string().contains("iPod_Control/iTunes/iTunesDB"),
            "got: {err}"
        );
    }

    #[test]
    fn resolve_mount_explicit_ipod_with_itunesdb_succeeds() {
        let dir = scratch_dir("with-itunesdb");
        std::fs::create_dir_all(dir.join("iPod_Control").join("iTunes")).unwrap();
        std::fs::write(
            dir.join("iPod_Control").join("iTunes").join("iTunesDB"),
            b"stub",
        )
        .unwrap();
        let mut config = test_config();
        config.ipod = Some(dir.to_string_lossy().into_owned());
        let resolved = resolve_mount(&config).unwrap();
        assert!(resolved.starts_with(&dir.to_string_lossy().into_owned()));
    }

    fn test_config() -> Config {
        Config {
            source: PathBuf::from("/nonexistent-source"),
            ipod: None,
            ffmpeg: PathBuf::from("ffmpeg"),
            dry_run: false,
            apply: false,
            no_delete: false,
            verbose: false,
            rebuild_manifest: false,
            use_tui: false,
            manifest_path: PathBuf::from("/nonexistent-manifest.json"),
            save_config: false,
            encoder: crate::cli::EncoderChoice::Ffmpeg,
            refalac_path: PathBuf::from("refalac64"),
            passthrough_wav: false,
            force_reencode: false,
            rockbox_compat: false,
            rockbox_compat_cli_flag: false,
            backfill_rockbox: false,
            scan_library: false,
            restore_db_backup: false,
            replace_library: false,
            verify_artwork: false,
        }
    }

    // --- collect_has_artwork: FFI-touching, fake-mount coverage ------------
    //
    // Follows the fake-mount + hand-rolled-iTunesDB pattern established in
    // `tests/fit_retry_integration.rs` / `tests/auto_restore_integration.rs`:
    // a real libgpod DB via direct FFI calls, no mocking. Scoped to the
    // no-art-consistent case per the task brief — attaching a real cover-art
    // thumbnail (`itdb_track_set_thumbnails_from_data`) needs gdk-pixbuf
    // loader plugins wired up (see `add_track_with_file`'s doc comment) and
    // pulls in the same asset-availability fragility that comment already
    // flags; asserting `has_artwork` reads back `false` for a track added
    // WITHOUT thumbnails already exercises the real FFI struct-field read
    // this function exists for. The full `verify_artwork` pipeline glues
    // this together with mounted `ManifestStore` authority and device
    // identity resolution, which still requires real hardware, so this test
    // stops at the FFI-touching unit rather than driving `verify_artwork`
    // end-to-end.
    // `resolve_mount`, `missing_ithmb`, `classify`, and `build_report` above
    // cover the rest of the pipeline's logic without needing real hardware
    // or the real config dir.

    const BARE_M4A: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a");

    fn fake_mount(label: &str) -> PathBuf {
        let base = scratch_dir(label);
        std::fs::create_dir_all(base.join("iPod_Control").join("iTunes")).unwrap();
        std::fs::create_dir_all(base.join("iPod_Control").join("Music").join("F00")).unwrap();
        base
    }

    /// Write a real, valid (empty) iTunesDB — same approach as
    /// `fit_retry_integration.rs::write_valid_itunesdb`.
    fn write_valid_itunesdb(mount: &Path) {
        use std::ffi::CString;
        use std::ptr;
        unsafe {
            let db = ffi::itdb_new();
            assert!(!db.is_null(), "itdb_new returned null");
            let mount_c = CString::new(mount.to_str().unwrap()).unwrap();
            ffi::itdb_set_mountpoint(db, mount_c.as_ptr());
            let title = CString::new("iPod").unwrap();
            let mpl = ffi::itdb_playlist_new(title.as_ptr(), 0);
            assert!(!mpl.is_null(), "itdb_playlist_new returned null");
            ffi::itdb_playlist_set_mpl(mpl);
            ffi::itdb_playlist_add(db, mpl, -1);
            let mut err: *mut ffi::GError = ptr::null_mut();
            let ok = ffi::itdb_write(db, &mut err);
            ffi::itdb_free(db);
            assert_ne!(ok, 0, "itdb_write failed generating test fixture");
        }
    }

    #[test]
    fn collect_has_artwork_reads_false_for_track_added_without_thumbnails() {
        let mount = fake_mount("collect-has-artwork");
        write_valid_itunesdb(&mount);

        let db = OwnedDb::open(&mount).unwrap();
        let handle = db
            .add_track_with_file(Path::new(BARE_M4A), &crate::ipod::db::Tags::default(), None)
            .unwrap();
        db.write().unwrap();
        drop(db);

        // Reopen from disk — matches what `verify_artwork` does (open,
        // read, never re-write) rather than trusting the in-memory struct
        // straight after the add.
        let reopened = OwnedDb::open(&mount).unwrap();
        let map = collect_has_artwork(&reopened);
        assert_eq!(
            map.get(&handle.dbid),
            Some(&false),
            "a track added with art=None must read back has_artwork=false"
        );
    }
}
