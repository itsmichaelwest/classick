//! Per-Action match arms (Add / Modify / MetadataOnly / Remove / Unchanged)
//! plus their supporting helpers — `transcode_one`, `commit_transcoded`,
//! staged album publication, `entry_from`, `build_rebuild_manifest`, and
//! `count_actions`.
//! Also owns the top-level `run` function that ties preflight, diff, review,
//! and apply together.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::checkpoint::CheckpointClock;
use crate::cli::EncoderChoice;
use crate::config::Config;
use crate::config_file;
use crate::device_state;
use crate::fit::DeferredAlbum;
use crate::free_space;
use crate::ipc::SkippedForSpace;
use crate::ipod::db::{open_with_auto_restore, OwnedDb, TrackHandle};
use crate::ipod::device;
use crate::library_index;
use crate::manifest::{self, Action, Manifest, ManifestEntry};
use crate::manifest_store::{LoadedManifest, ManifestStore};
use crate::pipeline::OrderedTranscoder;
use crate::preflight;
use crate::progress::{ActionPlanSummary, Decision, Progress, ReviewDecision};
use crate::source::{self, SourceEntry};
#[cfg(test)]
use crate::source_location::SourceIdentity;
use crate::source_location::SourceLocation;
use crate::tags::tags_from_probe;
use crate::transcode::{self, has_embedded_art, ProbeOutput, SourceAction};
use crate::try_with_prompt::{await_prompt, PromptOutcome};

/// Stable string label for an `EncoderChoice`, used for the manifest's
/// `encoder` field and for `diff`'s encoder-mismatch target string.
pub(crate) fn encoder_str(choice: EncoderChoice) -> &'static str {
    match choice {
        EncoderChoice::Ffmpeg => "ffmpeg",
        EncoderChoice::Refalac => "refalac",
    }
}

