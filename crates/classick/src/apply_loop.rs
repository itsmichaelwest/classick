//! Per-Action match arms (Add / Modify / MetadataOnly / Remove / Unchanged)
//! plus their supporting helpers — `transcode_one`, `commit_transcoded`,
//! `do_metadata_only`, `entry_from`, `build_rebuild_manifest`, `count_actions`.
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
use crate::pipeline::OrderedTranscoder;
use crate::preflight;
use crate::progress::{ActionPlanSummary, Decision, Progress, ReviewDecision};
use crate::source::{self, SourceEntry};
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
    Ok(stdout
        .lines()
        .next()
        .unwrap_or("ffmpeg")
        .trim()
        .to_string())
}

/// Effective Rockbox-compat for one run: the raw `--rockbox-compat` CLI flag
/// force-enables regardless of the device's persisted setting (matches the
/// pre-trust-package "CLI flag OR global setting" semantics — see
/// `config::resolve_with`'s `rockbox_compat` merge); otherwise the syncing
/// device's own setting wins. Pure so it's testable without touching the
/// device-settings file.
pub(crate) fn effective_rockbox(cli_flag: bool, device: &crate::device_config::DeviceSettings) -> bool {
    cli_flag || device.rockbox_compat
}

/// Outcome of `run`'s apply phase, distinct from the `anyhow::Result` outer
/// layer (which carries hard failures). `Completed` covers both a normal
/// finish and the early-return no-op paths (dry-run, nothing-to-do, review
/// Quit/DryRun) — none of those can have been paused. `Paused` means the
/// action loop broke early on `Decision::Pause`; the final db.write() +
/// manifest save already ran, so completed tracks are committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Paused,
}

