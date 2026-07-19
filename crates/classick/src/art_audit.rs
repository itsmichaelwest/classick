//! `--verify-artwork` audit mode: diagnostic + permanent regression harness
//! for the cover-art pipeline bugs documented in LEARNINGS.md ("macOS
//! Artwork Root Cause", the `itdb_write`-deletes-ithmb finding). For every
//! manifest entry with a known source, validates the matching DB track's
//! artwork chain and reports only inconsistent combinations:
//!
//!  1. does the source file have embedded cover art (same lofty-backed probe
//!     `apply_loop` uses via [`crate::transcode::probe`] /
//!     [`crate::transcode::has_embedded_art`]);
//!  2. does the matching DB track (by `ipod_dbid`) have every required
//!     artwork link, record, and thumbnail pointer;
//!  3. can libgpod decode that track's thumbnail into a pixbuf.
//!
//! A track whose source has no art does not require artwork and is consistent
//! (OK), but its DBID must still resolve to a real track.
//!
//! Read-only: opens the device's iTunesDB via [`OwnedDb::open`] but never
//! calls `.write()` — per LEARNINGS.md, `itdb_write` on a rewrite path can
//! delete ithmb thumbnails, which would make this diagnostic tool corrupt
//! the very state it's trying to inspect.

use crate::config::Config;
use crate::device_state;
use crate::ipod::db::{OwnedDb, TrackArtworkSignals};
use crate::ipod::device;
use crate::preflight;
use crate::progress::Progress;
use crate::transcode;
use anyhow::{anyhow, Context, Result};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkFailure {
    MissingTrack,
    HasArtworkUnset,
    MissingMhiiLink,
    MissingArtworkRecord,
    MissingThumbnail,
    DecodeFailed,
}

impl std::fmt::Display for ArtworkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::MissingTrack => "db track missing",
            Self::HasArtworkUnset => "db track has_artwork=0",
            Self::MissingMhiiLink => "db track mhii_link=0",
            Self::MissingArtworkRecord => "db track artwork record missing",
            Self::MissingThumbnail => "db track thumbnail missing",
            Self::DecodeFailed => "db track thumbnail decode failed",
        };
        f.write_str(reason)
    }
}

/// Classify one source/DB pair into `None` (consistent) or a specific broken
/// link in the expected thumbnail chain. DBID resolution is required for every
/// manifest entry; the remaining artwork checks apply only when the source has
/// embedded artwork.
pub fn classify(
    source_has_art: bool,
    signals: Option<TrackArtworkSignals>,
) -> Option<ArtworkFailure> {
    let Some(signals) = signals else {
        return Some(ArtworkFailure::MissingTrack);
    };
    if !source_has_art {
        return None;
    }
    if !signals.has_artwork {
        Some(ArtworkFailure::HasArtworkUnset)
    } else if signals.mhii_link == 0 {
        Some(ArtworkFailure::MissingMhiiLink)
    } else if !signals.has_artwork_record {
        Some(ArtworkFailure::MissingArtworkRecord)
    } else if !signals.has_thumbnail || !signals.has_thumbnails {
        Some(ArtworkFailure::MissingThumbnail)
    } else if !signals.decoded_thumbnail {
        Some(ArtworkFailure::DecodeFailed)
    } else {
        None
    }
}