/// Best-effort extraction of the source codec name (e.g. "flac", "mp3",
/// "aac", "alac", "vorbis", "opus", "pcm_s16le") from an ffprobe result.
/// Falls back to the first segment of `format.format_name` if no audio
/// stream advertises a codec_name; "unknown" if even that's missing.
pub(crate) fn source_format_from_probe(probe: &ProbeOutput) -> String {
    if let Some(s) = probe.streams.iter().find(|s| s.codec_type == "audio") {
        if let Some(c) = &s.codec_name {
            return c.clone();
        }
    }
    probe
        .format
        .format_name
        .as_deref()
        .unwrap_or("unknown")
        .split(',')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// Probe `<ffmpeg> -version` and return the first line (e.g. "ffmpeg version
/// n7.0 ..."). Forensic-only: recorded in `ManifestEntry.encoder_version`
/// so a future audit can correlate an iPod-side ALAC file with the exact
/// encoder that wrote it. Returns Err if ffmpeg isn't spawnable. `ffmpeg_path`
/// is the configured ffmpeg binary (F-21 / F-16).
pub(crate) fn ffmpeg_version(ffmpeg_path: &std::path::Path) -> Result<String> {
    use crate::windows_proc::NoConsoleWindow;
    let out = std::process::Command::new(ffmpeg_path)
        .args(["-hide_banner", "-version"])
        .no_console()
        .output()
        .with_context(|| format!("invoking {} -version", ffmpeg_path.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().next().unwrap_or("ffmpeg").trim().to_string())
}

/// Effective Rockbox-compat for one run: the raw `--rockbox-compat` CLI flag
/// force-enables regardless of the device's persisted setting (matches the
/// pre-trust-package "CLI flag OR global setting" semantics — see
/// `config::resolve_with`'s `rockbox_compat` merge); otherwise the syncing
/// device's own setting wins. Pure so it's testable without touching the
/// device-settings file.
pub(crate) fn effective_rockbox(
    cli_flag: bool,
    device: &crate::device_config::DeviceSettings,
) -> bool {
    cli_flag || device.rockbox_compat
}

/// Outcome of a run after any required coordinated publication has finished.
/// Hard failures remain in the outer `anyhow::Result` and therefore cannot be
/// mistaken for a clean cancelled or paused outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
    Paused,
}

fn review_outcome(decision: ReviewDecision) -> Option<RunOutcome> {
    match decision {
        ReviewDecision::Apply { .. } => None,
        ReviewDecision::DryRun => Some(RunOutcome::Completed),
        ReviewDecision::Quit => Some(RunOutcome::Cancelled),
    }
}

fn outcome_for_stop(reason: Option<crate::progress::StopReason>) -> RunOutcome {
    match reason {
        Some(crate::progress::StopReason::Cancelled) => RunOutcome::Cancelled,
        Some(crate::progress::StopReason::Paused) => RunOutcome::Paused,
        None => RunOutcome::Completed,
    }
}

pub fn source_change_requires_confirmation(
    loaded: &LoadedManifest,
    current: &SourceLocation,
) -> bool {
    if loaded.manifest.tracks.is_empty() {
        return false;
    }
    match loaded.source_identity.as_ref() {
        Some(recorded) => recorded != &current.identity,
        None => loaded
            .manifest
            .last_source_root
            .as_deref()
            .is_some_and(|recorded| recorded != current.resolved_path),
    }
}

pub(crate) fn configured_source_location(config: &Config) -> Result<SourceLocation> {
    let config_path = config
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config.toml");
    let persisted = config_file::load(&config_path)?;
    if let Some(mut location) = persisted
        .as_ref()
        .and_then(|persisted| persisted.source_location.clone())
    {
        let persisted_root_matches = persisted
            .as_ref()
            .and_then(|persisted| persisted.source.as_deref())
            == Some(config.source.as_path());
        if location.resolved_path == config.source || persisted_root_matches {
            location.resolved_path = config.source.clone();
            location.verify_resolved_identity()?;
            return Ok(location);
        }
    }
    SourceLocation::discover(config.source.clone())
}

pub(crate) fn manifest_store(config: &Config, mount: &Path, serial: &str) -> Result<ManifestStore> {
    Ok(ManifestStore::new(
        mount.to_path_buf(),
        serial.to_string(),
        device_state::device_manifest_path_in(
            config
                .manifest_path
                .parent()
                .unwrap_or_else(|| Path::new(".")),
            serial,
        )?,
        config.manifest_path.clone(),
        crate::atomic_file::AtomicFileWriter::new(),
    ))
}

fn publish_manifest(
    store: &ManifestStore,
    manifest: &Manifest,
    source: &SourceLocation,
    progress: &Progress,
) -> Result<()> {
    let outcome = store.publish(manifest, source)?;
    if let Some(warning) = outcome.host_cache_warning {
        tracing::warn!("manifest host cache refresh failed after device publish: {warning}");
        progress.log(format!(
            "Warning: iPod manifest was saved, but the host cache could not be refreshed: {warning}"
        ));
    }
    Ok(())
}

fn persisted_config_for_sync(
    config: &Config,
    source_location: &SourceLocation,
) -> config_file::PersistedConfig {
    let mut persisted = config.to_persisted();
    persisted.source_location = Some(source_location.clone());
    persisted
}

pub fn run(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<RunOutcome> {
    if config.dry_run && config.apply {
        return Err(anyhow!("--dry-run and --apply are mutually exclusive"));
    }
    if !config.dry_run && !config.apply && !config.use_tui {
        return Err(anyhow!(
            "interactive review requires a TTY.\n\
             Pass --apply to apply immediately, or --dry-run to preview without changes."
        ));
    }

    // Pre-resolve gates: ffmpeg, iPod mount, source walk. Each runs its own
    // Retry/Abort (or Retry/Change/Abort) prompt loop on failure. The iTunes
    // guard runs first because it's cheap and refusing early avoids any
    // wasted ffprobe/walk work if the user has iTunes open.
    preflight::verify_itunes_not_running(progress, decision_rx)?;
    preflight::verify_ffmpeg(config, progress, decision_rx)?;
    // Refalac is opt-in via --encoder refalac; only probe when the user asked
    // for it (per Phase 3 addendum Change 4). The resolved version string is
    // threaded into apply_loop so Wave 3 Task 6 can record it on each new
    // ManifestEntry; for Wave 2 it's parked behind a TODO.
    let refalac_version: Option<String> = if matches!(config.encoder, EncoderChoice::Refalac) {
        Some(preflight::verify_refalac(config, progress, decision_rx)?)
    } else {
        None
    };
    let mount = preflight::resolve_ipod_mount(config, progress, decision_rx)?;

    // Resolve the device identity now, ahead of both selection filtering and
    // the manifest load: the per-device manifest path (trust-package
    // layout), the foreign-manifest guard below, and the shared-vs-custom
    // selection-path resolution just below all key off the serial, so it
    // must be known before any of that. `identity` is reused as-is below,
    // right before `OwnedDb::open`, instead of re-resolving it.
    let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
    let serial = device_state::sanitize_serial(&identity.firewire_guid);

    // Rockbox-compat is per-device (trust package): `config.rockbox_compat`
    // as merged by `config::resolve` only reflects the CLI flag OR the
    // *global* daemon setting, resolved before the device's serial was even
    // known. Now that it is known, re-resolve against that device's own
    // settings.json (seeded once from the global value — see
    // `device_config::DeviceSettings::load_or_migrate`); the raw CLI flag
    // still force-enables for this one run regardless of what's persisted.
    let global_for_device_settings = config_file::load(&config_file::default_path()?)
        .ok()
        .flatten()
        .unwrap_or_default();
    let device_settings =
        crate::device_config::DeviceSettings::load_or_migrate(&serial, &global_for_device_settings);
    config.rockbox_compat = effective_rockbox(config.rockbox_compat_cli_flag, &device_settings);

    // Task 6: resolve the on-device playlist-mirror paths once, up front —
    // reused both for the session-start adopt right below and for the
    // mirror write after this run's `db.write()` (see the "Final commit"
    // section further down). Resolution failure (no resolvable config dir)
    // leaves both as `None`; every downstream use degrades to a no-op
    // rather than failing the sync — playlist mirroring is a convenience,
    // not core sync machinery.
    let playlist_store_root = crate::playlist::PlaylistStore::default_root().ok();
    let device_subscriptions_file = device_state::device_subscriptions_path(&serial).ok();
    if let (Some(root), Some(subs)) = (&playlist_store_root, &device_subscriptions_file) {
        // Runs before anything else this session reads the host playlist
        // store or this device's subscriptions.json (the sync_set/store
        // load below, and `subscriptions` a few lines down) — so an
        // adoption here is visible to the rest of THIS run, not just the
        // next one. See `device_playlists::adopt_from_ipod`'s doc comment
        // for the "host store empty AND device mirror non-empty" gate.
        crate::ipod::device_playlists::adopt_from_ipod(Path::new(&mount), root, subs);
    }

    let sources = preflight::walk_source(config, progress, decision_rx)?;
    guard_nonempty_walk(&sources, &config.source)?;
    // Task 5 (sync_set): device content = scope selection ∪ subscribed
    // playlists. `library_idx` is loaded once here (moved up from the fit
    // pass below, which reuses this same value by reference) since sync_set
    // needs it too. `sources` (the walk) is the existence oracle for
    // playlist tracks — see `sync_set::compute`'s doc comment.
    let library_idx: Option<library_index::LibraryIndex> = library_index::default_index_path()
        .ok()
        .map(|p| library_index::load_or_empty(&p, &config.source));
    let index_for_sync_set = library_idx
        .clone()
        .unwrap_or_else(|| library_index::LibraryIndex::empty(config.source.clone()));
    // Fix 1 (per-device selection ignored by sync): always resolve this
    // device's own `devices/<serial>/selection.json` — never the deprecated
    // `custom_selection`-gated shared/per-device split. This must match
    // every daemon read/write site (daemon/runtime.rs, daemon/library.rs),
    // which already use `effective_device_selection_path`.
    let selection = crate::selection::effective_device_selection_path(&serial)
        .map(|p| crate::selection::load_or_all(&p))
        .unwrap_or_else(|_| crate::selection::Selection::all());
    let subscriptions = device_subscriptions_file
        .as_ref()
        .map(|p| crate::device_config::Subscriptions::load_or_default(p))
        .unwrap_or_default();
    let mut effective_set: Option<crate::sync_set::EffectiveSet> = None;
    // Task 6 / Fix 2: desired on-device playlists as `(slug, display name,
    // resolved source paths)`. `slug` is the managed-identity join key
    // `reconcile_playlists_step`/`device_playlists::reconcile` now key on
    // (never `name` alone — `PlaylistStore::unique_slug` allows distinct
    // slugs like `gym`/`gym-2` to share a display name, and keying by name
    // would collapse them into one managed entry). `name` comes straight
    // from `EffectiveSet::playlist_tracks`, which already loaded each
    // playlist to resolve its members — no second `store.load` round-trip
    // needed here. `None` (rather than `Some(vec![])`) when the playlist
    // store itself couldn't be opened this run — that's "unknown", not
    // "user wants zero playlists", so the reconcile call near the final
    // commit skips entirely rather than deleting every previously-managed
    // playlist over a transient store-open failure.
    let mut desired_playlists: Option<Vec<(String, String, Vec<PathBuf>)>> = None;
    let sources = match crate::playlist::PlaylistStore::default_root()
        .and_then(crate::playlist::PlaylistStore::open)
    {
        Ok(store) => {
            let effective = crate::sync_set::compute(
                sources,
                &selection,
                &subscriptions,
                &store,
                &index_for_sync_set,
                &config.source,
            );
            for (slug, err) in &effective.playlist_errors {
                progress.log(format!("playlist '{slug}': {err}"));
            }
            if effective.missing_playlist_tracks > 0 {
                progress.log(format!(
                    "playlists: {} track(s) referenced but not found in the source walk",
                    effective.missing_playlist_tracks
                ));
            }
            progress.log(format!(
                "sync set: {} source track(s) ({} playlist(s) subscribed)",
                effective.sources.len(),
                subscriptions.playlists.len(),
            ));
            desired_playlists = Some(effective.playlist_tracks.clone());
            let sources = effective.sources.clone();
            effective_set = Some(effective);
            sources
        }
        Err(e) => {
            tracing::warn!(
                "playlists: cannot open playlist store ({e:#}); syncing scope selection only"
            );
            crate::selection::apply_to_sources(sources, &config.source, &serial, |msg| {
                progress.log(msg)
            })
        }
    };
    let _ = &effective_set; // sources + missing/error counts already consumed above
    let source_location = configured_source_location(config)?;
    let manifest_store = manifest_store(config, Path::new(&mount), &serial)?;
    let manifest_path = device_state::portable_manifest_path(Path::new(&mount));

    // 3. Load (or rebuild) manifest.
    let mut loaded = if config.rebuild_manifest {
        let db = open_with_auto_restore(Path::new(&mount), || {
            progress.log("Restored iPod database from backup after detecting corruption");
            progress.note_db_restored();
        })?;
        manifest_store.reconcile_from_live_db(&source_location, || {
            Ok(build_rebuild_manifest(&db, &serial))
        })?
    } else {
        manifest_store.load(&source_location)?
    };
    let recovery_cache = crate::artwork_cache::ArtworkCache::new(
        config
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("artwork-cache"),
    );
    let recovery = crate::sync_transaction::CheckpointCoordinator {
        mount: Path::new(&mount),
        serial: &identity.firewire_guid,
        manifest_store: &manifest_store,
        artwork_cache: recovery_cache,
    }
    .recover_pending_with_options(
        &mut loaded.manifest,
        progress,
        crate::sync_transaction::PublishOptions {
            desired_playlists: desired_playlists.as_deref(),
            playlist_state_root: None,
            device_identity: Some(&identity),
            playlist_failure_point: None,
        },
    )?;
    if !recovery.is_empty() {
        loaded = manifest_store.load(&source_location)?;
    }
    let needs_device_publish = loaded.needs_device_publish;
    let source_changed = source_change_requires_confirmation(&loaded, &source_location);
    let mut manifest = loaded.manifest;

    // 4. Diff. Pass `source::fingerprint` as the slow-path hash callback;
    //    it only fires for entries whose (mtime, size) doesn't match the
    //    manifest, so the steady-state "nothing changed" run is stat-only.
    //    The target_encoder + force_reencode pair drives the encoder-mismatch
    //    branch (manifest::is_encoder_mismatch) so an existing entry whose
    //    body was written by a different encoder gets promoted to Modify.
    let target_encoder = encoder_str(config.encoder);
    let actions = manifest::diff(
        &manifest,
        &sources,
        source::fingerprint,
        source::audio_fingerprint,
        target_encoder,
        config.force_reencode,
    )?;

    // Fit pass (Task 8): defer whole albums that won't fit the device's
    // remaining space rather than let a mid-sync "disk full" error abort the
    // run partway through an album. Budget = free bytes + this run's Remove
    // actions' reclaimed space - a safety reserve (fit::reserve_bytes); see
    // `compute_budget`. `library_idx` is a stat-cache read (never a rescan)
    // loaded once above (sync_set needs it too) and reused, by reference,
    // for the end-of-run retry below — `album_tag_of` is `Copy` (captures
    // only `&library_idx`), so passing it into `fit::plan_fit` doesn't
    // consume it.
    let album_tag_of = |path: &Path| -> Option<String> {
        library_idx
            .as_ref()?
            .files
            .get(path)
            .map(|t| t.album.clone())
    };
    let storage = free_space::query(Path::new(&mount));
    if storage.is_none() {
        progress.log(
            "Could not determine free space on the iPod; syncing without a size budget."
                .to_string(),
        );
    }
    // Published files remain live until the staged candidate is verified, so
    // removals provide no up-front credit to this transaction.
    let budget = compute_budget(storage, 0);
    let fit_outcome = crate::fit::plan_fit(actions, budget, album_tag_of);
    let actions = fit_outcome.kept;
    let deferred = fit_outcome.deferred;
    if !deferred.is_empty() {
        progress.log(format!(
            "Deferred {} album(s) that don't fit this run's staged-space budget.",
            deferred.len()
        ));
    }

    let (add, modify, metadata_only, remove, unchanged) = count_actions(&actions);

    // Pre-release migration only: old builds created this marker after a raw
    // mid-loop DB write. Its presence forces one coordinated publication;
    // current code never creates a new marker.
    let marker_path = match device_state::artwork_dirty_marker_path(&serial) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("artwork-dirty marker: path resolution failed: {e:#}");
            None
        }
    };
    let marker_present_before_apply = marker_path.as_ref().is_some_and(|p| p.exists());

    // Progress was started in main() and is borrowed here. Send the header now
    // that we know what to display.
    progress.header(
        config.source.display().to_string(),
        mount.clone(),
        manifest_path.display().to_string(),
    );

    if config.rebuild_manifest {
        progress.log(format!(
            "Rebuilt manifest from iPod: {} track(s) recorded as source-unknown",
            manifest.tracks.len()
        ));
    } else {
        progress.log(format!(
            "Loaded {} existing manifest entries",
            manifest.tracks.len()
        ));
    }
    progress.log(format!("Source: {} FLAC file(s)", sources.len()));

    progress.log(format!(
        "action plan: add={add} modify={modify} metadata={metadata_only} remove={remove} unchanged={unchanged}"
    ));

    let summary_struct = ActionPlanSummary {
        add,
        modify,
        metadata_only,
        remove,
        unchanged,
    };

    // A portable logical identity wins over mount spelling. Legacy manifests
    // without one retain the original root comparison as their safeguard.
    let mut safeguard_force_no_delete = false;
    if source_changed {
        let previous = manifest
            .last_source_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "a different logical source".to_string());
        let msg = format!(
            "Source root has changed since the last sync.\n\n\
                     Previous: {}\n\
                     Current : {}\n\n\
                     The current diff would REMOVE {} track(s) (everything in the manifest \
                     that's not in the new source).\n\n\
                     If this was intentional, choose Continue. If you typo'd --source or are \
                     pointing at a different library, choose Abort. If you want to add new \
                     tracks from the new source without touching the iPod's existing tracks, \
                     choose --no-delete mode.",
            previous,
            config.source.display(),
            remove,
        );
        let outcome = await_prompt(
            progress,
            decision_rx,
            msg,
            &[
                "Continue (apply Remove + Add normally)",
                "Use --no-delete for this run",
                "Abort",
            ],
            &[
                PromptOutcome::Retry,
                PromptOutcome::Skip,
                PromptOutcome::Abort,
            ],
        )?;
        match outcome {
            PromptOutcome::Retry => {
                progress.log("Source-change safeguard: user chose Continue.".to_string());
            }
            PromptOutcome::Skip => {
                progress.log(
                    "Source-change safeguard: applying with --no-delete for this run.".to_string(),
                );
                safeguard_force_no_delete = true;
            }
            _ => {
                return Err(anyhow!("source-change safeguard aborted"));
            }
        }
    }

    // Decide effective no_delete based on dry-run / apply / interactive review.
    // The safeguard_force_no_delete bit is OR'd into every branch so the
    // summary shown to the user matches what the apply loop will actually do.
    let effective_no_delete: bool = if config.dry_run {
        let no_delete = config.no_delete || safeguard_force_no_delete;
        let effective_remove = if no_delete { 0 } else { remove };
        let total_planned = add + modify + metadata_only + effective_remove;
        progress.summary(add, modify, metadata_only, remove, unchanged, total_planned);
        log_deferred_summary(progress, &deferred);
        progress.log("Dry run; nothing was written.");
        return Ok(RunOutcome::Completed);
    } else if config.apply || !config.use_tui {
        // Non-interactive: just apply with the configured no_delete.
        let no_delete = config.no_delete || safeguard_force_no_delete;
        let effective_remove = if no_delete { 0 } else { remove };
        let total_planned = add + modify + metadata_only + effective_remove;
        progress.summary(add, modify, metadata_only, remove, unchanged, total_planned);
        no_delete
    } else {
        // Interactive review.
        progress.review(
            summary_struct,
            config.no_delete || safeguard_force_no_delete,
        );
        match decision_rx.recv() {
            Ok(Decision::Review(ReviewDecision::Apply { no_delete })) => {
                let no_delete = no_delete || safeguard_force_no_delete;
                let effective_remove = if no_delete { 0 } else { remove };
                let total_planned = add + modify + metadata_only + effective_remove;
                progress.summary(add, modify, metadata_only, remove, unchanged, total_planned);
                no_delete
            }
            Ok(Decision::Review(decision @ ReviewDecision::DryRun)) => {
                log_deferred_summary(progress, &deferred);
                progress.log("Dry run; nothing was written.");
                return Ok(review_outcome(decision).expect("dry run is terminal"));
            }
            Ok(Decision::Review(decision @ ReviewDecision::Quit)) => {
                progress.log("Aborted; nothing was written.");
                progress.finalizing(crate::progress::StopReason::Cancelled, 0, 0);
                return Ok(review_outcome(decision).expect("quit is terminal"));
            }
            Ok(Decision::Prompt { .. }) | Ok(Decision::Form { .. }) | Ok(Decision::Pause) => {
                // Unexpected at this stage (no try_with_prompt / wizard caller
                // wired yet, and Pause only makes sense once the apply loop
                // is running). Return loudly rather than silently swallowing
                // a stray decision.
                return Err(anyhow!(
                    "unexpected prompt/form/pause decision before any prompt was sent"
                ));
            }
            Err(_) => {
                return Err(anyhow!("review channel disconnected unexpectedly"));
            }
        }
    };

    // safeguard_force_no_delete is already folded into every branch of the
    // `effective_no_delete` decision above, so no extra OR needed here.
    let effective_remove = if effective_no_delete { 0 } else { remove };
    let total_planned = add + modify + metadata_only + effective_remove;

    if add == 0
        && modify == 0
        && metadata_only == 0
        && (remove == 0 || effective_no_delete)
        && !marker_present_before_apply
        && !needs_device_publish
    {
        // Deferred albums still count as "nothing else to do" here — free
        // space hasn't changed since the fit pass queried it moments ago, so
        // a retry now would just fail again; not worth opening the DB for.
        // Still surfaced on Finish so the caller knows *why* their new album
        // didn't show up.
        if !deferred.is_empty() {
            let skipped = skipped_for_space(&deferred);
            progress.log(format!("Nothing else to do; {}", skipped.describe()));
            progress.note_skipped_for_space(skipped);
        } else {
            progress.log("Nothing to do.");
        }
        return Ok(RunOutcome::Completed);
    }
    if marker_present_before_apply
        && add == 0
        && modify == 0
        && metadata_only == 0
        && (remove == 0 || effective_no_delete)
    {
        // A legacy marker forces one empty coordinated publication so every
        // retained artwork link is prepared and verified before migration.
        progress.log(
            "Nothing to sync, but a previous interrupted sync left artwork unrepaired — \
             repairing before finishing…",
        );
    }

    // 5. Apply actions + 6. Commit DB + manifest.
    // Wrapped in a closure so that any mid-sync error can be paired with a
    // recovery hint before bubbling up, and progress.finish() is always called.
    //
    // NOTE: save_config is intentionally NOT inside this closure (it used to
    // be). A save-config failure happens AFTER db.write() + manifest::save have
    // already succeeded, so there are no orphans to recover from — bubbling it
    // out of this closure would trigger the misleading "orphan files /
    // --rebuild-manifest" recovery block. Hoisted below.
    let sync_result = run_staged_sync(
        config,
        progress,
        decision_rx,
        Path::new(&mount),
        &identity.firewire_guid,
        &identity,
        &manifest_store,
        &mut manifest,
        actions,
        desired_playlists.as_deref(),
        marker_path.as_deref(),
        effective_no_delete,
        total_planned,
        &refalac_version,
        &deferred,
        &library_idx,
    );

    if let Err(error) = &sync_result {
        progress.error(format!("Sync failed: {error:#}"));
        for line in RECOVERY_HINT_LINES {
            progress.error((*line).to_string());
        }
    }
    if sync_result.is_ok() {
        if let (Some(root), Some(subscriptions)) =
            (&playlist_store_root, &device_subscriptions_file)
        {
            crate::ipod::device_playlists::mirror_to_ipod(Path::new(&mount), root, subscriptions);
        }
    }
    // save_config is independent of the sync closure: it only runs on success
    // and a failure here is a warning (config-file write), not a reason to
    // print the orphan-files recovery block.
    if sync_result.is_ok() && config.save_config {
        match config_file::default_path().and_then(|p| {
            config_file::save(&p, &persisted_config_for_sync(config, &source_location)).map(|()| p)
        }) {
            Ok(config_path) => {
                progress.log(format!("config saved to {}", config_path.display()));
            }
            Err(e) => {
                progress.error(format!(
                    "warning: sync succeeded but failed to save config: {e}"
                ));
            }
        }
    }

    // progress.finish() is called by main() after this fn returns. Attach the
    // recovery block to the error so that, once the alternate screen is torn
    // down and log_tail is gone, the user still sees how to recover on stderr.
    sync_result.map_err(|e| {
        let hint = RECOVERY_HINT_LINES.join("\n  ");
        anyhow!("sync failed: {e:#}\n\nRecovery:\n  {hint}")
    })
}

