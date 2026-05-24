//! Per-Action match arms (Add / Modify / MetadataOnly / Remove / Unchanged)
//! plus their supporting helpers — `add_one`, `do_metadata_only`, `entry_from`,
//! `build_rebuild_manifest`, `count_actions`. Also owns the top-level `run`
//! function that ties preflight, diff, review, and apply together.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::mpsc::Receiver;

use crate::cli::EncoderChoice;
use crate::config::Config;
use crate::config_file;
use crate::ipod::db::{OwnedDb, TrackHandle};
use crate::ipod::device;
use crate::manifest::{self, Action, Manifest, ManifestEntry};
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
    let out = std::process::Command::new(ffmpeg_path)
        .args(["-hide_banner", "-version"])
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

pub fn run(config: &mut Config, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<()> {
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
    // Retry/Abort (or Retry/Change/Abort) prompt loop on failure.
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
        progress.summary(add, modify, remove, unchanged, total_planned);
        progress.log("Dry run; nothing was written.");
        return Ok(());
    } else if config.apply || !config.use_tui {
        // Non-interactive: just apply with the configured no_delete.
        let no_delete = config.no_delete || safeguard_force_no_delete;
        let effective_remove = if no_delete { 0 } else { remove };
        let total_planned = add + modify + metadata_only + effective_remove;
        progress.summary(add, modify, remove, unchanged, total_planned);
        no_delete
    } else {
        // Interactive review.
        progress.review(summary_struct, config.no_delete || safeguard_force_no_delete);
        match decision_rx.recv() {
            Ok(Decision::Review(ReviewDecision::Apply { no_delete })) => {
                let no_delete = no_delete || safeguard_force_no_delete;
                let effective_remove = if no_delete { 0 } else { remove };
                let total_planned = add + modify + metadata_only + effective_remove;
                progress.summary(add, modify, remove, unchanged, total_planned);
                no_delete
            }
            Ok(Decision::Review(ReviewDecision::DryRun)) => {
                progress.log("Dry run; nothing was written.");
                return Ok(());
            }
            Ok(Decision::Review(ReviewDecision::Quit)) => {
                progress.log("Aborted; nothing was written.");
                return Ok(());
            }
            Ok(Decision::Prompt { .. }) | Ok(Decision::Form { .. }) => {
                // Unexpected at this stage (no try_with_prompt / wizard caller
                // wired yet — Tasks 5+6 will do that). Return loudly rather
                // than silently swallowing a stray decision.
                return Err(anyhow!("unexpected prompt/form decision before any prompt was sent"));
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
        return Ok(());
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
    let sync_result: Result<()> = (|| -> Result<()> {
        let db = OwnedDb::open(Path::new(&mount))?;
        let guid = device::read_firewire_guid(Path::new(&mount))?;
        unsafe {
            let device_ptr = (*db.as_ptr()).device;
            device::set_firewire_guid(device_ptr, &guid)?;
        }

        let mut i = 0usize;
        for action in actions {
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
                        match db.delete_track(entry.ipod_dbid)
                            .with_context(|| format!("delete dbid {}", entry.ipod_dbid))
                        {
                            Ok(()) => break true,
                            Err(e) => {
                                let msg = format!(
                                    "Failed to remove {} (dbid {}):\n  {e}\n\nChoose:",
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
                        match db.delete_track(old.ipod_dbid)
                            .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))
                        {
                            Ok(()) => break true,
                            Err(e) => {
                                let msg = format!(
                                    "Failed to delete old version of {} (dbid {}) before re-adding:\n  {e}\n\nChoose:",
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

                    // Second half: re-add. Only runs if delete succeeded.
                    // (The --no-delete short-circuit above already returned.)
                    if deleted {
                        let added: Option<AddOneOutcome> = loop {
                            match add_one(&db, &src, config, &refalac_version) {
                                Ok(outcome) => break Some(outcome),
                                Err(e) => {
                                    let msg = format!(
                                        "Failed to add new version of {}:\n  {e}\n\nChoose:",
                                        src.path.display()
                                    );
                                    let outcome = await_prompt(
                                        progress, decision_rx, msg,
                                        &["Retry", "Skip this track", "Abort"],
                                        &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                                    )?;
                                    match outcome {
                                        PromptOutcome::Retry => continue,
                                        PromptOutcome::Skip => {
                                            // iPod state is "in-between": old
                                            // track gone, new not added. Next
                                            // run's diff will see the missing
                                            // iPod entry and re-add naturally.
                                            progress.log(format!(
                                                "Skipped Modify after partial delete for {}",
                                                src.path.display()
                                            ));
                                            break None;
                                        }
                                        _ => return Err(e),
                                    }
                                }
                            }
                        };
                        if let Some(o) = added {
                            manifest.tracks.push(entry_from(
                                &src,
                                &o.handle,
                                &o.fingerprint,
                                &o.audio_fingerprint,
                                &o.encoder,
                                &o.encoder_version,
                                &o.source_format,
                            ));
                        }
                    }
                    progress.track_done();
                }
                Action::Add(src) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("ADD {}", display_path(&src.path)),
                    );
                    // Skip is clean here: no iPod state changes, no manifest update.
                    let added: Option<AddOneOutcome> = loop {
                        match add_one(&db, &src, config, &refalac_version) {
                            Ok(outcome) => break Some(outcome),
                            Err(e) => {
                                let msg = format!(
                                    "Failed to add {}:\n  {e}\n\nChoose:",
                                    src.path.display()
                                );
                                let outcome = await_prompt(
                                    progress, decision_rx, msg,
                                    &["Retry", "Skip this track", "Abort"],
                                    &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort],
                                )?;
                                match outcome {
                                    PromptOutcome::Retry => continue,
                                    PromptOutcome::Skip => {
                                        progress.log(format!("Skipped Add for {}", src.path.display()));
                                        break None;
                                    }
                                    _ => return Err(e),
                                }
                            }
                        }
                    };
                    if let Some(o) = added {
                        manifest.tracks.push(entry_from(
                            &src,
                            &o.handle,
                            &o.fingerprint,
                            &o.audio_fingerprint,
                            &o.encoder,
                            &o.encoder_version,
                            &o.source_format,
                        ));
                    }
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
                                    "Failed to update metadata for {}:\n  {e}\n\nChoose:",
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
        }

        // 6. Commit DB + manifest. NEITHER is persisted unless we got this far.
        progress.log("Writing iPod DB...");
        db.write()?;
        progress.log("Writing manifest...");
        // Stamp the current source root onto the manifest so the next run's
        // source-change safeguard has something to compare against.
        manifest.last_source_root = Some(config.source.clone());
        manifest::save_atomic(&config.manifest_path, &manifest)?;

        progress.log("Done. Eject the iPod before unplugging.");
        Ok(())
    })();

    if let Err(e) = &sync_result {
        // While the TUI is up, push the recovery block into log_tail so users
        // see it inline. The same text is ALSO attached to the bubbled error
        // (see below) so it survives the TUI teardown that wipes log_tail.
        progress.error(format!("Sync failed: {e}"));
        for line in RECOVERY_HINT_LINES {
            progress.error((*line).to_string());
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
        anyhow!("sync failed: {e}\n\nRecovery:\n  {hint}")
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

/// Per-track result of `add_one`. A struct rather than a 6-tuple because
/// the Modify/Add arms in `run` and the manifest entry builder both consume
/// it; positional access would obscure intent at the call sites.
pub(crate) struct AddOneOutcome {
    pub handle: TrackHandle,
    pub fingerprint: String,
    pub audio_fingerprint: String,
    /// "ffmpeg" | "refalac" | "passthrough" — matches the manifest field.
    pub encoder: String,
    /// Forensic version string (e.g. "ffmpeg version n7.0 ..." or
    /// "refalac 1.85"). Empty for passthrough.
    pub encoder_version: String,
    /// ffprobe codec_name of the source (e.g. "flac", "mp3", "aac").
    pub source_format: String,
}

/// Probe → classify → branch on Passthrough/Transcode(encoder) for one source.
/// Adds the resulting file to the iPod via libgpod, then computes both file +
/// audio-only fingerprints for the manifest. Both fingerprints are computed
/// here (not by the walker) because Add/Modify are the only code paths that
/// need them — the steady-state Unchanged path stays stat-only. The audio
/// fingerprint lets future runs detect tag-only edits and take the
/// MetadataOnly fast path.
///
/// The encoder branch:
/// - Passthrough: byte-for-byte copy of the source (mp3 / aac / alac /
///   optionally wav). Encoder recorded as "passthrough" so the
///   encoder-mismatch heuristic can carve it out from future re-encodes.
/// - Transcode + Ffmpeg: existing `transcode_to_alac` (single-step ffmpeg
///   FLAC→ALAC, art passthrough).
/// - Transcode + Refalac: 2-step `transcode_via_refalac` (ffmpeg-decode to
///   WAV, then refalac to ALAC, with optional --artwork from a temp jpg).
pub(crate) fn add_one(
    db: &OwnedDb,
    src: &SourceEntry,
    config: &Config,
    refalac_version: &Option<String>,
) -> Result<AddOneOutcome> {
    let probe = transcode::probe(&src.path, &config.ffmpeg)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);
    let source_format = source_format_from_probe(&probe);

    let classify_cfg = transcode::ClassifyConfig {
        passthrough_wav: config.passthrough_wav,
    };
    let action = transcode::classify(&probe, &classify_cfg)
        .with_context(|| format!("classify {}", src.path.display()))?;

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
                // Refalac --artwork wants a path; extract embedded art to a
                // temp jpg first if any, then pass it. Cleaned up after.
                let art_path_opt = if has_embedded_art(&probe) {
                    let art_path = transcode::temp_art_path();
                    transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg).with_context(|| {
                        format!("extract art for refalac --artwork: {}", src.path.display())
                    })?;
                    Some(art_path)
                } else {
                    None
                };
                let ffmpeg_path = config.ffmpeg.as_path();
                let result = transcode::transcode_via_refalac(
                    &src.path,
                    &dst,
                    &config.refalac_path,
                    ffmpeg_path,
                    art_path_opt.as_deref(),
                )
                .with_context(|| format!("refalac transcode {}", src.path.display()));
                if let Some(p) = &art_path_opt {
                    let _ = std::fs::remove_file(p);
                }
                result?;
                let ver = refalac_version
                    .clone()
                    .unwrap_or_else(|| "refalac (version unknown)".to_string());
                ("refalac".to_string(), ver, dst)
            }
        },
    };

    // libgpod still writes its own thumbnail copy, so we extract once more
    // here for the apply_tags path. Source-side bytes are unchanged; this
    // is independent of refalac's own --artwork embed.
    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };

    let handle = db
        .add_track_with_file(&temp, &tags, art.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);

    let fingerprint = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;
    let audio_fingerprint = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;
    Ok(AddOneOutcome {
        handle,
        fingerprint,
        audio_fingerprint,
        encoder,
        encoder_version,
        source_format,
    })
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
        // Recorded per-track from add_one's classify+encoder branch so
        // future runs can detect encoder-mismatch (or carve out passthrough
        // entries that have no encoder identity to mismatch against).
        encoder: encoder.to_string(),
        encoder_version: encoder_version.to_string(),
        source_format: source_format.to_string(),
    }
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
    /// encoder name and what `add_one` records on each new entry. Mismatches
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
