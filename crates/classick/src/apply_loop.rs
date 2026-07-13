//! Per-Action match arms (Add / Modify / MetadataOnly / Remove / Unchanged)
//! plus their supporting helpers — `transcode_one`, `commit_transcoded`,
//! `do_metadata_only`, `entry_from`, `build_rebuild_manifest`, `count_actions`.
//! Also owns the top-level `run` function that ties preflight, diff, review,
//! and apply together.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::checkpoint::CheckpointClock;
use crate::cli::EncoderChoice;
use crate::config::Config;
use crate::config_file;
use crate::ipod::db::{OwnedDb, TrackHandle};
use crate::ipod::device;
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
    let sources = preflight::walk_source(config, progress, decision_rx)?;

    // 3. Load (or rebuild) manifest.
    let mut manifest = if config.rebuild_manifest {
        let db = OwnedDb::open(Path::new(&mount))?;
        let rebuilt = build_rebuild_manifest(&db);
        // Save eagerly so a crash after this point doesn't lose the rebuild.
        manifest::save_atomic(&config.manifest_path, &rebuilt)?;
        rebuilt
    } else {
        manifest::load_or_default(&config.manifest_path)?
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
    let (add, modify, metadata_only, remove, unchanged) = count_actions(&actions);

    // Progress was started in main() and is borrowed here. Send the header now
    // that we know what to display.
    progress.header(
        config.source.display().to_string(),
        mount.clone(),
        config.manifest_path.display().to_string(),
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
    {
        progress.log("Nothing to do.");
        return Ok(RunOutcome::Completed);
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
        // Resolve the (FirewireGuid, ModelNumStr) pair libgpod needs
        // to sign the iTunesDB iTunes will accept on read. Source
        // preference: on-disk SysInfo > SCSI INQUIRY VPD pages
        // (authoritative) > USB PID+capacity heuristic. ModelNumStr
        // is critical — without it libgpod's checksum_type collapses
        // to NONE and iTunes refuses the DB ("cannot read the
        // contents of the iPod"), even though the iPod's firmware
        // would still play the music.
        //
        // Resolved BEFORE OwnedDb::open: ModelNumStr also picks the
        // SysInfoExtended template below, and libgpod reads that file
        // during open.
        let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
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
        let db = OwnedDb::open(Path::new(&mount))?;
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
                    commit_pipelined(&transcoder, idx, &db, &mut manifest, &src, deleted, progress, decision_rx)?;
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
                    commit_pipelined(&transcoder, idx, &db, &mut manifest, &src, true, progress, decision_rx)?;
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
                    let updated: Option<ManifestEntry> = loop {
                        match do_metadata_only(&db, &source, &entry, config) {
                            Ok(new_entry) => break Some(new_entry),
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
                    if let Some(new_entry) = updated {
                        // Refresh the manifest entry — same iPod identity,
                        // new source fingerprint + mtime/size.
                        manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
                        manifest.tracks.push(new_entry);
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
                manifest::save_atomic(&config.manifest_path, &manifest)
                    .context("checkpoint: manifest save")?;
            }
        }

        // Stop the pipeline: no more `take()` calls are coming. Workers/feeder
        // are NOT joined here — on a normal finish they've already drained
        // (every dispatched job's index was taken above); on cancel/pause any
        // leftover in-flight transcodes block harmlessly on the bounded
        // channel and die with the process. Their temp files are cleaned up
        // by OS temp-dir housekeeping or the next reconcile pass.
        transcoder.stop();

        // 6. Final commit. NEITHER is persisted unless we got this far.
        // Also runs after a `cancelled`/`paused` break above so completed
        // tracks get a coherent final flush (no orphan pile-up under
        // iPod_Control\Music). Idempotent for the post-checkpoint residual:
        // if the last checkpoint just fired, this re-writes the same state.
        progress.log("Writing iPod DB...");
        db.write()?;
        progress.log("Writing manifest...");
        // Stamp the current source root onto the manifest so the next run's
        // source-change safeguard has something to compare against.
        manifest.last_source_root = Some(config.source.clone());
        manifest::save_atomic(&config.manifest_path, &manifest)?;

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
    // A fresh/full sync (unchanged == 0) needs none of this — every track got
    // fresh thumbnails as it was added. Skipped on pause and rebuild-manifest.
    // (Cost: re-reads + re-thumbnails the whole library — no audio re-copy.)
    let changed = add + modify + metadata_only + remove;
    if matches!(sync_result, Ok(RunOutcome::Completed))
        && !config.rebuild_manifest
        && changed > 0
        && unchanged > 0
    {
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
            Ok(()) => progress.log("Artwork refreshed.".to_string()),
            Err(e) => progress.log(format!("Apple artwork not refreshed ({e:#}).")),
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

/// Recovery instructions shown when the apply loop fails mid-sync. Kept as a
/// const so both the in-TUI `progress.error` log and the bubbled error message
/// stay in lock-step.
const RECOVERY_HINT_LINES: &[&str] = &[
    "The iPod may now contain orphan track files (added but not in the iTunesDB),",
    "and the manifest has NOT been updated.",
    "To recover: re-run with --rebuild-manifest, which will read the iPod's",
    "current DB and create a fresh manifest. Then run normally.",
];

/// Run the full MetadataOnly path for one entry: probe + tag-extract +
/// optional embedded-art extract + libgpod update + recompute source
/// fingerprint. Returns the freshly-built ManifestEntry to push.
///
/// Bundled into one fallible call so the per-track retry/skip loop in `run`
/// has a single closure to retry — anything failing inside re-runs the lot.
pub(crate) fn do_metadata_only(
    db: &OwnedDb,
    source: &SourceEntry,
    entry: &ManifestEntry,
    config: &Config,
) -> Result<ManifestEntry> {
    let probe = transcode::probe(&source.path, &config.ffmpeg)
        .with_context(|| format!("probe {}", source.path.display()))?;
    let tags = tags_from_probe(&probe);
    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&source.path, &art_path, &config.ffmpeg)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };
    db.update_track_metadata(entry.ipod_dbid, &tags, art.as_deref())
        .with_context(|| format!(
            "update_track_metadata dbid {} for {}",
            entry.ipod_dbid,
            source.path.display()
        ))?;
    let new_file_fp = source::fingerprint(&source.path)
        .with_context(|| format!("fingerprint {}", source.path.display()))?;
    Ok(ManifestEntry {
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
    })
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
    let art: Option<Vec<u8>> = if has_embedded_art(&probe) {
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
fn commit_pipelined(
    transcoder: &OrderedTranscoder<Transcoded>,
    idx: usize,
    db: &OwnedDb,
    manifest: &mut Manifest,
    src: &SourceEntry,
    commit: bool,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
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
    loop {
        match commit_transcoded(db, manifest, src, &t) {
            Ok(()) => return Ok(()),
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

/// Embed a source's tags + normalized art into an on-device `.m4a` (Rockbox
/// side). Returns `(tags, normalized_art, new_size)`. Pure file I/O; no libgpod.
pub(crate) fn backfill_one_file(
    device_file: &Path,
    source: &Path,
    ffmpeg: &Path,
) -> Result<(crate::ipod::db::Tags, Option<Vec<u8>>, u64)> {
    let (tags, art) = source_tags_and_art(source, ffmpeg)?;
    crate::artwork::embed_track_metadata(device_file, &tags, art.as_deref())
        .with_context(|| format!("embed into {}", device_file.display()))?;
    let size = std::fs::metadata(device_file)?.len();
    Ok((tags, art, size))
}

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
    let manifest = manifest::load_or_default(&config.manifest_path)?;
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

pub(crate) fn build_rebuild_manifest(db: &OwnedDb) -> Manifest {
    let handles = db.list_tracks_for_rebuild();
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
    Manifest { version: 1, ipod_serial: None, last_source_root: None, tracks }
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
}

#[cfg(test)]
mod backfill_tests {
    use super::backfill_one_file;

    #[test]
    fn backfill_embeds_into_existing_device_file() {
        // Copy the bare fixture as a stand-in on-device .m4a.
        let dir = std::env::temp_dir().join(format!("classick-backfill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dev = dir.join("track.m4a");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a"), &dev).unwrap();

        // The per-track backfill step: probe source tags → normalize art → embed.
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac")); // existing fixture: tags + embedded PNG art
        let before = std::fs::metadata(&dev).unwrap().len();
        let (tags, art, size) =
            backfill_one_file(&dev, src, &std::path::PathBuf::from("ffmpeg")).unwrap();
        let after = std::fs::metadata(&dev).unwrap().len();
        assert!(after >= before, "embedding should not shrink the file");
        assert_eq!(size, after, "returned size must match the embedded file");
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
        };
        assert_eq!(t.encoder, "ffmpeg");
        assert_eq!(t.source_format, "flac");
    }
}