struct AlbumBatch {
    key: String,
    actions: Vec<Action>,
}

#[allow(clippy::too_many_arguments)]
fn run_staged_sync(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    mount: &Path,
    serial: &str,
    identity: &crate::ipod::device::LibgpodIdentity,
    manifest_store: &ManifestStore,
    manifest: &mut Manifest,
    actions: Vec<Action>,
    desired_playlists: Option<&[(String, String, Vec<PathBuf>)]>,
    legacy_dirty_marker: Option<&Path>,
    no_delete: bool,
    total_planned: usize,
    refalac_version: &Option<String>,
    deferred: &[DeferredAlbum],
    library_index: &Option<library_index::LibraryIndex>,
) -> Result<RunOutcome> {
    if let Err(error) = crate::ipod::sysinfo_provision::provision(mount, identity) {
        progress.log(format!(
            "SysInfoExtended provisioning failed (art may not display): {error:#}"
        ));
    }
    if let Err(error) = crate::ipod::db::backup_itunesdb(mount) {
        progress.log(format!(
            "Pre-sync DB backup failed: {error}; sync will proceed without a fresh backup."
        ));
    }

    let artwork_cache = crate::artwork_cache::ArtworkCache::new(
        config
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("artwork-cache"),
    );
    if let Some(reason) = poll_stop(decision_rx) {
        progress.finalizing(reason, 0, 0);
        return Ok(outcome_for_stop(Some(reason)));
    }
    if let Some(reason) = prepare_retained_artwork(
        &artwork_cache,
        manifest,
        &actions,
        &config.ffmpeg,
        decision_rx,
    )? {
        progress.finalizing(reason, 0, 0);
        return Ok(outcome_for_stop(Some(reason)));
    }

    let mut batches = group_actions_by_album(actions, library_index).into_iter();
    let mut next_session_id = fresh_session_id();
    let mut journal =
        crate::pending_session::PendingSession::new(next_session_id, serial, Vec::new());
    let mut checkpoint = CheckpointClock::new(
        crate::CHECKPOINT_MAX_TRACKS,
        Duration::from_secs(crate::CHECKPOINT_MAX_SECONDS),
        Instant::now(),
    );
    let mut stop_reason = None;
    let mut completed = 0usize;
    let mut bytes_written = 0u64;
    let mut artwork_counts = ArtworkCounts::default();

    loop {
        let batch = match admit_next_album(&mut batches, decision_rx) {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(reason) => {
                stop_reason = Some(reason);
                break;
            }
        };

        let album_ordinal = journal.albums.len();
        journal
            .albums
            .push(crate::pending_session::PendingAlbum::new(
                batch.key,
                album_ordinal,
            ));
        let jobs = batch
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| match action {
                Action::Add(source) => Some((index, source.clone())),
                Action::Modify(source, _) if !no_delete => Some((index, source.clone())),
                Action::MetadataOnly { source, .. } => Some((index, source.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let worker_config = config.clone();
        let worker_version = refalac_version.clone();
        let transcoder = OrderedTranscoder::start(
            jobs,
            crate::transcode_workers(),
            crate::PIPELINE_WINDOW,
            move |source: &SourceEntry| transcode_one(source, &worker_config, &worker_version),
        );

        for (index, action) in batch.actions.into_iter().enumerate() {
            match action {
                Action::Unchanged(_) => {}
                Action::Remove(_) if no_delete => {}
                Action::Modify(_, _) if no_delete => {}
                Action::Remove(entry) => {
                    completed += 1;
                    progress.track_start(
                        completed,
                        total_planned,
                        format!("REMOVE {}", display_path(&entry.source_path)),
                    );
                    journal
                        .obsolete_files
                        .push(crate::pending_session::ObsoleteFile {
                            path: device_file_path(mount, &entry.ipod_relpath),
                            prior_dbid: entry.ipod_dbid,
                        });
                    checkpoint.record_track();
                    progress.track_done();
                }
                Action::Add(source) => {
                    completed += 1;
                    progress.track_start(
                        completed,
                        total_planned,
                        format!("ADD {}", display_path(&source.path)),
                    );
                    let applied = stage_transcoded_result(
                        &transcoder,
                        index,
                        source,
                        None,
                        mount,
                        &mut journal,
                        album_ordinal,
                        &artwork_cache,
                        &mut bytes_written,
                        &mut artwork_counts,
                        progress,
                    )?;
                    checkpoint.record_track();
                    if applied {
                        progress.track_done();
                    } else {
                        progress.track_skipped();
                    }
                }
                Action::Modify(source, old) | Action::MetadataOnly { source, entry: old } => {
                    completed += 1;
                    progress.track_start(
                        completed,
                        total_planned,
                        format!("REPLACE {}", display_path(&source.path)),
                    );
                    let applied = stage_transcoded_result(
                        &transcoder,
                        index,
                        source,
                        Some(old),
                        mount,
                        &mut journal,
                        album_ordinal,
                        &artwork_cache,
                        &mut bytes_written,
                        &mut artwork_counts,
                        progress,
                    )?;
                    checkpoint.record_track();
                    if applied {
                        progress.track_done();
                    } else {
                        progress.track_skipped();
                    }
                }
            }
        }
        transcoder.finish()?;
        crate::pending_session::PendingSessionStore::new(mount).save(&journal)?;

        if checkpoint.album_boundary(Instant::now()) {
            publish_journal(
                mount,
                serial,
                identity,
                manifest_store,
                &artwork_cache,
                manifest,
                &mut journal,
                None,
                progress,
            )?;
            clear_legacy_marker(legacy_dirty_marker)?;
            next_session_id = next_session_id.wrapping_add(1);
            journal =
                crate::pending_session::PendingSession::new(next_session_id, serial, Vec::new());
        }
    }

    if stop_reason.is_none() {
        stop_reason = poll_stop(decision_rx);
    }
    if let Some(reason) = stop_reason {
        progress.finalizing(reason, journal.albums.len(), journal.staged_files.len());
    }

    let legacy_migration = legacy_dirty_marker.is_some_and(Path::exists);
    if !journal.albums.is_empty() || legacy_migration || desired_playlists.is_some() {
        manifest.last_source_root = Some(config.source.clone());
        publish_journal(
            mount,
            serial,
            identity,
            manifest_store,
            &artwork_cache,
            manifest,
            &mut journal,
            desired_playlists,
            progress,
        )?;
        clear_legacy_marker(legacy_dirty_marker)?;
    }

    if !deferred.is_empty() {
        progress.note_skipped_for_space(skipped_for_space(deferred));
    }
    progress.note_artwork_summary(artwork_counts.to_summary());
    if bytes_written > 0 {
        progress.log(format!(
            "{} of audio staged this run.",
            crate::ipc::format_bytes_human(bytes_written)
        ));
    }
    progress.log(match stop_reason {
        Some(crate::progress::StopReason::Cancelled) => "Cancellation finalized safely.",
        Some(crate::progress::StopReason::Paused) => "Pause finalized safely.",
        None => "Done. Eject the iPod before unplugging.",
    });
    Ok(outcome_for_stop(stop_reason))
}

fn group_actions_by_album(
    actions: Vec<Action>,
    library_index: &Option<library_index::LibraryIndex>,
) -> Vec<AlbumBatch> {
    let mut albums = Vec::<AlbumBatch>::new();
    for action in actions {
        let source_path = match &action {
            Action::Add(source) | Action::Modify(source, _) => &source.path,
            Action::MetadataOnly { source, .. } => &source.path,
            Action::Remove(entry) | Action::Unchanged(entry) => &entry.source_path,
        };
        if matches!(action, Action::Unchanged(_)) {
            continue;
        }
        let album_tag = library_index
            .as_ref()
            .and_then(|index| index.files.get(source_path))
            .map(|track| track.album.as_str());
        let key = crate::fit::album_key(source_path, album_tag);
        if let Some(existing) = albums.iter_mut().find(|album| album.key == key) {
            existing.actions.push(action);
        } else {
            albums.push(AlbumBatch {
                key,
                actions: vec![action],
            });
        }
    }
    albums
}

fn poll_stop(decision_rx: &Receiver<Decision>) -> Option<crate::progress::StopReason> {
    loop {
        match decision_rx.try_recv() {
            Ok(Decision::Review(ReviewDecision::Quit)) => {
                return Some(crate::progress::StopReason::Cancelled)
            }
            Ok(Decision::Pause) => return Some(crate::progress::StopReason::Paused),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

fn admit_next_album<T>(
    albums: &mut impl Iterator<Item = T>,
    decision_rx: &Receiver<Decision>,
) -> std::result::Result<Option<T>, crate::progress::StopReason> {
    match poll_stop(decision_rx) {
        Some(reason) => Err(reason),
        None => Ok(albums.next()),
    }
}

fn fresh_session_id() -> crate::ipc_device::SessionId {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    epoch ^ ((std::process::id() as u64) << 32) ^ NEXT.fetch_add(1, Ordering::Relaxed)
}

fn prepare_retained_artwork(
    cache: &crate::artwork_cache::ArtworkCache,
    manifest: &Manifest,
    actions: &[Action],
    ffmpeg: &Path,
    decision_rx: &Receiver<Decision>,
) -> Result<Option<crate::progress::StopReason>> {
    let obsolete = actions
        .iter()
        .filter_map(|action| match action {
            Action::Remove(entry)
            | Action::Modify(_, entry)
            | Action::MetadataOnly { entry, .. } => Some(entry.ipod_dbid),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    for entry in manifest.tracks.iter().filter(|entry| entry.source_known) {
        if let Some(reason) = poll_stop(decision_rx) {
            return Ok(Some(reason));
        }
        if obsolete.contains(&entry.ipod_dbid) && !entry.source_path.exists() {
            cache.record_no_art(&entry.source_path)?;
            continue;
        }
        let (_, art) = source_tags_and_art(&entry.source_path, ffmpeg)
            .with_context(|| format!("prepare artwork for {}", entry.source_path.display()))?;
        match art {
            Some(bytes) => {
                cache.store_normalized(&entry.source_path, &bytes)?;
            }
            None => cache.record_no_art(&entry.source_path)?,
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn stage_transcoded_result(
    transcoder: &OrderedTranscoder<Transcoded>,
    index: usize,
    source: SourceEntry,
    old: Option<ManifestEntry>,
    mount: &Path,
    journal: &mut crate::pending_session::PendingSession,
    album_index: usize,
    artwork_cache: &crate::artwork_cache::ArtworkCache,
    bytes_written: &mut u64,
    artwork_counts: &mut ArtworkCounts,
    progress: &Progress,
) -> Result<bool> {
    let transcoded = match transcoder.take(index) {
        Ok(transcoded) => transcoded,
        Err(error) => {
            progress.error(format!(
                "Transcode failed for {}: {error:#}",
                source.path.display()
            ));
            return Ok(false);
        }
    };
    let pending_dir = crate::device_state::pending_sessions_dir(mount)
        .join(format!("{}.staged", journal.session_id));
    std::fs::create_dir_all(&pending_dir)?;
    let extension = transcoded
        .temp
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("m4a");
    let pending_path = pending_dir.join(format!("{}.{}", journal.staged_files.len(), extension));
    let partial = pending_path.with_extension(format!("{extension}.partial"));
    let artwork_hash = match transcoded.art.as_deref() {
        Some(bytes) => Some(artwork_cache.store_normalized(&source.path, bytes)?),
        None => {
            artwork_cache.record_no_art(&source.path)?;
            None
        }
    };
    let prior_dbid = old.as_ref().map_or(0, |entry| entry.ipod_dbid);
    let candidate_entry = ManifestEntry {
        source_path: source.path.clone(),
        source_mtime: source.mtime,
        source_size: source.size,
        source_fingerprint: transcoded.fingerprint,
        ipod_dbid: prior_dbid,
        ipod_relpath: String::new(),
        source_known: true,
        audio_fingerprint: transcoded.audio_fingerprint,
        encoder: transcoded.encoder,
        encoder_version: transcoded.encoder_version,
        source_format: transcoded.source_format,
    };
    if let Some(old) = old {
        journal
            .obsolete_files
            .push(crate::pending_session::ObsoleteFile {
                path: device_file_path(mount, &old.ipod_relpath),
                prior_dbid: old.ipod_dbid,
            });
    }
    let staged_index = journal.staged_files.len();
    journal
        .staged_files
        .push(crate::pending_session::StagedFile {
            source: source.path.clone(),
            pending_path: pending_path.clone(),
            final_ipod_path: None,
            dbid: 0,
            tags: transcoded.tags,
            artwork_hash,
            candidate_entry: Some(candidate_entry),
        });
    journal.albums[album_index]
        .staged_file_indices
        .push(staged_index);
    crate::pending_session::PendingSessionStore::new(mount).save(journal)?;

    let copy_result = std::fs::copy(&transcoded.temp, &partial)
        .with_context(|| format!("stage {}", source.path.display()))
        .and_then(|_| {
            std::fs::rename(&partial, &pending_path)
                .with_context(|| format!("publish staged file {}", pending_path.display()))
        });
    if let Err(error) = copy_result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    let size = std::fs::metadata(&pending_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let _ = std::fs::remove_file(&transcoded.temp);
    *bytes_written += size;
    artwork_counts.record(transcoded.art_outcome);
    Ok(true)
}

fn device_file_path(mount: &Path, relative: &str) -> PathBuf {
    mount.join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR))
}

#[allow(clippy::too_many_arguments)]
fn publish_journal(
    mount: &Path,
    serial: &str,
    identity: &crate::ipod::device::LibgpodIdentity,
    manifest_store: &ManifestStore,
    artwork_cache: &crate::artwork_cache::ArtworkCache,
    manifest: &mut Manifest,
    journal: &mut crate::pending_session::PendingSession,
    desired_playlists: Option<&[(String, String, Vec<PathBuf>)]>,
    progress: &Progress,
) -> Result<()> {
    let coordinator = crate::sync_transaction::CheckpointCoordinator {
        mount,
        serial,
        manifest_store,
        artwork_cache: artwork_cache.clone(),
    };
    coordinator.publish_with_options(
        journal,
        manifest,
        progress,
        crate::sync_transaction::PublishOptions {
            desired_playlists,
            playlist_state_root: None,
            device_identity: Some(identity),
            playlist_failure_point: None,
        },
    )?;
    Ok(())
}

fn clear_legacy_marker(marker: Option<&Path>) -> Result<()> {
    let Some(marker) = marker else { return Ok(()) };
    match std::fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("clear migrated artwork marker {}", marker.display())),
    }
}

/// `--replace-library`: erase EVERY track on the iPod, then fall through to
/// a normal `run()` sync of the current selection. Unlike `run`'s diff-driven
/// Remove (which only touches tracks the source no longer has), this wipes
/// the device unconditionally before applying — for the "I want the device
/// to exactly mirror the selection, full stop" case.
///
/// Sequence: resolve mount + identity (mirrors `run`'s early steps) →
/// session backup of the pre-wipe iTunesDB (same helper + ordering as `run`)
/// → open the DB (auto-restore) to learn the live track count → confirm
/// (skipped under `--apply`) → `wipe_all_tracks` → `db.write()` → reset the
/// per-device manifest to `Manifest::empty()` with the serial stamped →
/// fall through to `run(config, progress, decision_rx)` for the sync itself.
///
/// Note on ordering vs. the task brief: the brief's high-level sequence
/// lists confirmation before "open DB", but the confirmation message must
/// name the live track count, which only the opened DB can supply — so this
/// opens the DB first (also needed to wire the FirewireGuid/ModelNumStr for
/// write-signing before the wipe's `db.write()`) and confirms right after.
/// Backup-before-wipe and wipe/write-before-manifest-reset — the orderings
/// that actually matter for data safety — are unchanged.
///
/// `run()` re-resolves the mount/identity/manifest from scratch (its own
/// preflight + `backup_itunesdb` + `open_with_auto_restore` run again) —
/// deliberately simplest per the brief rather than threading state through.
/// The only user-visible side effect of that double resolution: `run`'s own
/// session backup overwrites the pre-wipe backup this function just made
/// with a backup of the now-empty DB. That's consistent with the
/// operation's advertised "cannot be undone" semantics — the pre-wipe
/// backup exists only as a narrow window to recover from an accidental
/// confirm, not as a permanent undo path once the sync proceeds.
/// Preflight for `--replace-library`: walks the source library and applies
/// the same `guard_nonempty_walk` check `run()` uses, so an empty or
/// unreachable source is caught BEFORE `replace_library` wipes anything.
/// `pub(crate)` — this is the seam final-review fix #1 exercises directly,
/// since driving all of `replace_library` needs a real device identity
/// (`device::resolve_libgpod_identity` shells out to IOKit/USB recovery and
/// has no fake-mount path) and so isn't practical to integration-test here.
pub(crate) fn preflight_replace(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<Vec<SourceEntry>> {
    let sources = preflight::walk_source(config, progress, decision_rx)?;
    guard_nonempty_walk(&sources, &config.source)?;
    Ok(sources)
}

pub fn replace_library(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<RunOutcome> {
    preflight::verify_itunes_not_running(progress, decision_rx)?;
    let mount = preflight::resolve_ipod_mount(config, progress, decision_rx)?;
    let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
    let serial = device_state::sanitize_serial(&identity.firewire_guid);
    let source_location = configured_source_location(config)?;
    let manifest_store = manifest_store(config, Path::new(&mount), &serial)?;

    // Validate the source BEFORE wiping anything: an empty/unreachable
    // source must never destroy the device's existing library. Walk it now
    // — before the confirmation prompt below, so the user is never asked to
    // confirm a doomed operation, and before the backup/wipe I/O that
    // follows. The result is intentionally discarded: `run()`, invoked at
    // the end of this function, re-walks the source from scratch. That's a
    // deliberate double-walk — cheap next to the risk of wiping a device on
    // the strength of a walk result that's gone stale by the time `run()`
    // gets to it (source unplugged/changed between here and there).
    let _ = preflight_replace(config, progress, decision_rx)?;

    // Defensive backup of the pre-wipe iTunesDB — same helper/order as
    // `run`'s pre-sync backup (see the doc comment above for why `run`'s own
    // backup, moments later, ends up overwriting this one).
    if let Err(e) = crate::ipod::db::backup_itunesdb(Path::new(&mount)) {
        progress.log(format!(
            "Pre-wipe DB backup failed: {e}; continuing without a fresh backup."
        ));
    }

    let db = open_with_auto_restore(Path::new(&mount), || {
        progress.log("Restored iPod database from backup after detecting corruption");
        progress.note_db_restored();
    })?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &identity.firewire_guid)?;
        device::set_model_num(device_ptr, &identity.model_num_str)?;
    }

    let track_count = db.track_count();
    if !should_skip_replace_confirmation(config.apply) {
        let outcome = await_prompt(
            progress,
            decision_rx,
            replace_library_confirm_message(track_count),
            &["Erase and sync", "Abort"],
            &[PromptOutcome::Custom(0), PromptOutcome::Abort],
        )?;
        if !matches!(outcome, PromptOutcome::Custom(0)) {
            return Err(anyhow!("--replace-library aborted by user"));
        }
    }

    progress.log(format!("Erasing {track_count} track(s) from the iPod…"));
    let wiped = crate::ipod::db::wipe_all_tracks(&db)?;
    progress.log(format!("Erased {wiped} track(s)."));
    write_replace_library_database(&db)?;
    drop(db);

    let mut manifest = Manifest::empty();
    manifest.ipod_serial = Some(serial);
    manifest.last_source_root = Some(config.source.clone());
    publish_manifest(&manifest_store, &manifest, &source_location, progress)
        .context("publish reset manifest after wipe")?;

    progress.log("Library erased; syncing the current selection…".to_string());
    run(config, progress, decision_rx)
}

/// Whether `replace_library`'s confirmation prompt should be skipped —
/// true iff `--apply` was passed. Pure so it's unit-testable without a
/// Progress/decision-channel harness.
pub(crate) fn should_skip_replace_confirmation(apply: bool) -> bool {
    apply
}

pub fn write_replace_library_database(db: &OwnedDb) -> Result<()> {
    crate::sync_transaction::write_coordinated_database(db).context("write iPod DB after wipe")
}

/// Message shown by `replace_library`'s confirmation prompt. Pure so its
/// exact wording is unit-testable.
pub(crate) fn replace_library_confirm_message(track_count: usize) -> String {
    format!(
        "This erases all {track_count} tracks on the iPod, then syncs your selection. \
         This cannot be undone."
    )
}

/// Recovery instructions shown when the apply loop fails mid-sync. Kept as a
/// const so both the in-TUI `progress.error` log and the bubbled error message
/// stay in lock-step.
const RECOVERY_HINT_LINES: &[&str] = &[
    "The iPod may now contain orphan track files (added but not in the iTunesDB),",
    "and the manifest has NOT been updated.",
    "To recover: re-run with --rebuild-manifest, which will read the iPod's",
    "current DB and create a fresh manifest. Then run normally.",
];

// -- Task 8: fit-pass helpers ------------------------------------------

/// Pure budget arithmetic behind the fit pass: free bytes this run's Remove
/// actions will reclaim, minus a safety reserve — saturating at 0 so a
/// reserve larger than free+credit reads as "no room" rather than
/// underflowing. `None` storage (the free-space query failed — unplugged,
/// permissions, an unqueryable filesystem) means "no budget", which
/// `fit::plan_fit` treats as fail-open (keep everything; a genuine
/// out-of-space condition will surface later as a normal iPod-write error).
/// `pub(crate)` so this can be unit-tested without a real mounted device.
pub(crate) fn compute_budget(
    storage: Option<free_space::StorageInfo>,
    remove_credit: u64,
) -> Option<u64> {
    let storage = storage?;
    let reserve = crate::fit::reserve_bytes(storage.total_bytes);
    Some(
        storage
            .free_bytes
            .saturating_add(remove_credit)
            .saturating_sub(reserve),
    )
}

/// Builds the `SkippedForSpace` rollup from a final (post-retry, if any)
/// deferred-album list.
pub(crate) fn skipped_for_space(deferred: &[DeferredAlbum]) -> SkippedForSpace {
    SkippedForSpace {
        albums: deferred.len(),
        tracks: deferred.iter().map(|d| d.tracks).sum(),
        bytes: deferred.iter().map(|d| d.bytes).sum(),
    }
}

/// Shared by the three early-return sites (`--dry-run`, interactive
/// Review's dry-run choice) that need to surface the fit pass's deferral
/// outcome without running the retry (dry-run writes nothing). No-op when
/// nothing was deferred.
fn log_deferred_summary(progress: &Progress, deferred: &[DeferredAlbum]) {
    if !deferred.is_empty() {
        progress.log(skipped_for_space(deferred).describe());
    }
}

/// Reconstructs the `Add` actions belonging to `deferred` albums from the
/// full pre-fit Add list, for the end-of-run retry pass. `fit::plan_fit`
/// only returns kept `Action`s plus a `DeferredAlbum` summary — the dropped
/// `Action`s themselves aren't retained — so the retry re-derives them here
/// using the same `album_key`/`album_tag_of` identity as the first pass,
/// which is what keeps an album's grouping stable across both passes. Pure;
/// unit-tested without any I/O.
pub(crate) fn deferred_add_actions(
    original_adds: &[SourceEntry],
    deferred: &[DeferredAlbum],
    album_tag_of: impl Fn(&Path) -> Option<String>,
) -> Vec<Action> {
    let deferred_keys: std::collections::HashSet<&str> =
        deferred.iter().map(|d| d.key.as_str()).collect();
    original_adds
        .iter()
        .filter(|src| {
            let tag = album_tag_of(&src.path);
            deferred_keys.contains(crate::fit::album_key(&src.path, tag.as_deref()).as_str())
        })
        .cloned()
        .map(Action::Add)
        .collect()
}

/// End-of-run deferred retry (Task 8): after the main apply loop has run,
/// if the fit pass deferred any albums, give them a single second chance
/// against a freshly-computed `budget` — the main loop's Removes may have
/// freed enough room. Single pass, no loop: whatever `fit::plan_fit` still
/// can't fit is returned as the final deferred list. Runs BEFORE the run's
/// final `db.write()`/manifest save (see `run`'s doc comment) so any
/// newly-added tracks land in the same commit as everything else.
///
/// `budget` is computed by the caller (fresh `free_space::query` + zero
/// remove-credit, since Removes already landed on disk during the main loop
/// — unlike the up-front budget, which had to project their reclaimed
/// space) rather than queried in here. That keeps the only bit of I/O in
/// this function's contract at the call site, so `retry_deferred` itself is
/// a pure function of its inputs and can be exercised in tests with an
/// arbitrary `Some(budget)`/`None` without needing a real mounted device.
/// `pub` (not `pub(crate)`) so `tests/fit_retry_integration.rs` can drive it
/// directly against a fake mount + hand-rolled DB.
///
/// Reuses `commit_pipelined` — the exact same per-track commit/retry/skip
/// machinery the main loop uses — via a second, small `OrderedTranscoder`
/// (the main loop's was already `stop()`-ed by the time this runs).
#[allow(clippy::too_many_arguments)]
pub fn retry_deferred(
    config: &Config,
    refalac_version: &Option<String>,
    db: &OwnedDb,
    manifest: &mut Manifest,
    original_adds: &[SourceEntry],
    deferred: Vec<DeferredAlbum>,
    budget: Option<u64>,
    album_tag_of: impl Fn(&Path) -> Option<String>,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    bytes_written: &mut u64,
    artwork_counts: &mut ArtworkCounts,
) -> Result<Vec<DeferredAlbum>> {
    let candidates = deferred_add_actions(original_adds, &deferred, &album_tag_of);
    if candidates.is_empty() {
        return Ok(deferred);
    }

    let retry_outcome = crate::fit::plan_fit(candidates, budget, &album_tag_of);
    if retry_outcome.kept.is_empty() {
        return Ok(retry_outcome.deferred);
    }

    let total = retry_outcome.kept.len();
    progress.log(format!(
        "Retrying {total} previously-deferred track(s) that now fit…"
    ));

    let jobs: Vec<(usize, SourceEntry)> = retry_outcome
        .kept
        .iter()
        .enumerate()
        .filter_map(|(idx, a)| match a {
            Action::Add(src) => Some((idx, src.clone())),
            _ => None, // deferred_add_actions only ever produces Adds
        })
        .collect();
    let config_for_workers = config.clone();
    let refalac_for_workers = refalac_version.clone();
    let transcoder = OrderedTranscoder::start(
        jobs,
        crate::transcode_workers(),
        crate::PIPELINE_WINDOW,
        move |src: &SourceEntry| transcode_one(src, &config_for_workers, &refalac_for_workers),
    );

    for (idx, action) in retry_outcome.kept.into_iter().enumerate() {
        let Action::Add(src) = action else { continue };
        progress.track_start(
            idx + 1,
            total,
            format!("ADD {} (retry)", display_path(&src.path)),
        );
        commit_pipelined(
            &transcoder,
            idx,
            db,
            manifest,
            &src,
            true,
            progress,
            decision_rx,
            bytes_written,
            artwork_counts,
        )?;
        progress.track_done();
    }
    transcoder.finish()?;

    Ok(retry_outcome.deferred)
}
/// Result of the worker-safe transcode half (`transcode_one`) for one source.
/// Carries every field the committer half (`commit_transcoded`) needs to add
/// the file to libgpod and build the manifest entry, so the committer never
/// has to re-read or re-probe the source file.
pub(crate) struct Transcoded {
    pub temp: std::path::PathBuf,
    pub tags: crate::ipod::db::Tags,
    pub art: Option<Vec<u8>>,
    /// "ffmpeg" | "refalac" | "passthrough" — matches the manifest field.
    pub encoder: String,
    /// Forensic version string (e.g. "ffmpeg version n7.0 ..." or
    /// "refalac 1.85"). Empty for passthrough.
    pub encoder_version: String,
    /// ffprobe codec_name of the source (e.g. "flac", "mp3", "aac").
    pub source_format: String,
    pub fingerprint: String,
    pub audio_fingerprint: String,
    /// Task 13: this source's cover-art extraction outcome, for
    /// `ArtworkSummary` accounting. Derived alongside `art` below.
    pub art_outcome: ArtOutcome,
}

/// Worker-safe half of the old `add_one`: probe → classify → branch on
/// Passthrough/Transcode(encoder) → extract art → compute both file + audio
/// fingerprints. Touches ONLY the filesystem — NEVER libgpod — so pipeline
/// worker threads can run this concurrently while a single committer thread
/// owns `OwnedDb`. Both fingerprints are computed here (not by the walker)
/// because Add/Modify are the only code paths that need them — the
/// steady-state Unchanged path stays stat-only. The audio fingerprint lets
/// future runs detect tag-only edits and take the MetadataOnly fast path.
///
/// The encoder branch:
/// - Passthrough: byte-for-byte copy of the source (mp3 / aac / alac /
///   optionally wav). Encoder recorded as "passthrough" so the
///   encoder-mismatch heuristic can carve it out from future re-encodes.
/// - Transcode + Ffmpeg: existing `transcode_to_alac` (single-step ffmpeg
///   FLAC→ALAC, art passthrough).
/// - Transcode + Refalac: 2-step `transcode_via_refalac` (ffmpeg-decode to
///   WAV, then refalac to ALAC, with optional --artwork from a temp jpg).
pub(crate) fn transcode_one(
    src: &SourceEntry,
    config: &Config,
    refalac_version: &Option<String>,
) -> Result<Transcoded> {
    let probe = transcode::probe(&src.path, &config.ffmpeg)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);
    let source_format = source_format_from_probe(&probe);

    let classify_cfg = transcode::ClassifyConfig {
        passthrough_wav: config.passthrough_wav,
    };
    let action = transcode::classify(&probe, &classify_cfg)
        .with_context(|| format!("classify {}", src.path.display()))?;
    let is_transcode = matches!(action, SourceAction::Transcode);

    // Resolve the on-disk file we'll feed into libgpod, plus the
    // encoder identity to record in the manifest.
    let (encoder, encoder_version, temp): (String, String, std::path::PathBuf) = match action {
        SourceAction::Passthrough => {
            let dst = transcode::temp_passthrough_path(&src.path);
            transcode::passthrough(&src.path, &dst)
                .with_context(|| format!("passthrough copy for {}", src.path.display()))?;
            ("passthrough".to_string(), String::new(), dst)
        }
        SourceAction::Transcode => match config.encoder {
            EncoderChoice::Ffmpeg => {
                let dst = transcode::temp_alac_path();
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                transcode::transcode_to_alac(&src.path, &dst, &config.ffmpeg)
                    .with_context(|| format!("transcode {}", src.path.display()))?;
                let ver = ffmpeg_version(&config.ffmpeg)
                    .unwrap_or_else(|_| "ffmpeg (version unknown)".to_string());
                ("ffmpeg".to_string(), ver, dst)
            }
            EncoderChoice::Refalac => {
                let dst = transcode::temp_alac_path();
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                let ffmpeg_path = config.ffmpeg.as_path();
                transcode::transcode_via_refalac(
                    &src.path,
                    &dst,
                    &config.refalac_path,
                    ffmpeg_path,
                    None, // audio-only: art embedded later by artwork::embed
                )
                .with_context(|| format!("refalac transcode {}", src.path.display()))?;
                let ver = refalac_version
                    .clone()
                    .unwrap_or_else(|| "refalac (version unknown)".to_string());
                ("refalac".to_string(), ver, dst)
            }
        },
    };

    // Extract source art once; normalize to a small baseline JPEG that feeds
    // BOTH libgpod's Apple thumbnails AND (when rockbox_compat) the embedded
    // covr atom. Non-fatal: on any art failure, fall back to no art.
    let has_art = has_embedded_art(&probe);
    let art: Option<Vec<u8>> = if has_art {
        let art_path = transcode::temp_art_path();
        let raw = transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        match raw {
            Ok(bytes) => match crate::artwork::normalize(&bytes) {
                Ok(norm) => Some(norm),
                Err(e) => {
                    tracing::warn!(
                        "art normalize failed for {}: {e:#}; using raw bytes",
                        src.path.display()
                    );
                    Some(bytes)
                }
            },
            Err(e) => {
                tracing::warn!("art extract failed for {}: {e:#}", src.path.display());
                None
            }
        }
    } else {
        None
    };
    // Task 13: NoArt/Embedded/Failed for the run's ArtworkSummary. The
    // `art extract failed` warn above already logs source path + reason —
    // this just classifies that same outcome for counting, never silently.
    let art_outcome = ArtOutcome::from_probe(has_art, art.is_some());

    // Rockbox: make the transcoded .m4a self-describing (tags + art). Only for
    // transcoded output — passthrough files keep their own metadata. Non-fatal.
    if config.rockbox_compat && is_transcode {
        if let Err(e) = crate::artwork::embed_track_metadata(&temp, &tags, art.as_deref()) {
            tracing::warn!("rockbox embed failed for {}: {e:#}", src.path.display());
        }
    }

    let fingerprint = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;
    let audio_fingerprint = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;

    Ok(Transcoded {
        temp,
        tags,
        art,
        encoder,
        encoder_version,
        source_format,
        fingerprint,
        audio_fingerprint,
        art_outcome,
    })
}

/// Committer half of the old `add_one`: add the already-transcoded file to
/// libgpod (wrapped in `retry_transient` — iPod writes are the transient
/// failure class this whole feature exists to smooth over) and push the
/// manifest entry. MUST run on the single committer thread — this is the
/// only function in the Add/Modify path that touches `OwnedDb`.
///
/// Borrows `Transcoded` rather than consuming it so a caller's Retry/Skip/
/// Abort loop can re-attempt the commit against the SAME transcoded temp
/// file without re-running the (expensive, and already-succeeded) transcode.
/// The temp file is removed ONLY after a successful add, so a failed attempt
/// leaves it in place for the retry.
fn commit_transcoded(
    db: &OwnedDb,
    manifest: &mut Manifest,
    src: &SourceEntry,
    t: &Transcoded,
) -> Result<()> {
    let handle = crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
        db.add_track_with_file(&t.temp, &t.tags, t.art.as_deref())
    })
    .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&t.temp);

    manifest.tracks.push(entry_from(
        src,
        &handle,
        &t.fingerprint,
        &t.audio_fingerprint,
        &t.encoder,
        &t.encoder_version,
        &t.source_format,
    ));
    Ok(())
}

/// Bridges a pipelined (already-dispatched) transcode job to the committer.
/// `idx` is the action index the job was dispatched under; `transcoder.take`
/// blocks until that job's result is ready, then frees a pipeline window
/// permit — so this MUST be called exactly once for every index that was
/// handed to `OrderedTranscoder::start`, even when the caller ends up
/// discarding the result (`commit = false`).
///
/// - `take` returning `Err` is a deterministic transcode failure (bad file,
///   ffmpeg/afconvert failure, etc.) — NOT retried; logged and skipped.
/// - `take` returning `Ok(t)` with `commit = false` means the caller already
///   decided this job's output must not land on the iPod (currently: a
///   Modify whose delete-old half failed and was skipped). The temp file is
///   still cleaned up here.
/// - `take` returning `Ok(t)` with `commit = true` runs the normal
///   Retry/Skip/Abort loop around `commit_transcoded`, reusing the same `t`
///   on Retry (the transcode already succeeded — only the libgpod add needs
///   retrying).
#[allow(clippy::too_many_arguments)]
fn commit_pipelined(
    transcoder: &OrderedTranscoder<Transcoded>,
    idx: usize,
    db: &OwnedDb,
    manifest: &mut Manifest,
    src: &SourceEntry,
    commit: bool,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    bytes_written: &mut u64,
    artwork_counts: &mut ArtworkCounts,
) -> Result<()> {
    let t = match transcoder.take(idx) {
        Ok(t) => t,
        Err(e) => {
            if commit {
                progress.error(format!(
                    "Transcode failed for {}: {e:#}",
                    src.path.display()
                ));
                progress.log(format!("Skipped {} (transcode failed)", src.path.display()));
            }
            return Ok(());
        }
    };
    if !commit {
        let _ = std::fs::remove_file(&t.temp);
        return Ok(());
    }
    // Stat before commit_transcoded runs — it removes `t.temp` on success, so
    // this is the last point the actual on-iPod file size is readable. Task
    // 8's running tally wants the real transcoded/passthrough size, not the
    // source FLAC's (which is what fit's budgeting uses as an estimate).
    let size = std::fs::metadata(&t.temp).map(|m| m.len()).unwrap_or(0);
    loop {
        match commit_transcoded(db, manifest, src, &t) {
            Ok(()) => {
                *bytes_written += size;
                // Task 13: only count this source's art outcome once it's
                // actually landed on the device — a track that never
                // committed never got its art written either.
                artwork_counts.record(t.art_outcome);
                return Ok(());
            }
            Err(e) => match prompt_retry_skip_abort(progress, decision_rx, src, &e)? {
                PromptOutcome::Retry => continue,
                PromptOutcome::Skip => {
                    progress.log(format!("Skipped {} (commit failed)", src.path.display()));
                    return Ok(());
                }
                _ => return Err(e),
            },
        }
    }
}

/// Shared Retry/Skip/Abort prompt for `transcode_one`/`commit_transcoded`
/// failures in the Add arm and the Modify arm's re-add half. Extracted here
/// because both arms previously duplicated the identical prompt wording,
/// options, and outcome mapping.
fn prompt_retry_skip_abort(
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    src: &SourceEntry,
    e: &anyhow::Error,
) -> Result<PromptOutcome> {
    let msg = format!("Failed to add {}:\n  {e:#}\n\nChoose:", src.path.display());
    await_prompt(
        progress,
        decision_rx,
        msg,
        &["Retry", "Skip this track", "Abort"],
        &[
            PromptOutcome::Retry,
            PromptOutcome::Skip,
            PromptOutcome::Abort,
        ],
    )
}

/// Take the last 2 path components for display ("Album\Track.flac"). Falls
/// back to the full path if there's no parent. Used by track_start labels so
/// long UNC paths don't overflow the TUI current-track panel width. The full
/// path is still in the manifest and in earlier `progress.log` lines, just
/// not in the live status line.
fn display_path(p: &Path) -> String {
    let parent = p.parent().and_then(|p| p.file_name());
    let file = p.file_name();
    match (parent, file) {
        (Some(parent), Some(file)) => {
            format!("{}\\{}", parent.to_string_lossy(), file.to_string_lossy())
        }
        _ => p.display().to_string(),
    }
}

pub(crate) fn count_actions(actions: &[Action]) -> (usize, usize, usize, usize, usize) {
    let mut add = 0;
    let mut modify = 0;
    let mut metadata_only = 0;
    let mut remove = 0;
    let mut unchanged = 0;
    for a in actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::MetadataOnly { .. } => metadata_only += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    (add, modify, metadata_only, remove, unchanged)
}

pub(crate) fn entry_from(
    src: &SourceEntry,
    handle: &TrackHandle,
    fingerprint: &str,
    audio_fingerprint: &str,
    encoder: &str,
    encoder_version: &str,
    source_format: &str,
) -> ManifestEntry {
    ManifestEntry {
        source_path: src.path.clone(),
        source_mtime: src.mtime,
        source_size: src.size,
        source_fingerprint: fingerprint.to_string(),
        ipod_dbid: handle.dbid,
        ipod_relpath: handle.ipod_relpath.clone(),
        source_known: true,
        audio_fingerprint: audio_fingerprint.to_string(),
        // Recorded per-track from transcode_one's classify+encoder branch so
        // future runs can detect encoder-mismatch (or carve out passthrough
        // entries that have no encoder identity to mismatch against).
        encoder: encoder.to_string(),
        encoder_version: encoder_version.to_string(),
        source_format: source_format.to_string(),
    }
}

/// Embed tags + normalized art from `source` into the on-device `.m4a` at
/// `device_file`, in place (no re-transcode). Returns the new file size.
/// Non-fatal caller decides skip vs abort. Public(crate) for unit tests.
/// Probe a source file and return `(tags, normalized_cover_art)`. Shared by the
/// Rockbox `.m4a` embed and the Apple ArtworkDB rebuild. Art is normalized to a
/// small baseline JPEG (`artwork::normalize`); `None` when the source has no
/// embedded art. On normalize failure, falls back to the raw art bytes.
pub(crate) fn source_tags_and_art(
    source: &Path,
    ffmpeg: &Path,
) -> Result<(crate::ipod::db::Tags, Option<Vec<u8>>)> {
    let probe =
        transcode::probe(source, ffmpeg).with_context(|| format!("probe {}", source.display()))?;
    let tags = tags_from_probe(&probe);
    let art: Option<Vec<u8>> = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        let raw = transcode::extract_cover_art(source, &art_path, ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        raw.ok()
            .and_then(|b| crate::artwork::normalize(&b).ok().or(Some(b)))
    } else {
        None
    };
    Ok((tags, art))
}

// (A `backfill_one_file` wrapper over `source_tags_and_art` +
// `embed_track_metadata` used to live here; the production backfill paths
// call the pair directly, so the wrapper was dead code — its end-to-end
// fixture test survives below, exercising the same pair.)

/// Refresh artwork + metadata for the already-synced library on BOTH firmwares,
/// WITHOUT re-copying audio. Used by the "Update existing library" command and
/// after a bulk source retag (Lidarr etc.).
///
/// Two phases, no re-transcode:
///  1. **Rockbox** — embed each track's tags + normalized cover art into its
///     on-device `.m4a` (Rockbox reads tags/art straight from the file). Only
///     for transcoded ALAC output; passthrough files (mp3/aac/…) already carry
///     their own tags/art, so their file is left alone.
///  2. **Apple** — rebuild the ArtworkDB fresh for ALL managed tracks:
///     re-thumbnail each from source art, DELETE the stale ithmb/ArtworkDB,
///     then `itdb_write`. The delete-then-build is essential — `itdb_write`'s
///     REWRITE path drops cover-art thumbnails loaded from an existing DB
///     (verified on-device: F1069 deleted), while its fresh-BUILD path (no stale
///     ithmb present) writes them correctly. Also refreshes the iTunesDB tags.
///
/// Safety: if the iPod holds tracks classick doesn't manage (absent from the
/// manifest, or skipped this run), the fresh rebuild would drop THEIR Apple art
/// (we can't regenerate it — no source), so Phase 2 is skipped and a warning is
/// logged; the Rockbox file updates from Phase 1 still stand. Per-track failures
/// skip (warn), never abort.
pub fn backfill_rockbox(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<RunOutcome> {
    let mount = preflight::resolve_ipod_mount(config, progress, decision_rx)?;
    // Same per-device manifest as `run`: this reads (never writes) the
    // manifest, but it must be the same file `run` uses, or a backfill
    // would silently operate on a stale/legacy copy of the track list.
    let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
    let serial = device_state::sanitize_serial(&identity.firewire_guid);
    let source_location = configured_source_location(config)?;
    let manifest = manifest_store(config, Path::new(&mount), &serial)?
        .load(&source_location)?
        .manifest;
    let entries: Vec<&crate::manifest::ManifestEntry> = manifest
        .tracks
        .iter()
        .filter(|e| e.source_known && !e.ipod_relpath.is_empty())
        .collect();
    let total = entries.len();
    progress.summary(0, 0, total, 0, 0, total);
    progress.log(format!(
        "Refreshing artwork + metadata for {total} track(s) — no audio re-copy…"
    ));

    // Phase 1: per-track Rockbox embed + collect (dbid, tags, art) for Phase 2.
    let (mut updated, mut skipped) = (0usize, 0usize);
    let mut refreshed: Vec<(u64, crate::ipod::db::Tags, Option<Vec<u8>>)> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let label = entry
            .source_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        progress.track_start(i + 1, total, label);
        let device_file = Path::new(&mount).join(
            entry
                .ipod_relpath
                .replace('\\', std::path::MAIN_SEPARATOR_STR),
        );
        if !device_file.exists() || !entry.source_path.exists() {
            skipped += 1;
            progress.track_done();
            continue;
        }
        match source_tags_and_art(&entry.source_path, &config.ffmpeg) {
            Ok((tags, art)) => {
                // Rockbox: embed into the .m4a — transcoded ALAC only.
                // Passthrough files already carry their own tags/art.
                if entry.encoder != "passthrough" {
                    if let Err(e) =
                        crate::artwork::embed_track_metadata(&device_file, &tags, art.as_deref())
                    {
                        tracing::warn!("embed {} failed: {e:#}", device_file.display());
                    }
                }
                refreshed.push((entry.ipod_dbid, tags, art));
                updated += 1;
            }
            Err(e) => {
                tracing::warn!("backfill skip {}: {e:#}", entry.source_path.display());
                skipped += 1;
            }
        }
        progress.track_done();
    }

    // Phase 2: rebuild the Apple ArtworkDB fresh so Apple firmware shows art too.
    match rebuild_apple_artwork(Path::new(&mount), &refreshed) {
        Ok(()) => progress.log("Apple firmware artwork rebuilt.".to_string()),
        Err(e) => progress.log(format!(
            "Apple artwork not rebuilt ({e:#}); Rockbox files still updated."
        )),
    }

    progress.log(format!(
        "Refresh complete: {updated} updated, {skipped} skipped."
    ));
    Ok(RunOutcome::Completed)
}

/// Phase 2 of `backfill_rockbox`: rebuild the Apple ArtworkDB fresh from
/// `refreshed` = `(dbid, tags, normalized_art)` for every managed track.
///
/// Returns `Err` (leaving the Apple side untouched) if the iPod holds tracks not
/// present in `refreshed` — the fresh rebuild deletes ALL thumbnails, so an
/// unmanaged track we don't re-thumbnail would lose its cover; refusing is safer
/// than silently clobbering art we can't regenerate.
fn rebuild_apple_artwork(
    mount: &Path,
    refreshed: &[(u64, crate::ipod::db::Tags, Option<Vec<u8>>)],
) -> Result<()> {
    let identity = device::resolve_libgpod_identity(mount)?;
    // Provision so the artwork format list (incl. F1069) is correct for the write.
    crate::ipod::sysinfo_provision::provision(mount, &identity).ok();
    let db = OwnedDb::open(mount)?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &identity.firewire_guid)?;
        device::set_model_num(device_ptr, &identity.model_num_str)?;
    }

    // Foreign/unmanaged-track guard: every track on the iPod must be in
    // `refreshed`, or the fresh rebuild would drop its (irretrievable) art.
    let managed: std::collections::HashSet<u64> = refreshed.iter().map(|(d, _, _)| *d).collect();
    let unmanaged = db
        .list_tracks_for_rebuild()
        .into_iter()
        .filter(|t| !managed.contains(&t.dbid))
        .count();
    if unmanaged > 0 {
        anyhow::bail!(
            "{unmanaged} on-iPod track(s) not covered by this refresh; \
             skipping Apple artwork rebuild to avoid dropping their covers"
        );
    }

    // Re-thumbnail every track from source art (into libgpod's in-memory state).
    for (dbid, tags, art) in refreshed {
        db.update_track_metadata(*dbid, tags, art.as_deref())
            .unwrap_or_else(|e| tracing::warn!("re-thumbnail dbid {dbid}: {e:#}"));
    }

    // Force libgpod's fresh-BUILD path: delete the stale ArtworkDB + ithmb so
    // itdb_write rebuilds them from the in-memory thumbnails (preserving F1069)
    // instead of taking the destructive rewrite path.
    let art_dir = mount.join("iPod_Control").join("Artwork");
    if let Ok(rd) = std::fs::read_dir(&art_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
                if n.ends_with(".ithmb") || n == "ArtworkDB" {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
    write_rebuilt_artwork_database(&db)?;
    Ok(())
}

pub fn write_rebuilt_artwork_database(db: &OwnedDb) -> Result<()> {
    crate::sync_transaction::write_coordinated_database(db)
        .context("write rebuilt Apple artwork database")
}

pub(crate) fn build_rebuild_manifest(db: &OwnedDb, serial: &str) -> Manifest {
    build_rebuild_manifest_from_handles(db.list_tracks_for_rebuild(), serial)
}

/// Pure core of [`build_rebuild_manifest`], split out so it's unit-testable
/// without a real libgpod-backed `OwnedDb`.
pub(crate) fn build_rebuild_manifest_from_handles(
    handles: Vec<TrackHandle>,
    serial: &str,
) -> Manifest {
    let tracks = handles
        .into_iter()
        .map(|h| ManifestEntry {
            source_path: std::path::PathBuf::new(),
            source_mtime: 0,
            source_size: 0,
            source_fingerprint: String::new(),
            ipod_dbid: h.dbid,
            ipod_relpath: h.ipod_relpath,
            source_known: false,
            audio_fingerprint: String::new(),
            // Rebuilt entries have no source file to introspect — encoder and
            // source_format are genuinely unknown. The encoder-mismatch carve-out
            // for "unknown" keeps these untouched on first run after rebuild,
            // and they'll get refilled on the next Modify via entry_from.
            encoder: "unknown".to_string(),
            encoder_version: String::new(),
            source_format: "flac".to_string(),
        })
        .collect();
    // last_source_root is intentionally None: the iPod's DB doesn't carry
    // the original source library root. The authority store resolves the
    // current source when it encodes this runtime model as portable v2.
    Manifest {
        version: 1,
        ipod_serial: Some(serial.to_string()),
        last_source_root: None,
        tracks,
    }
}

/// Joins desired source paths to this run's DBIDs, loads the connected
/// device's ownership authority, and returns an in-memory reconcile candidate
/// for coordinated checkpoint publication. `desired` is `(slug, display name,
/// resolved source paths)` — `slug` is threaded through as the
/// managed-identity join key (Fix 2: `reconcile` used to key on display
/// name alone, which collapses two distinct playlists that happen to share
/// a name via `PlaylistStore::unique_slug`'s `-2` disambiguation). Extracted
/// from `run` so every fallible step in here — the dbid join is infallible,
/// while display names are presentation only. Mutation failures propagate so
/// the checkpoint can roll back the candidate DB; ownership is never written
/// here.
///
/// `pub` (not `pub(crate)`) so integration tests can drive the full
/// slug→display-name / manifest→dbid seam without duplicating it — see
/// `tests/playlists_e2e.rs`'s `reconcile_through_reconcile_playlists_step_*`
/// tests (Fix 3).
pub fn reconcile_playlists_step(
    db: &OwnedDb,
    desired: &[(String, String, Vec<PathBuf>)],
    manifest: &Manifest,
    serial: &str,
    progress: &Progress,
) -> Result<()> {
    reconcile_playlists_candidate_step(db, desired, manifest, serial, progress).map(drop)
}

pub fn reconcile_playlists_candidate_step(
    db: &OwnedDb,
    desired: &[(String, String, Vec<PathBuf>)],
    manifest: &Manifest,
    serial: &str,
    progress: &Progress,
) -> Result<crate::ipod::device_playlists::PlaylistReconcileOutcome> {
    let root = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve config dir"))?
        .join(crate::PROJECT_DIR);
    reconcile_playlists_candidate_step_in(db, desired, manifest, &root, serial, progress)
}

/// Test/override variant of [`reconcile_playlists_step`]. `root` selects only
/// the inert host-cache location; connected-device ownership remains the sole
/// authority loaded from the DB's mount.
pub fn reconcile_playlists_step_in(
    db: &OwnedDb,
    desired: &[(String, String, Vec<PathBuf>)],
    manifest: &Manifest,
    root: &Path,
    serial: &str,
    progress: &Progress,
) -> Result<()> {
    reconcile_playlists_candidate_step_in(db, desired, manifest, root, serial, progress).map(drop)
}

pub fn reconcile_playlists_candidate_step_in(
    db: &OwnedDb,
    desired: &[(String, String, Vec<PathBuf>)],
    manifest: &Manifest,
    root: &Path,
    serial: &str,
    progress: &Progress,
) -> Result<crate::ipod::device_playlists::PlaylistReconcileOutcome> {
    let dbid_by_source_path: std::collections::HashMap<&Path, u64> = manifest
        .tracks
        .iter()
        .map(|e| (e.source_path.as_path(), e.ipod_dbid))
        .collect();
    let desired: Vec<crate::ipod::device_playlists::DesiredPlaylist> = desired
        .iter()
        .map(|(slug, name, paths)| {
            let dbids: Vec<u64> = paths
                .iter()
                .filter_map(|p| dbid_by_source_path.get(p.as_path()).copied())
                .collect();
            crate::ipod::device_playlists::DesiredPlaylist {
                slug: slug.clone(),
                display_name: name.clone(),
                ordered_dbids: dbids,
            }
        })
        .collect();
    let mount = db
        .mount_path()
        .context("resolve iPod mount for playlist ownership")?;
    let host_cache = crate::device_state::managed_playlists_path_in(root, serial)?;
    let ownership_store = crate::ipod::playlist_ownership::DeviceOwnershipStore::new(
        mount,
        serial.to_string(),
        host_cache,
        crate::atomic_file::AtomicFileWriter::new(),
    );
    let previous = ownership_store
        .load_device_read_only()
        .context("load device-authoritative playlist ownership")?;
    let outcome = crate::ipod::device_playlists::reconcile_candidate(db, &desired, &previous)
        .context("reconcile Classick-managed iTunesDB playlists")?;
    let stats = outcome.stats;
    if stats.created + stats.updated + stats.removed > 0 {
        progress.log(format!(
            "playlists: {} created, {} updated, {} removed",
            stats.created, stats.updated, stats.removed
        ));
    }
    Ok(outcome)
}

/// Guard against a raw source walk that found zero audio files. An empty
/// walk almost always means something's wrong (unmounted NAS share, typo'd
/// path, wrong drive letter) — silently proceeding would let the diff read
/// it as "the user emptied their library" and plan a Remove for every track
/// on the iPod. Runs on the RAW walk result, BEFORE selection filtering, so
/// a selection that legitimately excludes everything (an explicit user
/// action) is unaffected — see `selection::apply_to_sources`.
pub(crate) fn guard_nonempty_walk(sources: &[SourceEntry], root: &Path) -> Result<()> {
    if sources.is_empty() {
        anyhow::bail!(
            "Source library at {} contains no audio files — not syncing. \
             If you meant to empty this iPod, use Replace Library.",
            root.display()
        );
    }
    Ok(())
}

/// Per-track cover-art outcome for one Add/Modify/MetadataOnly action,
/// feeding `ArtworkCounts`/`ArtworkSummary`. Derived from whether the source
/// probe reported embedded art and whether extraction/decode of it
/// succeeded — never from whether the track itself made it onto the device
/// (a whole-track commit failure is already surfaced via the existing
/// Retry/Skip/Abort prompt + log lines, and isn't double-counted here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtOutcome {
    /// Source had no embedded art — not counted as eligible.
    NoArt,
    /// Source had embedded art and it was extracted/decoded successfully.
    Embedded,
    /// Source had embedded art but extraction/decode failed.
    Failed,
}

impl ArtOutcome {
    pub fn from_probe(has_art: bool, extracted_ok: bool) -> Self {
        match (has_art, extracted_ok) {
            (false, _) => ArtOutcome::NoArt,
            (true, true) => ArtOutcome::Embedded,
            (true, false) => ArtOutcome::Failed,
        }
    }
}

/// Running tally of `ArtOutcome`s across a run's Add/Modify/MetadataOnly
/// actions, converted to the wire `ArtworkSummary` at Finish. Pure
/// accumulator — no I/O, easy to unit-test in isolation from the apply loop.
/// `pub` (not `pub(crate)`) because `retry_deferred` is `pub` for the
/// integration-test harness (`tests/fit_retry_integration.rs`,
/// `tests/wipe_all_tracks_integration.rs`) and now takes `&mut ArtworkCounts`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtworkCounts {
    pub eligible: usize,
    pub embedded: usize,
    pub failed_sources: usize,
}

impl ArtworkCounts {
    pub fn record(&mut self, outcome: ArtOutcome) {
        match outcome {
            ArtOutcome::NoArt => {}
            ArtOutcome::Embedded => {
                self.eligible += 1;
                self.embedded += 1;
            }
            ArtOutcome::Failed => {
                self.eligible += 1;
                self.failed_sources += 1;
            }
        }
    }

    pub fn to_summary(self) -> crate::ipc::ArtworkSummary {
        crate::ipc::ArtworkSummary {
            embedded: self.embedded,
            eligible: self.eligible,
            failed_sources: self.failed_sources,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded_with_identity(
        identity: Option<crate::source_location::SourceIdentity>,
        legacy_root: &str,
    ) -> crate::manifest_store::LoadedManifest {
        crate::manifest_store::LoadedManifest {
            manifest: Manifest {
                version: 2,
                ipod_serial: Some("SERIAL".into()),
                last_source_root: Some(PathBuf::from(legacy_root)),
                tracks: vec![crate::manifest::ManifestEntry {
                    source_path: PathBuf::from(legacy_root).join("track.flac"),
                    source_mtime: 0,
                    source_size: 1,
                    source_fingerprint: "fp".into(),
                    ipod_dbid: 1,
                    ipod_relpath: "iPod_Control/Music/F00/track.m4a".into(),
                    source_known: true,
                    audio_fingerprint: String::new(),
                    encoder: "unknown".into(),
                    encoder_version: String::new(),
                    source_format: "flac".into(),
                }],
            },
            origin: crate::manifest_store::ManifestOrigin::DeviceV2,
            needs_device_publish: false,
            source_identity: identity,
        }
    }

    fn smb_source(root: &str, share: &str) -> crate::source_location::SourceLocation {
        crate::source_location::SourceLocation {
            resolved_path: PathBuf::from(root),
            identity: crate::source_location::SourceIdentity::Smb {
                host: "jupiter".into(),
                share: share.into(),
                subpath: Some(crate::portable_path::PortablePath::parse("media/music").unwrap()),
            },
        }
    }

    #[test]
    fn source_safeguard_accepts_same_smb_identity_at_an_alternate_mount() {
        let source = smb_source("/Volumes/data-1/media/music", "data");
        let loaded =
            loaded_with_identity(Some(source.identity.clone()), "/Volumes/data/media/music");

        assert!(!source_change_requires_confirmation(&loaded, &source));
    }

    #[test]
    fn source_safeguard_rejects_a_different_smb_share_before_legacy_root() {
        let recorded = smb_source("/Volumes/data/media/music", "archive");
        let current = smb_source("/Volumes/data/media/music", "data");
        let loaded = loaded_with_identity(Some(recorded.identity), "/Volumes/data/media/music");

        assert!(source_change_requires_confirmation(&loaded, &current));
    }

    #[test]
    fn source_safeguard_falls_back_to_legacy_root_without_logical_identity() {
        let current = smb_source("/Volumes/data-1/media/music", "data");
        let loaded = loaded_with_identity(None, "/Volumes/data/media/music");

        assert!(source_change_requires_confirmation(&loaded, &current));
    }

    #[test]
    fn save_config_projection_preserves_the_logical_source_identity() {
        let mut config = test_config_for_preflight(PathBuf::from("/Volumes/data-1/media/music"));
        config.save_config = true;
        let source = smb_source("/Volumes/data-1/media/music", "data");

        let persisted = persisted_config_for_sync(&config, &source);

        assert_eq!(persisted.source_location, Some(source));
    }

    #[test]
    fn unsaved_direct_cli_smb_sources_receive_portable_identities() {
        let base =
            std::env::temp_dir().join(format!("classick-direct-cli-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        for source in [
            r"\\JUPITER\Data\media\music",
            "smb://JUPITER/Data/media/music",
        ] {
            let mut config = test_config_for_preflight(PathBuf::from(source));
            config.manifest_path = base.join("manifest.json");
            let location = configured_source_location(&config).unwrap();

            assert!(matches!(
                location.identity,
                SourceIdentity::Smb {
                    ref host,
                    ref share,
                    ..
                } if host.eq_ignore_ascii_case("jupiter")
                    && share.eq_ignore_ascii_case("data")
            ));
        }
    }

    #[test]
    fn configured_source_rejects_a_different_filesystem_at_the_saved_smb_path() {
        let base = std::env::temp_dir().join(format!(
            "classick-source-identity-verification-{}",
            std::process::id()
        ));
        let source = base.join("mounted-source");
        std::fs::create_dir_all(&source).unwrap();
        let config_path = base.join("config.toml");
        config_file::save(
            &config_path,
            &config_file::PersistedConfig {
                source: Some(source.clone()),
                source_location: Some(SourceLocation {
                    resolved_path: source.clone(),
                    identity: SourceIdentity::Smb {
                        host: "jupiter".into(),
                        share: "data".into(),
                        subpath: Some(
                            crate::portable_path::PortablePath::parse("media/music").unwrap(),
                        ),
                    },
                }),
                ..config_file::PersistedConfig::default()
            },
        )
        .unwrap();
        let mut config = test_config_for_preflight(source);
        config.manifest_path = base.join("manifest.json");

        let error = configured_source_location(&config).unwrap_err();
        assert!(error
            .to_string()
            .contains("resolved source is a different SMB location"));
        let _ = std::fs::remove_dir_all(base);
    }

    /// encoder_str is what `run` feeds into `manifest::diff` as the target
    /// encoder name and what `transcode_one` records on each new entry. Mismatches
    /// here would silently break the encoder-mismatch diff branch — every
    /// fresh entry would look like a mismatch against itself.
    #[test]
    fn encoder_str_maps_choices_to_manifest_strings() {
        assert_eq!(encoder_str(EncoderChoice::Ffmpeg), "ffmpeg");
        assert_eq!(encoder_str(EncoderChoice::Refalac), "refalac");
    }

    fn device_settings_with_rockbox(rockbox_compat: bool) -> crate::device_config::DeviceSettings {
        crate::device_config::DeviceSettings {
            rockbox_compat,
            ..crate::device_config::DeviceSettings::default()
        }
    }

    /// The CLI `--rockbox-compat` flag force-enables for one run regardless
    /// of what the device's own settings.json says.
    #[test]
    fn effective_rockbox_cli_flag_forces_on() {
        assert!(effective_rockbox(
            true,
            &device_settings_with_rockbox(false)
        ));
        assert!(effective_rockbox(true, &device_settings_with_rockbox(true)));
    }

    /// Without the CLI flag, the device's own setting decides.
    #[test]
    fn effective_rockbox_without_cli_flag_follows_device_setting() {
        assert!(!effective_rockbox(
            false,
            &device_settings_with_rockbox(false)
        ));
        assert!(effective_rockbox(
            false,
            &device_settings_with_rockbox(true)
        ));
    }

    fn guard_test_entry(path: &str) -> SourceEntry {
        SourceEntry {
            path: std::path::PathBuf::from(path),
            mtime: 1,
            size: 10,
        }
    }

    /// A raw walk that found zero audio files must never be treated as "the
    /// user wants an empty iPod" — that's a removal plan in disguise (e.g. a
    /// disconnected NAS share, a typo'd source path). It's a hard error.
    #[test]
    fn guard_nonempty_walk_rejects_empty_walk() {
        let err = guard_nonempty_walk(&[], Path::new("/m/music")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("contains no audio files"), "got: {msg}");
        assert!(msg.contains("/m/music"), "got: {msg}");
    }

    #[test]
    fn guard_nonempty_walk_allows_nonempty_walk() {
        let sources = vec![guard_test_entry("/m/music/a.flac")];
        assert!(guard_nonempty_walk(&sources, Path::new("/m/music")).is_ok());
    }

    /// The guard runs on the RAW walk result, before selection filtering.
    /// A selection that legitimately excludes every track (an explicit user
    /// action) must still proceed — it's `apply_to_sources`/`filter`'s job to
    /// produce an empty Vec here, not the guard's job to reject it.
    #[test]
    fn guard_passes_raw_walk_even_when_selection_would_filter_everything() {
        let sources = vec![guard_test_entry("/m/music/a.flac")];
        // Guard sees the raw, non-empty walk result and allows it through.
        assert!(guard_nonempty_walk(&sources, Path::new("/m/music")).is_ok());

        // Downstream, selection filtering to zero is still allowed to happen —
        // it never reaches (or re-triggers) the guard.
        let mut index =
            crate::library_index::LibraryIndex::empty(std::path::PathBuf::from("/m/music"));
        let sel = crate::selection::Selection {
            version: 1,
            mode: crate::selection::SelectionMode::Include,
            rules: vec![crate::selection::SelectionRule::Artist {
                name: "Nobody".into(),
            }],
        };
        let (kept, _dirty) = crate::selection::filter(sources, &sel, &mut index, |_| {
            Ok(crate::library_index::TrackTags {
                artist: "Someone Else".into(),
                album_artist: String::new(),
                album: "Album".into(),
                genre: "Genre".into(),
                title: String::new(),
                duration_ms: 0,
                year: None,
            })
        });
        assert!(
            kept.is_empty(),
            "selection filtering everything out must still be allowed"
        );
    }

    // -- final-review #1: replace_library validates before wiping --------

    /// RED/GREEN seam for final-review fix #1: `preflight_replace` must
    /// reject an empty source directory with the same "contains no audio
    /// files" error `run()`'s own `guard_nonempty_walk` produces, so
    /// `replace_library` never reaches its confirmation prompt (let alone
    /// the wipe) for a doomed sync. Uses a real, empty tempdir — not a
    /// missing path — so `preflight::walk_source`'s walk succeeds (Ok(vec
    /// of zero)) and the guard is what fails; a missing path would instead
    /// enter `walk_source`'s retry/change/abort prompt loop and hang
    /// waiting on `decision_rx`.
    #[test]
    fn preflight_replace_rejects_empty_source() {
        let tmp = std::env::temp_dir().join(format!(
            "classick-preflight-replace-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut config = test_config_for_preflight(tmp.clone());
        let (progress, decision_rx) = crate::progress::Progress::start(false, false).unwrap();

        let err = preflight_replace(&mut config, &progress, &decision_rx).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("contains no audio files"), "got: {msg}");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Mirror-image GREEN case: a source with one real audio file passes
    /// the guard and the walk result comes back non-empty.
    #[test]
    fn preflight_replace_allows_nonempty_source() {
        let tmp = std::env::temp_dir().join(format!(
            "classick-preflight-replace-nonempty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("track.flac"),
            b"not really flac, walk() only checks the extension",
        )
        .unwrap();

        let mut config = test_config_for_preflight(tmp.clone());
        let (progress, decision_rx) = crate::progress::Progress::start(false, false).unwrap();

        let sources = preflight_replace(&mut config, &progress, &decision_rx).unwrap();
        assert_eq!(sources.len(), 1, "got: {sources:?}");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Minimal `Config` for driving `preflight_replace` in isolation — only
    /// `source` matters to `preflight::walk_source`/`guard_nonempty_walk`;
    /// every other field is a value that would blow up if actually used,
    /// which is the point (this seam must not touch them).
    fn test_config_for_preflight(source: std::path::PathBuf) -> Config {
        Config {
            source,
            ipod: None,
            ffmpeg: std::path::PathBuf::from("ffmpeg"),
            dry_run: false,
            apply: true,
            no_delete: false,
            verbose: false,
            rebuild_manifest: false,
            use_tui: false,
            manifest_path: std::path::PathBuf::from("/nonexistent-manifest.json"),
            save_config: false,
            encoder: EncoderChoice::Ffmpeg,
            refalac_path: std::path::PathBuf::from("refalac64"),
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

    /// source_format_from_probe pulls codec_name from the first audio stream
    /// (matching what classify uses). Any non-audio leading stream — for
    /// instance an attached_pic video stream that ffprobe sometimes emits
    /// first — must NOT win.
    #[test]
    fn source_format_from_probe_prefers_audio_stream_codec_name() {
        let json = r#"{
            "streams":[
                {"codec_type":"video","codec_name":"mjpeg"},
                {"codec_type":"audio","codec_name":"flac"}
            ],
            "format":{"format_name":"flac"}
        }"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(source_format_from_probe(&probe), "flac");
    }

    /// When an audio stream has no `codec_name` (defensive — Phase 1/2
    /// fixtures didn't carry it), fall back to the first component of
    /// `format.format_name`. Comma-split mirrors classify's container logic.
    #[test]
    fn source_format_from_probe_falls_back_to_first_format_name_segment() {
        let json = r#"{
            "streams":[{"codec_type":"audio"}],
            "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2"}
        }"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(source_format_from_probe(&probe), "mov");
    }

    /// Last resort: neither codec_name nor format_name — return "unknown"
    /// rather than panicking. Manifest entries can later get corrected on
    /// the next normal Modify.
    #[test]
    fn source_format_from_probe_returns_unknown_when_no_info() {
        let json = r#"{"streams":[],"format":{}}"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        assert_eq!(source_format_from_probe(&probe), "unknown");
    }

    // -- build_rebuild_manifest stamps ipod_serial -----------------------

    #[test]
    fn build_rebuild_manifest_from_handles_stamps_serial() {
        let handles = vec![
            TrackHandle {
                dbid: 111,
                ipod_relpath: r"iPod_Control\Music\F12\ABCD.m4a".to_string(),
            },
            TrackHandle {
                dbid: 222,
                ipod_relpath: r"iPod_Control\Music\F34\WXYZ.m4a".to_string(),
            },
        ];
        let m = build_rebuild_manifest_from_handles(handles, "SER123");
        assert_eq!(m.ipod_serial, Some("SER123".to_string()));
        assert_eq!(m.tracks.len(), 2);
        assert_eq!(m.tracks[0].ipod_dbid, 111);
        assert!(
            !m.tracks[0].source_known,
            "rebuilt entries have no known source"
        );
        assert_eq!(m.last_source_root, None);
    }

    // -- Task 8: compute_budget ------------------------------------------

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn compute_budget_none_storage_is_none() {
        assert_eq!(compute_budget(None, 1_000), None);
    }

    #[test]
    fn compute_budget_underflow_saturates_to_zero() {
        // 10GB total -> reserve is the 512MB floor (2% of 10GB < 512MB).
        // free=0, credit=0 -> 0 - 512MB must saturate to 0, not panic/wrap.
        let storage = free_space::StorageInfo {
            total_bytes: 10 * GB,
            free_bytes: 0,
        };
        assert_eq!(compute_budget(Some(storage), 0), Some(0));
    }

    #[test]
    fn compute_budget_normal_case_is_free_plus_credit_minus_reserve() {
        let storage = free_space::StorageInfo {
            total_bytes: 100 * GB,
            free_bytes: 10 * GB,
        };
        let remove_credit = 2 * GB;
        let reserve = crate::fit::reserve_bytes(storage.total_bytes); // 2% of 100GB = 2GB
        let expected = 10 * GB + remove_credit - reserve;
        assert_eq!(compute_budget(Some(storage), remove_credit), Some(expected));
    }

    #[test]
    fn compute_budget_zero_credit_is_free_minus_reserve() {
        let storage = free_space::StorageInfo {
            total_bytes: 100 * GB,
            free_bytes: 50 * GB,
        };
        let reserve = crate::fit::reserve_bytes(storage.total_bytes);
        assert_eq!(compute_budget(Some(storage), 0), Some(50 * GB - reserve));
    }

    // -- Task 8: deferred_add_actions ------------------------------------

    fn src_entry(path: &str, size: u64) -> SourceEntry {
        SourceEntry {
            path: std::path::PathBuf::from(path),
            mtime: 1_700_000_000,
            size,
        }
    }

    fn deferred_album(key: &str, tracks: usize, bytes: u64) -> DeferredAlbum {
        DeferredAlbum {
            key: key.to_string(),
            tracks,
            bytes,
        }
    }

    #[test]
    fn deferred_add_actions_reconstructs_only_deferred_albums() {
        let original_adds = vec![
            src_entry("/m/Kept/01.flac", 10),
            src_entry("/m/Deferred/01.flac", 10),
            src_entry("/m/Deferred/02.flac", 10),
        ];
        let deferred = vec![deferred_album("/m/Deferred", 2, 20)];
        let actions = deferred_add_actions(&original_adds, &deferred, |_| None);
        let paths: Vec<_> = actions
            .iter()
            .map(|a| match a {
                Action::Add(s) => s.path.clone(),
                other => panic!("expected only Add actions, got {other:?}"),
            })
            .collect();
        assert_eq!(
            paths,
            vec![
                std::path::PathBuf::from("/m/Deferred/01.flac"),
                std::path::PathBuf::from("/m/Deferred/02.flac"),
            ]
        );
    }

    #[test]
    fn deferred_add_actions_empty_when_nothing_deferred() {
        let original_adds = vec![src_entry("/m/Kept/01.flac", 10)];
        let actions = deferred_add_actions(&original_adds, &[], |_| None);
        assert!(actions.is_empty());
    }

    #[test]
    fn deferred_add_actions_respects_album_tag_grouping() {
        // Same identity fit::plan_fit uses: a tag wins over the parent dir.
        let original_adds = vec![
            src_entry("/m/dirX/trackA-1.flac", 10),
            src_entry("/m/dirX/trackA-2.flac", 10),
            src_entry("/m/dirY/01.flac", 10),
        ];
        let deferred = vec![deferred_album("Album Two", 2, 20)];
        let tag_of = |p: &Path| -> Option<String> {
            if p.to_string_lossy().contains("trackA") {
                Some("Album Two".to_string())
            } else {
                None
            }
        };
        let actions = deferred_add_actions(&original_adds, &deferred, tag_of);
        assert_eq!(
            actions.len(),
            2,
            "only the tagged pair belongs to the deferred album"
        );
    }

    // -- Task 8: skipped_for_space rollup ---------------------------------

    #[test]
    fn skipped_for_space_sums_across_albums() {
        let deferred = vec![deferred_album("A", 3, 100), deferred_album("B", 5, 200)];
        let rollup = skipped_for_space(&deferred);
        assert_eq!(rollup.albums, 2);
        assert_eq!(rollup.tracks, 8);
        assert_eq!(rollup.bytes, 300);
    }

    // -- Task 11: --replace-library pure helpers ---------------------------

    #[test]
    fn replace_confirmation_is_skipped_under_apply() {
        assert!(should_skip_replace_confirmation(true));
        assert!(!should_skip_replace_confirmation(false));
    }

    #[test]
    fn replace_confirm_message_names_the_track_count_and_is_irreversible() {
        let msg = replace_library_confirm_message(42);
        assert!(msg.contains("42"), "got: {msg}");
        assert!(msg.contains("erases all"), "got: {msg}");
        assert!(msg.contains("cannot be undone"), "got: {msg}");
    }

    #[test]
    fn replace_confirm_message_handles_zero_tracks() {
        let msg = replace_library_confirm_message(0);
        assert!(msg.contains("all 0 tracks"), "got: {msg}");
    }

    #[test]
    fn legacy_dirty_marker_is_removed_only_by_explicit_migration() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join(format!("legacy-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("artwork-dirty");
        std::fs::write(&marker, b"legacy").unwrap();

        clear_legacy_marker(Some(&marker)).unwrap();

        assert!(!marker.exists());
        clear_legacy_marker(Some(&marker)).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
    // -- Task 13: ArtworkCounts / ArtOutcome -------------------------------

    #[test]
    fn art_outcome_from_probe_classifies_no_art_embedded_and_failed() {
        assert_eq!(ArtOutcome::from_probe(false, false), ArtOutcome::NoArt);
        assert_eq!(ArtOutcome::from_probe(false, true), ArtOutcome::NoArt);
        assert_eq!(ArtOutcome::from_probe(true, true), ArtOutcome::Embedded);
        assert_eq!(ArtOutcome::from_probe(true, false), ArtOutcome::Failed);
    }

    #[test]
    fn artwork_counts_tallies_across_outcomes() {
        let mut counts = ArtworkCounts::default();
        counts.record(ArtOutcome::NoArt);
        counts.record(ArtOutcome::Embedded);
        counts.record(ArtOutcome::Embedded);
        counts.record(ArtOutcome::Failed);
        assert_eq!(counts.eligible, 3, "NoArt is not eligible");
        assert_eq!(counts.embedded, 2);
        assert_eq!(counts.failed_sources, 1);
    }

    #[test]
    fn artwork_counts_to_summary_maps_fields() {
        let mut counts = ArtworkCounts::default();
        counts.record(ArtOutcome::Embedded);
        counts.record(ArtOutcome::Failed);
        let summary = counts.to_summary();
        assert_eq!(summary.eligible, 2);
        assert_eq!(summary.embedded, 1);
        assert_eq!(summary.failed_sources, 1);
    }
}

#[cfg(test)]
mod backfill_tests {
    #[test]
    fn backfill_embeds_into_existing_device_file() {
        // Copy the bare fixture as a stand-in on-device .m4a.
        let dir = std::env::temp_dir().join(format!("classick-backfill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dev = dir.join("track.m4a");
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a"),
            &dev,
        )
        .unwrap();

        // The per-track backfill step exactly as production runs it
        // (apply_loop's backfill paths): probe source tags → normalize art
        // → embed into the on-device file.
        let src = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tagged.flac"
        )); // existing fixture: tags + embedded PNG art
        let before = std::fs::metadata(&dev).unwrap().len();
        let (tags, art) =
            super::source_tags_and_art(src, &std::path::PathBuf::from("ffmpeg")).unwrap();
        crate::artwork::embed_track_metadata(&dev, &tags, art.as_deref()).unwrap();
        let after = std::fs::metadata(&dev).unwrap().len();
        assert!(after >= before, "embedding should not shrink the file");
        // The returned tags + normalized art are what the caller re-applies to
        // the Apple ithmb (the art-break fix); a tagged source must yield them.
        assert!(
            tags.title.is_some() || tags.artist.is_some(),
            "tags extracted from source"
        );
        assert!(
            art.is_some(),
            "normalized cover art returned for a track with embedded art"
        );

        use lofty::file::TaggedFileExt;
        let tag = lofty::read_from_path(&dev).unwrap();
        assert!(tag.primary_tag().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;
    // Ensures Transcoded carries every field the manifest entry needs, so the
    // committer half can build an entry without re-reading the source.
    #[test]
    fn transcoded_has_manifest_fields() {
        let t = super::Transcoded {
            temp: std::path::PathBuf::from("/tmp/x.m4a"),
            tags: crate::ipod::db::Tags::default(),
            art: None,
            encoder: "ffmpeg".into(),
            encoder_version: "v".into(),
            source_format: "flac".into(),
            fingerprint: "fp".into(),
            audio_fingerprint: "afp".into(),
            art_outcome: super::ArtOutcome::NoArt,
        };
        assert_eq!(t.encoder, "ffmpeg");
        assert_eq!(t.source_format, "flac");
    }

    #[test]
    fn review_quit_is_an_explicit_cancelled_outcome() {
        assert_eq!(
            review_outcome(ReviewDecision::Quit),
            Some(RunOutcome::Cancelled)
        );
    }

    #[test]
    fn cancel_before_first_album_admits_nothing() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Decision::Review(ReviewDecision::Quit)).unwrap();
        let mut albums = vec![vec![1, 2], vec![3]].into_iter();
        assert_eq!(
            admit_next_album(&mut albums, &rx),
            Err(crate::progress::StopReason::Cancelled)
        );
        assert_eq!(albums.next(), Some(vec![1, 2]));
    }

    #[test]
    fn cancel_mid_album_drains_current_and_admits_no_next_album() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut albums = vec![vec![1, 2], vec![3, 4]].into_iter();
        assert_eq!(admit_next_album(&mut albums, &rx), Ok(Some(vec![1, 2])));
        tx.send(Decision::Review(ReviewDecision::Quit)).unwrap();
        assert_eq!(
            admit_next_album(&mut albums, &rx),
            Err(crate::progress::StopReason::Cancelled)
        );
        assert_eq!(albums.next(), Some(vec![3, 4]));
    }

    #[test]
    fn pause_after_current_album_has_a_distinct_outcome() {
        assert_eq!(
            outcome_for_stop(Some(crate::progress::StopReason::Paused)),
            RunOutcome::Paused
        );
    }
}