/// Pure aggregation: fold a sequence of already-computed per-track signals
/// into an `ArtAuditReport` via [`classify`]. Kept separate from
/// `verify_artwork`'s I/O (mount resolution, FFI, source probes) so the
/// aggregation logic — the part most worth regression-testing — needs no
/// hardware or fake-mount harness to exercise.
pub fn build_report<'a, I>(entries: I) -> ArtAuditReport
where
    I: IntoIterator<Item = (&'a Path, bool, Option<TrackArtworkSignals>)>,
{
    let mut report = ArtAuditReport::default();
    for (source_path, source_has_art, signals) in entries {
        report.checked += 1;
        match classify(source_has_art, signals) {
            Some(reason) => report.failures.push(ArtAuditFailure {
                source_path: source_path.to_path_buf(),
                reason: reason.to_string(),
            }),
            None => report.ok += 1,
        }
    }
    report
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
    let mut report = ArtAuditReport::default();
    let mut resolved: Vec<(PathBuf, bool, Option<TrackArtworkSignals>)> = Vec::new();

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
        resolved.push((
            entry.source_path.clone(),
            source_has_art,
            db.track_artwork_signals(entry.ipod_dbid),
        ));
    }

    let sub = build_report(
        resolved
            .iter()
            .map(|(path, source_has_art, signals)| (path.as_path(), *source_has_art, *signals)),
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
    use crate::ffi;
    use crate::ipod::db::TrackArtworkSignals;

    fn complete_artwork() -> TrackArtworkSignals {
        TrackArtworkSignals {
            has_artwork: true,
            mhii_link: 42,
            has_artwork_record: true,
            has_thumbnail: true,
            has_thumbnails: true,
            decoded_thumbnail: true,
        }
    }

    // --- classify: per-track validation chain ----------------------------

    #[test]
    fn classify_missing_track_even_when_source_has_no_art() {
        assert_eq!(classify(false, None), Some(ArtworkFailure::MissingTrack));
    }

    #[test]
    fn classify_track_without_source_art_is_legitimately_ok() {
        let no_art = TrackArtworkSignals::default();
        assert_eq!(classify(false, Some(no_art)), None);
    }

    #[test]
    fn classify_expected_artwork_requires_has_artwork_flag() {
        let mut signals = complete_artwork();
        signals.has_artwork = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::HasArtworkUnset)
        );
    }

    #[test]
    fn classify_global_ithmb_does_not_hide_zero_per_track_link() {
        let dir = scratch_dir("global-ithmb-zero-link");
        std::fs::write(dir.join("F1069_1.ithmb"), b"global thumbnails").unwrap();
        assert!(dir.join("F1069_1.ithmb").exists());

        let mut signals = complete_artwork();
        signals.mhii_link = 0;
        signals.has_artwork_record = false;
        signals.has_thumbnail = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::MissingMhiiLink)
        );
    }

    #[test]
    fn classify_expected_artwork_requires_artwork_record() {
        let mut signals = complete_artwork();
        signals.has_artwork_record = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::MissingArtworkRecord)
        );
    }

    #[test]
    fn classify_expected_artwork_requires_thumbnail_pointer() {
        let mut signals = complete_artwork();
        signals.has_thumbnail = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::MissingThumbnail)
        );
    }

    #[test]
    fn classify_expected_artwork_requires_libgpod_thumbnail_state() {
        let mut signals = complete_artwork();
        signals.has_thumbnails = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::MissingThumbnail)
        );
    }

    #[test]
    fn classify_expected_artwork_requires_decodable_thumbnail() {
        let mut signals = complete_artwork();
        signals.decoded_thumbnail = false;
        assert_eq!(
            classify(true, Some(signals)),
            Some(ArtworkFailure::DecodeFailed)
        );
    }

    #[test]
    fn classify_complete_per_track_artwork_is_ok() {
        assert_eq!(classify(true, Some(complete_artwork())), None);
    }

    // --- build_report: aggregation ---------------------------------------

    #[test]
    fn build_report_counts_ok_and_failures() {
        let p1 = PathBuf::from("/music/a.flac");
        let p2 = PathBuf::from("/music/b.flac");
        let p3 = PathBuf::from("/music/c.flac");
        let mut no_link = complete_artwork();
        no_link.mhii_link = 0;
        let entries = vec![
            (p1.as_path(), true, Some(complete_artwork())),
            (p2.as_path(), true, None),
            (p3.as_path(), true, Some(no_link)),
        ];
        let report = build_report(entries);
        assert_eq!(report.checked, 3);
        assert_eq!(report.ok, 1);
        assert_eq!(report.failures.len(), 2);
        assert_eq!(report.failures[0].source_path, p2);
        assert_eq!(report.failures[0].reason, "db track missing");
        assert_eq!(report.failures[1].source_path, p3);
        assert_eq!(report.failures[1].reason, "db track mhii_link=0");
    }

    #[test]
    fn build_report_invariant_checked_equals_ok_plus_failures() {
        let p = PathBuf::from("/x.flac");
        let cases = [
            (false, Some(TrackArtworkSignals::default())),
            (true, None),
            (true, Some(TrackArtworkSignals::default())),
            (true, Some(complete_artwork())),
        ];
        let entries: Vec<(&Path, bool, Option<TrackArtworkSignals>)> = cases
            .iter()
            .map(|(source_has_art, signals)| (p.as_path(), *source_has_art, *signals))
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

    fn scratch_dir(label: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("classick-art-audit-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
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

    // --- track_artwork_signals: FFI-touching, fake-mount coverage ----------
    //
    // Follows the fake-mount + hand-rolled-iTunesDB pattern established in
    // `tests/fit_retry_integration.rs` / `tests/auto_restore_integration.rs`:
    // a real libgpod DB via direct FFI calls, no mocking. Scoped to the
    // no-art-consistent case per the task brief. The pure classifier covers
    // every broken-link state; this test proves the wrapper safely reads and
    // decodes a real libgpod track without requiring hardware or art fixtures.

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
    fn track_artwork_signals_read_no_thumbnail_for_track_added_without_art() {
        let mount = fake_mount("track-artwork-signals");
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
        let signals = reopened.track_artwork_signals(handle.dbid).unwrap();
        assert!(!signals.has_artwork);
        assert_eq!(signals.mhii_link, 0);
        assert!(!signals.has_thumbnail);
        assert!(!signals.has_thumbnails);
        assert!(!signals.decoded_thumbnail);
    }
}