pub fn run(config: &mut Config, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<RunOutcome> {
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
    let index_for_sync_set =
        library_idx.clone().unwrap_or_else(|| library_index::LibraryIndex::empty(config.source.clone()));
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
    let sources = match crate::playlist::PlaylistStore::default_root().and_then(crate::playlist::PlaylistStore::open) {
        Ok(store) => {
            let effective = crate::sync_set::compute(
                sources, &selection, &subscriptions, &store, &index_for_sync_set, &config.source,
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
            tracing::warn!("playlists: cannot open playlist store ({e:#}); syncing scope selection only");
            crate::selection::apply_to_sources(sources, &config.source, &serial, |msg| {
                progress.log(msg)
            })
        }
    };
    let _ = &effective_set; // sources + missing/error counts already consumed above
    // One-time move of the legacy flat manifest.json into the per-device
    // trust-package layout; a no-op once migrated. `config.manifest_path`
    // keeps meaning "the legacy/root path" (still test-overridable) — the
    // per-device path returned here is what load/save actually use.
    let manifest_path = device_state::migrate_legacy_manifest(&config.manifest_path, &serial)?;

    // 3. Load (or rebuild) manifest.
    let mut manifest = if config.rebuild_manifest {
        let db = open_with_auto_restore(Path::new(&mount), || {
            progress.log("Restored iPod database from backup after detecting corruption");
            progress.note_db_restored();
        })?;
        let rebuilt = build_rebuild_manifest(&db, &serial);
        // Save eagerly so a crash after this point doesn't lose the rebuild.
        manifest::save_atomic(&manifest_path, &rebuilt)?;
        rebuilt
    } else {
        let mut loaded = manifest::load_or_default(&manifest_path)?;
        if manifest_is_foreign(&loaded, &serial) {
            tracing::warn!(
                manifest_serial = ?loaded.ipod_serial,
                device_serial = %serial,
                manifest_path = %manifest_path.display(),
                "manifest belongs to a different iPod; treating as empty for this run",
            );
            progress.error(format!(
                "Manifest at {} was last synced to a different iPod (recorded serial {:?}, \
                 connected device serial {serial}). Treating it as empty for this run rather \
                 than risk mismatched track ownership.",
                manifest_path.display(),
                loaded.ipod_serial,
            ));
            for line in RECOVERY_HINT_LINES {
                progress.error((*line).to_string());
            }
            loaded = Manifest::empty();
        }
        // Stamp/adopt this device on every load — covers both a fresh
        // manifest (ipod_serial was None) and the foreign-reset case above.
        loaded.ipod_serial = Some(serial.clone());
        loaded
    };

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
        library_idx.as_ref()?.files.get(path).map(|t| t.album.clone())
    };
    let remove_credit: u64 = actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove(entry) => Some(entry.source_size),
            _ => None,
        })
        .sum();
    let storage = free_space::query(Path::new(&mount));
    if storage.is_none() {
        progress.log(
            "Could not determine free space on the iPod; syncing without a size budget."
                .to_string(),
        );
    }
    let budget = compute_budget(storage, remove_credit);
    // Snapshot of the pre-fit Add plan, kept around so the end-of-run retry
    // can reconstruct exactly the Adds belonging to deferred albums (plan_fit
    // only returns kept Actions + a DeferredAlbum summary, not the dropped
    // Actions themselves).
    let original_adds: Vec<SourceEntry> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Add(src) => Some(src.clone()),
            _ => None,
        })
        .collect();
    let fit_outcome = crate::fit::plan_fit(actions, budget, album_tag_of);
    let actions = fit_outcome.kept;
    let deferred = fit_outcome.deferred;
    if !deferred.is_empty() {
        progress.log(format!(
            "Deferred {} album(s) that don't fit this run's space budget; will retry once after \
             applying the rest.",
            deferred.len()
        ));
    }

    let (add, modify, metadata_only, remove, unchanged) = count_actions(&actions);

    // Task 13: resolve the artwork-dirty marker up front. Its presence means
    // a previous pause/cancel/crash left a mid-loop checkpoint's cover-art
    // thumbnails unrepaired (`rebuild_apple_artwork`'s doc comment explains
    // why `db.write()` on a parsed DB drops them). Checked again below, both
    // to keep the "nothing to sync" early return from skipping a pending
    // repair (the `should_rebuild_artwork(0, N, true) -> true` case) and to
    // feed `should_rebuild_artwork`'s OR-clause at the end of a real run.
    // Resolution failure (e.g. no resolvable config dir) is logged and
    // treated as "marker support unavailable this run" — never fails the
    // sync; it just falls back to the pre-Task-13 `changed > 0` gate.
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
        progress.log(format!("Loaded {} existing manifest entries", manifest.tracks.len()));
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

    // Source-change safeguard: if the manifest's last_source_root is set
    // AND differs from the current config.source, AND the manifest has tracks,
    // loudly confirm before letting the diff's Remove actions fire. This
    // catches the catastrophic case where the user typo'd --source or pointed
    // at a different library entirely — without this, every existing track
    // would be Removed (because it's "missing" from the wrong source root).
    let mut safeguard_force_no_delete = false;
    if !manifest.tracks.is_empty() {
        if let Some(last) = &manifest.last_source_root {
            if last != &config.source {
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
                    last.display(),
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
                    &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                )?;
                match outcome {
                    PromptOutcome::Retry => {
                        progress.log("Source-change safeguard: user chose Continue.".to_string());
                    }
                    PromptOutcome::Skip => {
                        progress.log(
                            "Source-change safeguard: applying with --no-delete for this run."
                                .to_string(),
                        );
                        safeguard_force_no_delete = true;
                    }
                    _ => {
                        return Err(anyhow!("source-change safeguard aborted"));
                    }
                }
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
        progress.review(summary_struct, config.no_delete || safeguard_force_no_delete);
        match decision_rx.recv() {
            Ok(Decision::Review(ReviewDecision::Apply { no_delete })) => {
                let no_delete = no_delete || safeguard_force_no_delete;
                let effective_remove = if no_delete { 0 } else { remove };
                let total_planned = add + modify + metadata_only + effective_remove;
                progress.summary(add, modify, metadata_only, remove, unchanged, total_planned);
                no_delete
            }
            Ok(Decision::Review(ReviewDecision::DryRun)) => {
                log_deferred_summary(progress, &deferred);
                progress.log("Dry run; nothing was written.");
                return Ok(RunOutcome::Completed);
            }
            Ok(Decision::Review(ReviewDecision::Quit)) => {
                progress.log("Aborted; nothing was written.");
                return Ok(RunOutcome::Completed);
            }
            Ok(Decision::Prompt { .. }) | Ok(Decision::Form { .. }) | Ok(Decision::Pause) => {
                // Unexpected at this stage (no try_with_prompt / wizard caller
                // wired yet, and Pause only makes sense once the apply loop
                // is running). Return loudly rather than silently swallowing
                // a stray decision.
                return Err(anyhow!("unexpected prompt/form/pause decision before any prompt was sent"));
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
        // Task 13: the diff itself has nothing to sync, but a previous
        // pause/cancel/crash left cover art unrepaired (the artwork-dirty
        // marker is still set). Fall through into the apply loop instead of
        // the early return above — the loop body will process zero actions,
        // but the post-loop `should_rebuild_artwork` gate sees
        // `marker_present == true` and repairs anyway.
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
    let sync_result: Result<RunOutcome> = (|| -> Result<RunOutcome> {
        // `identity` (FirewireGuid, ModelNumStr pair libgpod needs to sign
        // the iTunesDB iTunes will accept on read) was already resolved
        // above — ahead of the manifest load, to derive `serial` — and is
        // reused here rather than re-probing SCSI/USB a second time.
        // ModelNumStr is critical: without it libgpod's checksum_type
        // collapses to NONE and iTunes refuses the DB ("cannot read the
        // contents of the iPod"), even though the iPod's firmware would
        // still play the music.
        //
        // Still resolved BEFORE OwnedDb::open: ModelNumStr also picks the
        // SysInfoExtended template below, and libgpod reads that file
        // during open.
        progress.log(format!(
            "iPod identity: FirewireGuid={}, ModelNumStr={}",
            identity.firewire_guid, identity.model_num_str,
        ));
        // Provision the per-model SysInfoExtended so libgpod emits the
        // artwork ithmb formats the firmware reads (notably F1069).
        // Non-fatal: art is best-effort, never abort a sync over it.
        if let Err(e) = crate::ipod::sysinfo_provision::provision(Path::new(&mount), &identity) {
            progress.log(format!(
                "SysInfoExtended provisioning failed (art may not display): {e:#}"
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

        // Defensive backup of iTunesDB before we touch it. If a sync
        // crashes mid-write and corrupts the live DB, the user can
        // restore from `iTunesDB.classick-backup`. See
        // `crate::ipod::db::backup_itunesdb` for the why.
        if let Err(e) = crate::ipod::db::backup_itunesdb(Path::new(&mount)) {
            progress.log(format!(
                "Pre-sync DB backup failed: {e}; sync will proceed without a fresh backup."
            ));
        }

        // Reconcile DB with disk before the diff so we see a 1:1
        // baseline. Cleans up orphan .m4a files from previous crashed
        // syncs AND removes DB entries whose files were deleted out
        // from under us. Cheap (~1s for a 1,400-track library) and
        // eliminates a whole class of compounding corruption.
        let report = db.reconcile_with_disk(Path::new(&mount));
        if !report.is_clean() {
            progress.log(format!(
                "Pre-sync reconcile: removed {} orphan file(s), {} dangling DB ref(s); {} orphan deletion(s) failed.",
                report.orphans_deleted, report.dangling_removed, report.orphans_failed,
            ));
        }

        // Dispatch parallel transcode jobs for every Add/Modify action ahead
        // of the single committer loop below. Modify actions that will be
        // skipped under --no-delete (see the Modify arm) are excluded here —
        // dispatching one would mean nobody ever calls `take()` for it,
        // permanently leaking a window permit and an on-disk temp file for
        // the rest of the process. Add jobs are always dispatched (Add never
        // short-circuits before reaching the committer).
        let transcode_jobs: Vec<(usize, SourceEntry)> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, a)| match a {
                Action::Add(src) => Some((idx, src.clone())),
                Action::Modify(src, _old) if !effective_no_delete => Some((idx, src.clone())),
                _ => None,
            })
            .collect();
        let config_for_workers = config.clone();
        let refalac_for_workers = refalac_version.clone();
        let transcoder = OrderedTranscoder::start(
            transcode_jobs,
            crate::transcode_workers(),
            crate::PIPELINE_WINDOW,
            move |src: &SourceEntry| transcode_one(src, &config_for_workers, &refalac_for_workers),
        );

        let mut i = 0usize;
        let mut cancelled = false;
        let mut paused = false;
        // Task 8: actual on-iPod bytes written this run (transcoded/
        // passthrough file sizes, not source FLAC sizes). Accumulated
        // across the main loop below AND the end-of-run retry pass.
        let mut bytes_written: u64 = 0;
        // Task 13: per-track cover-art outcome tally for `ArtworkSummary`,
        // populated by `commit_pipelined` (Add/Modify) and the MetadataOnly
        // arm below, surfaced via `progress.note_artwork_summary` near the
        // end of this closure.
        let mut artwork_counts = ArtworkCounts::default();
        let mut ckpt = CheckpointClock::new(
            crate::CHECKPOINT_MAX_TRACKS,
            Duration::from_secs(crate::CHECKPOINT_MAX_SECONDS),
            Instant::now(),
        );
        for (idx, action) in actions.into_iter().enumerate() {
            // Non-blocking cancel/pause poll. The IPC stdin reader maps a
            // `{"type":"cancel"}` from the daemon to
            // Decision::Review(ReviewDecision::Quit) and pushes it onto
            // decision_rx. Without this check we'd only observe Quit at
            // the next try_with_prompt error-retry — i.e. never, if the
            // sync is healthy — and the daemon's bounded_kill would
            // TerminateProcess us mid-track, orphaning every file
            // already copied via itdb_cp_track_to_ipod. By bailing here
            // we fall through to the post-loop db.write() + manifest
            // save below, so all completed tracks are properly
            // registered in iTunesDB and no orphans are left behind.
            //
            // Decision::Pause is the same drain-then-stop shape as Cancel;
            // the only difference (surfaced in a later task) is the terminal
            // outcome reported to the caller.
            match decision_rx.try_recv() {
                Ok(Decision::Review(ReviewDecision::Quit)) => {
                    progress.log("Cancel requested — finalising completed tracks before stopping...");
                    cancelled = true;
                    break;
                }
                Ok(Decision::Pause) => {
                    progress.log("Pause requested — finalising in-flight tracks…");
                    paused = true;
                    break;
                }
                // Other decisions at this point are stray (no prompt is
                // in flight in the action loop body). Drop them; the
                // alternative — propagating them — has no consumer.
                Ok(_) => {}
                Err(_) => {}
            }
            match action {
                Action::Unchanged(_) => continue,
                Action::Remove(entry) => {
                    if effective_no_delete {
                        continue;
                    }
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("REMOVE {} (dbid {})", display_path(&entry.source_path), entry.ipod_dbid),
                    );
                    // Retry/Skip/Abort loop. Skip leaves the manifest entry
                    // intact: the track is still on the iPod (delete failed),
                    // so the manifest stays in sync with iPod state.
                    let removed = loop {
                        match crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
                            db.delete_track(entry.ipod_dbid)
                                .with_context(|| format!("delete dbid {}", entry.ipod_dbid))
                        }) {
                            Ok(()) => break true,
                            Err(e) => {
                                let msg = format!(
                                    "Failed to remove {} (dbid {}):\n  {e:#}\n\nChoose:",
                                    entry.source_path.display(),
                                    entry.ipod_dbid
                                );
                                let outcome = await_prompt(
                                    progress, decision_rx, msg,
                                    &["Retry", "Skip this track", "Abort"],
                                    &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                                )?;
                                match outcome {
                                    PromptOutcome::Retry => continue,
                                    PromptOutcome::Skip => {
                                        progress.log(format!(
                                            "Skipped Remove for dbid {} ({})",
                                            entry.ipod_dbid,
                                            entry.source_path.display()
                                        ));
                                        break false;
                                    }
                                    _ => return Err(e),
                                }
                            }
                        }
                    };
                    if removed {
                        manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
                    }
                    progress.track_done();
                }
                Action::Modify(src, old) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("MODIFY {}", display_path(&src.path)),
                    );

                    // Under --no-delete we can't replace the existing track
                    // without first removing it. Running only the Add half
                    // would push a second manifest entry for the same
                    // source_path while the original iPod track remains —
                    // a permanent duplicate the next Remove pass can't reap.
                    // Treat as Unchanged for this run; the user can re-run
                    // without --no-delete to pick it up.
                    if effective_no_delete {
                        progress.log(format!(
                            "Skipping MODIFY {} under --no-delete (would create duplicate)",
                            src.path.display()
                        ));
                        progress.track_done();
                        continue;
                    }

                    // First half: delete the old track. On failure, prompt;
                    // Skip here means we never touched the iPod, so the
                    // manifest entry stays as-is.
                    let deleted = loop {
                        match crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
                            db.delete_track(old.ipod_dbid)
                                .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))
                        }) {
                            Ok(()) => break true,
                            Err(e) => {
                                let msg = format!(
                                    "Failed to delete old version of {} (dbid {}) before re-adding:\n  {e:#}\n\nChoose:",
                                    src.path.display(),
                                    old.ipod_dbid
                                );
                                let outcome = await_prompt(
                                    progress, decision_rx, msg,
                                    &["Retry", "Skip this track", "Abort"],
                                    &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                                )?;
                                match outcome {
                                    PromptOutcome::Retry => continue,
                                    PromptOutcome::Skip => {
                                        progress.log(format!(
                                            "Skipped Modify (delete failed) for {}",
                                            src.path.display()
                                        ));
                                        break false;
                                    }
                                    _ => return Err(e),
                                }
                            }
                        }
                    };
                    if deleted {
                        manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid);
                    }

                    // Second half: commit the already-pipelined transcode.
                    // `take(idx)` MUST run whenever a job was dispatched for
                    // this index — and one was, since the dispatch filter
                    // above only excludes no-delete-skipped Modifies (this
                    // arm already returned in that case) — regardless of
                    // whether the delete succeeded. Otherwise the pipeline's
                    // window permit and the worker's temp file for this job
                    // leak for the rest of the process. `deleted` selects
                    // whether the drained result is actually committed
                    // (`true`) or discarded (`false`, delete failed + user
                    // skipped — iPod state is "in-between": old track gone,
                    // new not added; the next run's diff sees the missing
                    // iPod entry and re-adds naturally).
                    commit_pipelined(
                        &transcoder, idx, &db, &mut manifest, &src, deleted, progress, decision_rx,
                        &mut bytes_written, &mut artwork_counts,
                    )?;
                    progress.track_done();
                }
                Action::Add(src) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("ADD {}", display_path(&src.path)),
                    );
                    // Add always dispatches a job (see the filter above), so
                    // `take(idx)` always runs here.
                    commit_pipelined(
                        &transcoder, idx, &db, &mut manifest, &src, true, progress, decision_rx,
                        &mut bytes_written, &mut artwork_counts,
                    )?;
                    progress.track_done();
                }
                Action::MetadataOnly { source, entry } => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("METADATA {}", display_path(&source.path)),
                    );
                    // Whole metadata-update sequence is bundled into
                    // `do_metadata_only` so the retry loop has a single
                    // fallible call to wrap. Skip leaves manifest as-is.
                    let updated: Option<(ManifestEntry, ArtOutcome)> = loop {
                        match do_metadata_only(&db, &source, &entry, config) {
                            Ok((new_entry, art_outcome)) => break Some((new_entry, art_outcome)),
                            Err(e) => {
                                let msg = format!(
                                    "Failed to update metadata for {}:\n  {e:#}\n\nChoose:",
                                    source.path.display()
                                );
                                let outcome = await_prompt(
                                    progress, decision_rx, msg,
                                    &["Retry", "Skip this track", "Abort"],
                                    &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                                )?;
                                match outcome {
                                    PromptOutcome::Retry => continue,
                                    PromptOutcome::Skip => {
                                        // KNOWN LIMITATION (Bug 5): if
                                        // do_metadata_only failed AFTER
                                        // apply_tags mutated the in-memory
                                        // Itdb_Track (e.g. thumbnail update
                                        // failed), the new tag values are
                                        // already in the DB and will be
                                        // flushed by the run-end db.write().
                                        // The manifest entry stays at the
                                        // OLD state, so the next run will
                                        // see "Unchanged"/"MetadataOnly"
                                        // while iTunesDB tags are mid-state.
                                        // Surface this loudly so the user
                                        // knows to eject + re-run.
                                        progress.error(format!(
                                            "Skipped MetadataOnly for {} — partial tag write \
                                             may persist; recommended: eject the iPod and re-run \
                                             after the underlying issue is resolved.",
                                            source.path.display()
                                        ));
                                        break None;
                                    }
                                    _ => return Err(e),
                                }
                            }
                        }
                    };
                    if let Some((new_entry, art_outcome)) = updated {
                        // Refresh the manifest entry — same iPod identity,
                        // new source fingerprint + mtime/size.
                        manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
                        manifest.tracks.push(new_entry);
                        artwork_counts.record(art_outcome);
                    }
                    progress.track_done();
                }
            }

            // Periodic checkpoint: time-or-count (whichever fires first)
            // persists the iTunesDB + manifest. Without this, a daemon crash
            // / USB unplug / power loss mid-sync orphans every file copied
            // via itdb_cp_track_to_ipod since the only db.write() was
            // scheduled for the very end. With this, the orphan window is
            // bounded to CHECKPOINT_MAX_TRACKS tracks or CHECKPOINT_MAX_SECONDS
            // of wall-clock, whichever comes first.
            //
            // Unchanged / no-delete-Remove/Modify actions hit `continue`
            // before reaching here and so don't record a tick — they don't
            // mutate state worth persisting.
            if ckpt.record(Instant::now()) {
                progress.log(format!("Checkpoint: persisting state after {i} tracks…"));
                crate::try_with_prompt::retry_transient(&crate::RETRY_BACKOFF, || {
                    db.write().context("checkpoint: db.write")
                })?;
                manifest.last_source_root = Some(config.source.clone());
                manifest::save_atomic(&manifest_path, &manifest)
                    .context("checkpoint: manifest save")?;
                // Task 13: this `db.write()` is a `parsed`-DB write — libgpod
                // drops existing tracks' F1069 thumbnails on it (see the
                // `rebuild_apple_artwork` doc comment below). Mark the device
                // dirty so ANY exit from here on (this run's own end-of-run
                // rebuild, or — if we pause/cancel/crash before that runs —
                // the *next* run's `should_rebuild_artwork` gate) knows a
                // repair is owed. Best-effort; never fails the sync.
                if let Some(marker) = &marker_path {
                    touch_marker(marker);
                }
            }
        }

        // Stop the pipeline: no more `take()` calls are coming. Workers/feeder
        // are NOT joined here — on a normal finish they've already drained
        // (every dispatched job's index was taken above); on cancel/pause any
        // leftover in-flight transcodes block harmlessly on the bounded
        // channel and die with the process. Their temp files are cleaned up
        // by OS temp-dir housekeeping or the next reconcile pass.
        transcoder.stop();

        // Task 8: end-of-run deferred retry. Runs BEFORE the final commit
        // below so any tracks it adds land in the same db.write()/manifest
        // save as everything else — see `retry_deferred`'s doc comment.
        // Skipped on cancel/pause: the user asked to stop, and free space
        // hasn't meaningfully changed since the up-front fit pass queried it
        // moments before the loop started, so a retry would just repeat the
        // same "doesn't fit" verdict.
        let final_deferred: Vec<DeferredAlbum> = if !cancelled && !paused && !deferred.is_empty() {
            // Fresh free-space query: Removes already landed on disk during
            // the main loop, so this reflects any space they reclaimed — no
            // remove credit to fold in this time (unlike the up-front
            // budget, which had to project it). See `retry_deferred`'s doc
            // comment for why this I/O lives at the call site.
            let retry_storage = free_space::query(Path::new(&mount));
            let retry_budget = compute_budget(retry_storage, 0);
            retry_deferred(
                config,
                &refalac_version,
                &db,
                &mut manifest,
                &original_adds,
                deferred,
                retry_budget,
                album_tag_of,
                progress,
                decision_rx,
                &mut bytes_written,
                &mut artwork_counts,
            )?
        } else {
            deferred
        };
        if !final_deferred.is_empty() {
            progress.note_skipped_for_space(skipped_for_space(&final_deferred));
        }
        // Task 13: surface this run's art tally regardless of outcome
        // (completed/paused/cancelled) — whatever actually got processed
        // above is worth reporting either way.
        progress.note_artwork_summary(artwork_counts.to_summary());
        if bytes_written > 0 {
            progress.log(format!(
                "{} of audio written this run.",
                crate::ipc::format_bytes_human(bytes_written)
            ));
        }

        // Task 6: reconcile Classick-managed iTunesDB playlists. Runs AFTER
        // the track loop (every dbid a desired playlist might reference has
        // already landed in the in-memory DB, including this run's own new
        // Adds) and BEFORE the final `db.write()` below, so playlist
        // creates/updates/removes land in the SAME commit as the track
        // changes — no separate write, no window where tracks exist on the
        // device but playlists don't yet reflect them (or vice versa).
        //
        // Gated on `should_reconcile_playlists` (Task 8's `final_deferred`
        // gate, same rationale): a paused/cancelled run skips reconcile
        // entirely and picks it back up on the next full run, since desired
        // membership is re-derived fresh every time rather than diffed
        // incrementally.
        //
        // `desired_playlists` is `None` when the playlist store
        // couldn't be opened earlier this run (see where it's built,
        // above) — skip the reconcile entirely rather than pass `desired =
        // []`, which would read as "the user wants zero playlists" and
        // remove every previously-managed one over what's really just a
        // transient store-open failure.
        //
        // Warn-only end-to-end (spec §6): any failure inside
        // `reconcile_playlists_step` — including the final
        // `ManagedPlaylists::save` — is logged and swallowed here rather
        // than bubbled, so a playlist-only problem never sinks an
        // otherwise-successful sync. The final `db.write()` below always
        // runs regardless.
        if should_reconcile_playlists(cancelled, paused) {
            if let Some(desired) = &desired_playlists {
                if let Err(e) = reconcile_playlists_step(&db, desired, &manifest, &serial, progress) {
                    tracing::warn!("playlist reconcile failed: {e:#}");
                    progress.log(format!("playlist update failed: {e:#} — will retry next sync"));
                }
            }
        }

        // 6. Final commit. NEITHER is persisted unless we got this far.
        // Also runs after a `cancelled`/`paused` break above so completed
        // tracks get a coherent final flush (no orphan pile-up under
        // iPod_Control\Music). Idempotent for the post-checkpoint residual:
        // if the last checkpoint just fired, this re-writes the same state.
        progress.log("Writing iPod DB...");
        db.write()?;
        // Task 6: mirror the host playlist store + this device's
        // subscriptions.json onto the iPod, now that the DB write above
        // confirms the reconciled playlists (and the tracks they
        // reference) are actually persisted. Best-effort — see
        // `device_playlists::mirror_to_ipod`'s doc comment; a mirror
        // failure never fails an otherwise-successful sync.
        if let (Some(root), Some(subs)) = (&playlist_store_root, &device_subscriptions_file) {
            crate::ipod::device_playlists::mirror_to_ipod(Path::new(&mount), root, subs);
        }
        progress.log("Writing manifest...");
        // Stamp the current source root onto the manifest so the next run's
        // source-change safeguard has something to compare against.
        manifest.last_source_root = Some(config.source.clone());
        manifest::save_atomic(&manifest_path, &manifest)?;

        if paused {
            progress.log("Sync paused. Completed tracks were saved. Eject the iPod before unplugging.");
        } else if cancelled {
            progress.log("Sync cancelled. Completed tracks were saved. Eject the iPod before unplugging.");
        } else {
            progress.log("Done. Eject the iPod before unplugging.");
        }
        if paused {
            Ok(RunOutcome::Paused)
        } else {
            Ok(RunOutcome::Completed)
        }
    })();

    if let Err(e) = &sync_result {
        // While the TUI is up, push the recovery block into log_tail so users
        // see it inline. The same text is ALSO attached to the bubbled error
        // (see below) so it survives the TUI teardown that wipes log_tail.
        progress.error(format!("Sync failed: {e:#}"));
        for line in RECOVERY_HINT_LINES {
            progress.error((*line).to_string());
        }
    }

    // Incremental-sync artwork repair. When a sync CHANGED the DB but KEPT
    // existing tracks (unchanged > 0), libgpod's final `itdb_write` drops the
    // existing tracks' Apple cover-art thumbnails — it only writes thumbnails
    // re-set in memory this session, and existing tracks were loaded as
    // references (see `rebuild_apple_artwork`, verified on-device). So rebuild
    // the ArtworkDB fresh from source art for the whole library, and re-embed
    // the Rockbox `.m4a` tags/art for those tracks when `rockbox_compat` is on.
    // On a fresh/full sync it's a redundant (idempotent, guarded) re-thumbnail
    // — cheap vs. the sync itself, and robust regardless of libgpod's exact
    // preservation behavior. Skipped on pause and rebuild-manifest. (No audio
    // re-copy.)
    //
    // Gate is `should_rebuild_artwork` (Task 13): `changed > 0` alone (not
    // `&& unchanged > 0` — see that fn's doc comment for the 05d15ce
    // bulk-retag case it must keep covering) OR the artwork-dirty marker is
    // present, i.e. a previous pause/cancel/crash left a mid-loop
    // checkpoint's thumbnails unrepaired and this run must repair them even
    // if its own diff is entirely Unchanged.
    let changed = add + modify + metadata_only + remove;
    if matches!(sync_result, Ok(RunOutcome::Completed)) && !config.rebuild_manifest {
        let marker_present = marker_path.as_ref().is_some_and(|p| p.exists());
        if should_rebuild_artwork(changed, unchanged, marker_present) {
            progress.log("Refreshing artwork for existing tracks (no re-copy)…".to_string());
            let mut refreshed: Vec<(u64, crate::ipod::db::Tags, Option<Vec<u8>>)> = Vec::new();
            for entry in manifest
                .tracks
                .iter()
                .filter(|e| e.source_known && !e.ipod_relpath.is_empty())
            {
                let device_file = Path::new(&mount)
                    .join(entry.ipod_relpath.replace('\\', std::path::MAIN_SEPARATOR_STR));
                if !device_file.exists() || !entry.source_path.exists() {
                    continue;
                }
                match source_tags_and_art(&entry.source_path, &config.ffmpeg) {
                    Ok((tags, art)) => {
                        if config.rockbox_compat && entry.encoder != "passthrough" {
                            let _ = crate::artwork::embed_track_metadata(
                                &device_file,
                                &tags,
                                art.as_deref(),
                            );
                        }
                        refreshed.push((entry.ipod_dbid, tags, art));
                    }
                    Err(e) => {
                        tracing::warn!("art refresh: {} skipped: {e:#}", entry.source_path.display())
                    }
                }
            }
            match rebuild_apple_artwork(Path::new(&mount), &refreshed) {
                Ok(()) => {
                    progress.log("Artwork refreshed.".to_string());
                    // Repair landed — clear the marker so a future run's gate
                    // doesn't force a rebuild it no longer needs.
                    if let Some(marker) = &marker_path {
                        clear_marker(marker);
                    }
                }
                Err(e) => {
                    progress.log(format!("Apple artwork not refreshed ({e:#})."));
                    // Repair still owed — leave the marker set so the NEXT
                    // run's gate retries it via `marker_present`, per the
                    // "every exit path repairs before or at the next
                    // session" invariant.
                }
            }
        } else if let Some(marker) = &marker_path {
            // Nothing to repair this run (no DB-changing sync, no
            // outstanding marker — `marker_present` was already false or
            // `should_rebuild_artwork` would have been true). Harmless no-op
            // when absent; defensively clears a stale marker if one somehow
            // slipped through.
            clear_marker(marker);
        }
    }

    // save_config is independent of the sync closure: it only runs on success
    // and a failure here is a warning (config-file write), not a reason to
    // print the orphan-files recovery block.
    if sync_result.is_ok() && config.save_config {
        match config_file::default_path()
            .and_then(|p| config_file::save(&p, &config.to_persisted()).map(|()| p))
        {
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
    db.write().context("write iPod DB after wipe")?;
    drop(db);

    let manifest_path = device_state::migrate_legacy_manifest(&config.manifest_path, &serial)?;
    let mut manifest = Manifest::empty();
    manifest.ipod_serial = Some(serial);
    manifest::save_atomic(&manifest_path, &manifest).context("save reset manifest after wipe")?;

    progress.log("Library erased; syncing the current selection…".to_string());
    run(config, progress, decision_rx)
}

/// Whether `replace_library`'s confirmation prompt should be skipped —
/// true iff `--apply` was passed. Pure so it's unit-testable without a
/// Progress/decision-channel harness.
pub(crate) fn should_skip_replace_confirmation(apply: bool) -> bool {
    apply
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
pub(crate) fn compute_budget(storage: Option<free_space::StorageInfo>, remove_credit: u64) -> Option<u64> {
    let storage = storage?;
    let reserve = crate::fit::reserve_bytes(storage.total_bytes);
    Some(storage.free_bytes.saturating_add(remove_credit).saturating_sub(reserve))
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
    progress.log(format!("Retrying {total} previously-deferred track(s) that now fit…"));

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
        progress.track_start(idx + 1, total, format!("ADD {} (retry)", display_path(&src.path)));
        commit_pipelined(
            &transcoder, idx, db, manifest, &src, true, progress, decision_rx, bytes_written,
            artwork_counts,
        )?;
        progress.track_done();
    }
    transcoder.stop();

    Ok(retry_outcome.deferred)
}

/// Run the full MetadataOnly path for one entry: probe + tag-extract +
/// optional embedded-art extract + libgpod update + recompute source
/// fingerprint. Returns the freshly-built ManifestEntry to push, plus this
/// track's `ArtOutcome` for the run's `ArtworkSummary` (Task 13).
///
/// Bundled into one fallible call so the per-track retry/skip loop in `run`
/// has a single closure to retry — anything failing inside re-runs the lot.
///
/// Art extraction failure is deliberately NON-fatal to the metadata update
/// (mirrors `transcode_one`'s treatment): falling back to no art and
/// warn-logging the source path + reason lets the tag update still land,
/// rather than aborting the whole MetadataOnly action — and lets the
/// failure feed `ArtworkSummary.failed_sources` rather than only surfacing
/// as a generic "Failed to update metadata" retry/skip/abort prompt.
pub(crate) fn do_metadata_only(
    db: &OwnedDb,
    source: &SourceEntry,
    entry: &ManifestEntry,
    config: &Config,
) -> Result<(ManifestEntry, ArtOutcome)> {
    let probe = transcode::probe(&source.path, &config.ffmpeg)
        .with_context(|| format!("probe {}", source.path.display()))?;
    let tags = tags_from_probe(&probe);
    let has_art = has_embedded_art(&probe);
    let art: Option<Vec<u8>> = if has_art {
        let art_path = transcode::temp_art_path();
        let extracted = transcode::extract_cover_art(&source.path, &art_path, &config.ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        match extracted {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("art extract failed for {}: {e:#}", source.path.display());
                None
            }
        }
    } else {
        None
    };
    let art_outcome = ArtOutcome::from_probe(has_art, art.is_some());
    db.update_track_metadata(entry.ipod_dbid, &tags, art.as_deref())
        .with_context(|| format!(
            "update_track_metadata dbid {} for {}",
            entry.ipod_dbid,
            source.path.display()
        ))?;
    let new_file_fp = source::fingerprint(&source.path)
        .with_context(|| format!("fingerprint {}", source.path.display()))?;
    Ok((
        ManifestEntry {
            source_path: source.path.clone(),
            source_mtime: source.mtime,
            source_size: source.size,
            source_fingerprint: new_file_fp,
            ipod_dbid: entry.ipod_dbid,
            ipod_relpath: entry.ipod_relpath.clone(),
            source_known: true,
            audio_fingerprint: entry.audio_fingerprint.clone(),
            // MetadataOnly preserves the iPod-side file body verbatim, so the
            // encoder identity is unchanged. Copy from the existing entry.
            encoder: entry.encoder.clone(),
            encoder_version: entry.encoder_version.clone(),
            source_format: entry.source_format.clone(),
        },
        art_outcome,
    ))
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
                    tracing::warn!("art normalize failed for {}: {e:#}; using raw bytes", src.path.display());
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
                progress.error(format!("Transcode failed for {}: {e:#}", src.path.display()));
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
        &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
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
        (Some(parent), Some(file)) => format!(
            "{}\\{}",
            parent.to_string_lossy(),
            file.to_string_lossy()
        ),
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
    let probe = transcode::probe(source, ffmpeg)
        .with_context(|| format!("probe {}", source.display()))?;
    let tags = tags_from_probe(&probe);
    let art: Option<Vec<u8>> = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        let raw = transcode::extract_cover_art(source, &art_path, ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        raw.ok().and_then(|b| crate::artwork::normalize(&b).ok().or(Some(b)))
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
    let manifest_path = device_state::migrate_legacy_manifest(&config.manifest_path, &serial)?;
    let manifest = manifest::load_or_default(&manifest_path)?;
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
        let device_file = Path::new(&mount)
            .join(entry.ipod_relpath.replace('\\', std::path::MAIN_SEPARATOR_STR));
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
    db.write()?;
    Ok(())
}

pub(crate) fn build_rebuild_manifest(db: &OwnedDb, serial: &str) -> Manifest {
    build_rebuild_manifest_from_handles(db.list_tracks_for_rebuild(), serial)
}

/// Pure core of [`build_rebuild_manifest`], split out so it's unit-testable
/// without a real libgpod-backed `OwnedDb`.
pub(crate) fn build_rebuild_manifest_from_handles(handles: Vec<TrackHandle>, serial: &str) -> Manifest {
    let tracks = handles.into_iter().map(|h| ManifestEntry {
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
    }).collect();
    // last_source_root is intentionally None: the iPod's DB doesn't carry
    // the original source library root. The next normal sync's
    // manifest::save_atomic populates it from config.source.
    Manifest { version: 1, ipod_serial: Some(serial.to_string()), last_source_root: None, tracks }
}

// -- Task 6: playlist reconcile helpers --------------------------------

/// Whether the end-of-run playlist reconcile (`reconcile_playlists_step`)
/// should run at all this pass — mirrors the adjacent `final_deferred` gate
/// (Task 8) for the same reason: the user asked to stop, so this run's
/// device state is intentionally incomplete, and reconcile compares against
/// dbids for tracks that may not have finished landing. Skipping here is
/// safe because reconcile is idempotent and re-derives `desired` fresh from
/// the manifest/subscriptions every run — a paused or cancelled run simply
/// leaves the on-device playlists as they were until the next full run.
/// `pub(crate)` so it's unit-testable without a real `OwnedDb`.
pub(crate) fn should_reconcile_playlists(cancelled: bool, paused: bool) -> bool {
    !cancelled && !paused
}

/// Core of Task 6's end-of-run playlist reconcile: joins `desired`'s source
/// paths to this run's dbids via `manifest`, then calls
/// `device_playlists::reconcile`. `desired` is `(slug, display name,
/// resolved source paths)` — `slug` is threaded through as the
/// managed-identity join key (Fix 2: `reconcile` used to key on display
/// name alone, which collapses two distinct playlists that happen to share
/// a name via `PlaylistStore::unique_slug`'s `-2` disambiguation). Extracted
/// from `run` so every fallible step in here — the dbid join is infallible,
/// but the `reconcile` call (which covers `ensure_managed_playlist`/removal
/// calls and the final `ManagedPlaylists::save`) is not — surfaces through a
/// single `Result` that the caller handles warn-only (see spec §6: playlist
/// problems are fail-visible but must NEVER fail the sync, same rationale as
/// `mirror_to_ipod`'s doc comment). This function itself still returns
/// `Err` on failure; it's the CALLER's job to catch it rather than bubble
/// it into the final `db.write()`.
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
    let root = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve config dir"))?
        .join(crate::PROJECT_DIR);
    reconcile_playlists_step_in(db, desired, manifest, &root, serial, progress)
}

/// Test/override variant of [`reconcile_playlists_step`]: reconciles against
/// `root/devices/<serial>/managed_playlists.json` (via
/// `device_playlists::reconcile_in`) instead of the real config dir.
pub fn reconcile_playlists_step_in(
    db: &OwnedDb,
    desired: &[(String, String, Vec<PathBuf>)],
    manifest: &Manifest,
    root: &Path,
    serial: &str,
    progress: &Progress,
) -> Result<()> {
    let dbid_by_source_path: std::collections::HashMap<&Path, u64> = manifest
        .tracks
        .iter()
        .map(|e| (e.source_path.as_path(), e.ipod_dbid))
        .collect();
    let desired: Vec<(String, String, Vec<u64>)> = desired
        .iter()
        .map(|(slug, name, paths)| {
            let dbids: Vec<u64> = paths
                .iter()
                .filter_map(|p| dbid_by_source_path.get(p.as_path()).copied())
                .collect();
            (slug.clone(), name.clone(), dbids)
        })
        .collect();
    let stats = crate::ipod::device_playlists::reconcile_in(db, &desired, root, serial)
        .context("reconcile Classick-managed iTunesDB playlists")?;
    if stats.created + stats.updated + stats.removed > 0 {
        progress.log(format!(
            "playlists: {} created, {} updated, {} removed",
            stats.created, stats.updated, stats.removed
        ));
    }
    Ok(())
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

/// True if `manifest.ipod_serial` names a specific device (i.e. is `Some`)
/// that differs from the currently-connected, already-sanitized `serial`.
/// A `None` `ipod_serial` — a fresh manifest, or a legacy manifest that
/// predates this field — is NOT foreign: the sync adopts the device rather
/// than refusing to proceed.
pub(crate) fn manifest_is_foreign(manifest: &Manifest, serial: &str) -> bool {
    matches!(&manifest.ipod_serial, Some(s) if s != serial)
}

/// Task 13: whether the end-of-run Apple ArtworkDB rebuild (`rebuild_apple_artwork`)
/// should run.
///
/// `changed > 0` alone (no `unchanged` requirement) is the invariant fixed by
/// 05d15ce: a bulk retag of the WHOLE library is all-MetadataOnly with
/// `unchanged == 0`, yet `itdb_write` still drops existing tracks' F1069
/// thumbnails, so it still needs the rebuild. `unchanged` is kept as a
/// parameter (unused in the boolean below) because the truth table this is
/// tested against is specified in terms of it, and it's cheap to keep for
/// future logging/observability even though it doesn't gate the decision.
///
/// `marker_present` ORs in the case a previous pause/cancel/crash left a
/// mid-loop checkpoint's thumbnails unrepaired (see the artwork-dirty marker
/// lifecycle in `run`): this run's OWN diff can be entirely Unchanged
/// (`changed == 0`) and it must still repair, because the outstanding damage
/// predates this run's diff.
pub(crate) fn should_rebuild_artwork(changed: usize, unchanged: usize, marker_present: bool) -> bool {
    let _ = unchanged; // see doc comment: intentionally not part of the gate
    changed > 0 || marker_present
}

/// Best-effort creation of the artwork-dirty marker at `path` (empty file),
/// if it doesn't already exist. Called whenever a mid-loop checkpoint's
/// `db.write()` runs, so a pause/cancel/crash between here and the next
/// successful `rebuild_apple_artwork` leaves a durable flag that forces the
/// *next* run's `should_rebuild_artwork` gate open even if that run's own
/// diff is all-Unchanged. I/O failures are logged and swallowed — the marker
/// is a best-effort repair hint, never a reason to fail a sync.
pub(crate) fn touch_marker(path: &Path) {
    if path.exists() {
        return;
    }
    if let Err(e) = std::fs::write(path, b"") {
        tracing::warn!("artwork-dirty marker: failed to create {}: {e:#}", path.display());
    }
}

/// Best-effort removal of the artwork-dirty marker at `path`, once
/// `rebuild_apple_artwork` has actually repaired the device (or nothing
/// needed repairing this run). I/O failures are logged and swallowed for the
/// same reason as `touch_marker`.
pub(crate) fn clear_marker(path: &Path) {
    if !path.exists() {
        return;
    }
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!("artwork-dirty marker: failed to remove {}: {e:#}", path.display());
    }
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
        crate::device_config::DeviceSettings { rockbox_compat, ..crate::device_config::DeviceSettings::default() }
    }

    /// The CLI `--rockbox-compat` flag force-enables for one run regardless
    /// of what the device's own settings.json says.
    #[test]
    fn effective_rockbox_cli_flag_forces_on() {
        assert!(effective_rockbox(true, &device_settings_with_rockbox(false)));
        assert!(effective_rockbox(true, &device_settings_with_rockbox(true)));
    }

    /// Without the CLI flag, the device's own setting decides.
    #[test]
    fn effective_rockbox_without_cli_flag_follows_device_setting() {
        assert!(!effective_rockbox(false, &device_settings_with_rockbox(false)));
        assert!(effective_rockbox(false, &device_settings_with_rockbox(true)));
    }

    fn guard_test_entry(path: &str) -> SourceEntry {
        SourceEntry { path: std::path::PathBuf::from(path), mtime: 1, size: 10 }
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
        let mut index = crate::library_index::LibraryIndex::empty(std::path::PathBuf::from("/m/music"));
        let sel = crate::selection::Selection {
            version: 1,
            mode: crate::selection::SelectionMode::Include,
            rules: vec![crate::selection::SelectionRule::Artist { name: "Nobody".into() }],
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
        assert!(kept.is_empty(), "selection filtering everything out must still be allowed");
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
        std::fs::write(tmp.join("track.flac"), b"not really flac, walk() only checks the extension")
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

    // -- manifest_is_foreign truth table --------------------------------

    #[test]
    fn manifest_is_foreign_true_when_serial_mismatches() {
        let mut m = Manifest::empty();
        m.ipod_serial = Some("AAA111".to_string());
        assert!(manifest_is_foreign(&m, "BBB222"));
    }

    #[test]
    fn manifest_is_foreign_false_when_serial_matches() {
        let mut m = Manifest::empty();
        m.ipod_serial = Some("AAA111".to_string());
        assert!(!manifest_is_foreign(&m, "AAA111"));
    }

    #[test]
    fn manifest_is_foreign_false_when_serial_is_none() {
        // Covers both a brand-new manifest and a legacy (pre-Task-2)
        // manifest whose `ipod_serial` field predates this feature and
        // deserializes as None via #[serde(default)] — both cases adopt
        // the connected device rather than being treated as foreign.
        let m = Manifest::empty();
        assert!(!manifest_is_foreign(&m, "AAA111"));
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
        assert!(!m.tracks[0].source_known, "rebuilt entries have no known source");
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
        let storage = free_space::StorageInfo { total_bytes: 10 * GB, free_bytes: 0 };
        assert_eq!(compute_budget(Some(storage), 0), Some(0));
    }

    #[test]
    fn compute_budget_normal_case_is_free_plus_credit_minus_reserve() {
        let storage = free_space::StorageInfo { total_bytes: 100 * GB, free_bytes: 10 * GB };
        let remove_credit = 2 * GB;
        let reserve = crate::fit::reserve_bytes(storage.total_bytes); // 2% of 100GB = 2GB
        let expected = 10 * GB + remove_credit - reserve;
        assert_eq!(compute_budget(Some(storage), remove_credit), Some(expected));
    }

    #[test]
    fn compute_budget_zero_credit_is_free_minus_reserve() {
        let storage = free_space::StorageInfo { total_bytes: 100 * GB, free_bytes: 50 * GB };
        let reserve = crate::fit::reserve_bytes(storage.total_bytes);
        assert_eq!(compute_budget(Some(storage), 0), Some(50 * GB - reserve));
    }

    // -- Task 8: deferred_add_actions ------------------------------------

    fn src_entry(path: &str, size: u64) -> SourceEntry {
        SourceEntry { path: std::path::PathBuf::from(path), mtime: 1_700_000_000, size }
    }

    fn deferred_album(key: &str, tracks: usize, bytes: u64) -> DeferredAlbum {
        DeferredAlbum { key: key.to_string(), tracks, bytes }
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
        assert_eq!(actions.len(), 2, "only the tagged pair belongs to the deferred album");
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

    // -- Task 6: should_reconcile_playlists truth table --------------------

    #[test]
    fn should_reconcile_playlists_true_on_clean_completion() {
        assert!(should_reconcile_playlists(false, false));
    }

    #[test]
    fn should_reconcile_playlists_false_when_cancelled() {
        assert!(!should_reconcile_playlists(true, false));
    }

    #[test]
    fn should_reconcile_playlists_false_when_paused() {
        assert!(!should_reconcile_playlists(false, true));
    }

    #[test]
    fn should_reconcile_playlists_false_when_both_cancelled_and_paused() {
        // Not a state the real loop can produce, but the gate must still
        // fail closed rather than panicking or picking one over the other.
        assert!(!should_reconcile_playlists(true, true));
    }

    // -- Task 13: should_rebuild_artwork truth table ----------------------

    #[test]
    fn should_rebuild_artwork_false_when_nothing_changed_and_no_marker() {
        assert!(!should_rebuild_artwork(0, 0, false));
    }

    #[test]
    fn should_rebuild_artwork_true_when_changed_and_unchanged_both_positive() {
        assert!(should_rebuild_artwork(3, 5, false));
    }

    #[test]
    fn should_rebuild_artwork_true_when_changed_alone_is_positive() {
        // Pins the 05d15ce fix: a bulk retag of the WHOLE library is all
        // MetadataOnly with unchanged == 0 (every track got touched), yet
        // itdb_write still drops existing tracks' F1069 thumbnails, so the
        // rebuild must still run. Regressing to `changed>0 && unchanged>0`
        // would silently reintroduce that bug for exactly this case.
        assert!(should_rebuild_artwork(4, 0, false));
    }

    #[test]
    fn should_rebuild_artwork_true_when_marker_present_despite_all_unchanged_diff() {
        // Pause-then-noop-resume repair case: a previous run's mid-loop
        // checkpoint left thumbnails unrepaired, then the resumed run's own
        // diff is entirely Unchanged (changed == 0). The marker alone must
        // still force the rebuild.
        assert!(should_rebuild_artwork(0, 7, true));
    }

    #[test]
    fn should_rebuild_artwork_false_when_marker_absent_and_nothing_changed() {
        assert!(!should_rebuild_artwork(0, 0, false));
    }

    // -- Task 13: artwork-dirty marker create/delete round-trip -----------

    fn marker_tempdir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("apply_loop-marker-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn touch_marker_creates_empty_file_once() {
        let root = marker_tempdir();
        let path = device_state::artwork_dirty_marker_path_in(&root, "SER1").unwrap();
        assert!(!path.exists());
        touch_marker(&path);
        assert!(path.exists());
        // Idempotent: a second touch on an existing marker must not error or
        // truncate/rewrite in a way that fails.
        touch_marker(&path);
        assert!(path.exists());
    }

    #[test]
    fn clear_marker_removes_existing_marker() {
        let root = marker_tempdir();
        let path = device_state::artwork_dirty_marker_path_in(&root, "SER2").unwrap();
        touch_marker(&path);
        assert!(path.exists());
        clear_marker(&path);
        assert!(!path.exists());
    }

    #[test]
    fn clear_marker_on_absent_marker_is_a_harmless_no_op() {
        let root = marker_tempdir();
        let path = device_state::artwork_dirty_marker_path_in(&root, "SER3").unwrap();
        assert!(!path.exists());
        clear_marker(&path); // must not panic or create the file
        assert!(!path.exists());
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
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a"), &dev).unwrap();

        // The per-track backfill step exactly as production runs it
        // (apply_loop's backfill paths): probe source tags → normalize art
        // → embed into the on-device file.
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac")); // existing fixture: tags + embedded PNG art
        let before = std::fs::metadata(&dev).unwrap().len();
        let (tags, art) =
            super::source_tags_and_art(src, &std::path::PathBuf::from("ffmpeg")).unwrap();
        crate::artwork::embed_track_metadata(&dev, &tags, art.as_deref()).unwrap();
        let after = std::fs::metadata(&dev).unwrap().len();
        assert!(after >= before, "embedding should not shrink the file");
        // The returned tags + normalized art are what the caller re-applies to
        // the Apple ithmb (the art-break fix); a tagged source must yield them.
        assert!(tags.title.is_some() || tags.artist.is_some(), "tags extracted from source");
        assert!(art.is_some(), "normalized cover art returned for a track with embedded art");

        use lofty::file::TaggedFileExt;
        let tag = lofty::read_from_path(&dev).unwrap();
        assert!(tag.primary_tag().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod split_tests {
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
}
