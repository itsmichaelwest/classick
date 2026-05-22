use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self, Config};
use ipod_sync::config_file;
use ipod_sync::ipod::db::{OwnedDb, Tags, TrackHandle};
use ipod_sync::ipod::{device, detect_ipod_mount};
use ipod_sync::manifest::{self, Action, Manifest, ManifestEntry};
use ipod_sync::progress::{ActionPlanSummary, Decision, Progress, ReviewDecision};
use ipod_sync::source::{self, SourceEntry};
use ipod_sync::transcode::{self, has_embedded_art, ProbeOutput, ProbeTags};
use ipod_sync::try_with_prompt::{await_prompt, PromptOutcome};
use ipod_sync::wizard;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::mpsc::Receiver;

fn main() -> Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();

    // Logging first (no TUI needed for tracing-subscriber). We use the raw
    // verbose flag from the CLI since Config resolution may itself prompt
    // through the TUI; we want tracing wired up before any of that.
    ipod_sync::logging::init(cli.verbose);

    // Pre-flight: decide if we're going TUI or plain. Mirrors the logic
    // Config::resolve uses (--no-tui flag, stdout is a TTY).
    let use_tui = !cli.no_tui && std::io::stdout().is_terminal();

    let (progress, decision_rx) = Progress::start(use_tui)?;

    // Everything else runs while the TUI is up. Any error from here on
    // routes through progress.error / progress.prompt before exiting.
    let result = orchestrate(cli, &progress, &decision_rx);

    // Make sure the TUI tears down even on error. `finish` consumes `progress`,
    // so it must run after `orchestrate` returns (which only borrows it).
    progress.finish();

    result
}

/// Renamed wrapper that contains all the post-Progress work. Errors bubble up
/// through this and into main; progress.finish() runs unconditionally afterwards.
fn orchestrate(cli: Cli, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<()> {
    // Surface config.toml parse errors with a TUI prompt + reset option BEFORE
    // anything else touches the persisted config (ensure_source_or_wizard
    // itself calls config_file::load and would otherwise blow up on a corrupt
    // file). Loop so a successful reset-then-retry continues the run.
    let config_path = config_file::default_path()?;
    loop {
        match config_file::load(&config_path) {
            Ok(_) => break,
            Err(e) => {
                let msg = format!(
                    "Could not parse {}:\n  {e}\n\n\
                     [1] Reset config to defaults (deletes the file)\n\
                     [2] Abort and fix it manually",
                    config_path.display()
                );
                let outcome = await_prompt(
                    progress,
                    decision_rx,
                    msg,
                    &["Reset to defaults", "Abort"],
                    &[PromptOutcome::Custom(0), PromptOutcome::Abort],
                )?;
                match outcome {
                    PromptOutcome::Custom(0) => {
                        std::fs::remove_file(&config_path)
                            .map_err(|e| anyhow!("remove {}: {e}", config_path.display()))?;
                        progress.log("config reset; retrying load...".to_string());
                        continue;
                    }
                    _ => return Err(anyhow!("config parse failed; aborted")),
                }
            }
        }
    }

    ensure_source_or_wizard(&cli, progress, decision_rx)?;
    let config = config::resolve(cli)?;
    run(&config, progress, decision_rx)
}

/// If no source is resolvable from CLI/env/persisted config AND we're on a TTY
/// AND --no-tui isn't set, launch the wizard. After it succeeds, the persisted
/// config has a source and the subsequent config::resolve will succeed.
///
/// Non-TTY or --no-tui: do nothing (resolve will produce its standard error).
fn ensure_source_or_wizard(
    cli: &Cli,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    // Quick check: if CLI provided source, we don't need anything.
    if cli.source.is_some() {
        return Ok(());
    }
    if std::env::var(ipod_sync::config::SOURCE_ENV).is_ok() {
        return Ok(());
    }
    let config_path = config_file::default_path()?;
    if let Some(persisted) = config_file::load(&config_path)? {
        if persisted.source.is_some() {
            return Ok(());
        }
    }
    // No source from any layer. Check whether we can run the wizard.
    if cli.no_tui || !std::io::stdout().is_terminal() {
        return Ok(()); // resolve will error with the standard message
    }
    // Launch the wizard via the running Progress. On success it writes the
    // source to config.toml; the subsequent config::resolve will pick it up.
    let _saved = wizard::run(progress, decision_rx)?;
    Ok(())
}

fn run(config: &Config, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<()> {
    if config.dry_run && config.apply {
        return Err(anyhow!("--dry-run and --apply are mutually exclusive"));
    }
    if !config.dry_run && !config.apply && !config.use_tui {
        return Err(anyhow!(
            "interactive review requires a TTY.\n\
             Pass --apply to apply immediately, or --dry-run to preview without changes."
        ));
    }

    // ffmpeg/ffprobe gate — surface a TUI prompt with Retry/Abort instead
    // of bailing to stderr if the tools aren't on PATH yet.
    loop {
        match transcode::verify_tools_available() {
            Ok(()) => break,
            Err(e) => {
                let msg = format!(
                    "ffmpeg or ffprobe was not found on PATH:\n  {e}\n\n\
                     Install via: winget install Gyan.FFmpeg\n\
                     Then retry."
                );
                let outcome = await_prompt(
                    progress,
                    decision_rx,
                    msg,
                    &["Retry", "Abort"],
                    &[PromptOutcome::Retry, PromptOutcome::Abort],
                )?;
                match outcome {
                    PromptOutcome::Retry => continue,
                    _ => return Err(anyhow!("ffmpeg/ffprobe required; aborted")),
                }
            }
        }
    }

    // 1. Resolve iPod mount. The explicit --ipod branch keeps its early-return
    //    validation; auto-detect gets a retry loop so the user can plug in
    //    the device and retry without restarting the process.
    let mount = match &config.ipod {
        Some(m) => {
            let p = ensure_trailing_backslash(m);
            if !Path::new(&p).join("iPod_Control").join("iTunes").join("iTunesDB").exists() {
                return Err(anyhow!("explicit --ipod {} does not contain iPod_Control\\iTunes\\iTunesDB", p));
            }
            p
        }
        None => loop {
            match detect_ipod_mount() {
                Ok(m) => break m,
                Err(e) => {
                    let msg = format!(
                        "{e}\n\nPlug in your iPod and press [1] Retry, or [2] Abort to quit."
                    );
                    let outcome = await_prompt(
                        progress,
                        decision_rx,
                        msg,
                        &["Retry", "Abort"],
                        &[PromptOutcome::Retry, PromptOutcome::Abort],
                    )?;
                    if outcome != PromptOutcome::Retry {
                        return Err(anyhow!("iPod required; aborted"));
                    }
                }
            }
        },
    };

    // 2. Walk source. Wrap in retry loop so a transient SMB blip / wrong path
    //    can be recovered without restarting (with an option to re-pick the
    //    source via the wizard — see v1 limitation note below).
    let sources = loop {
        match source::walk(&config.source) {
            Ok(s) => break s,
            Err(e) => {
                let msg = format!(
                    "Source library unreachable at {}:\n  {e}\n\nChoose:",
                    config.source.display()
                );
                let outcome = await_prompt(
                    progress,
                    decision_rx,
                    msg,
                    &["Retry", "Change source path", "Abort"],
                    &[PromptOutcome::Retry, PromptOutcome::Custom(1), PromptOutcome::Abort],
                )?;
                match outcome {
                    PromptOutcome::Retry => continue,
                    PromptOutcome::Custom(1) => {
                        // v1 limitation: Config is borrowed immutably here, so
                        // we can't swap config.source mid-run. Persist the new
                        // source to config.toml via the wizard and ask the
                        // user to re-launch. Restructuring to a mutable Config
                        // is deferred.
                        let new_source = wizard::run(progress, decision_rx)?;
                        progress.log(format!(
                            "Source updated to {}. Re-launch ipod-sync to use it.",
                            new_source.display()
                        ));
                        return Ok(());
                    }
                    _ => return Err(anyhow!("source unreachable; aborted")),
                }
            }
        }
    };

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
    let actions = manifest::diff(
        &manifest,
        &sources,
        source::fingerprint,
        source::audio_fingerprint,
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

    // Decide effective no_delete based on dry-run / apply / interactive review.
    let effective_no_delete: bool = if config.dry_run {
        let effective_remove = if config.no_delete { 0 } else { remove };
        let total_planned = add + modify + metadata_only + effective_remove;
        progress.summary(add, modify, remove, unchanged, total_planned);
        progress.log("Dry run; nothing was written.");
        return Ok(());
    } else if config.apply || !config.use_tui {
        // Non-interactive: just apply with the configured no_delete.
        let effective_remove = if config.no_delete { 0 } else { remove };
        let total_planned = add + modify + metadata_only + effective_remove;
        progress.summary(add, modify, remove, unchanged, total_planned);
        config.no_delete
    } else {
        // Interactive review.
        progress.review(summary_struct, config.no_delete);
        match decision_rx.recv() {
            Ok(Decision::Review(ReviewDecision::Apply { no_delete })) => {
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
                        format!("REMOVE {} (dbid {})", entry.source_path.display(), entry.ipod_dbid),
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
                        format!("MODIFY {}", src.path.display()),
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
                        let added: Option<(TrackHandle, String, String)> = loop {
                            match add_one(&db, &src) {
                                Ok(triple) => break Some(triple),
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
                        if let Some((handle, fp, audio_fp)) = added {
                            manifest.tracks.push(entry_from(&src, &handle, &fp, &audio_fp));
                        }
                    }
                    progress.track_done();
                }
                Action::Add(src) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("ADD {}", src.path.display()),
                    );
                    // Skip is clean here: no iPod state changes, no manifest update.
                    let added: Option<(TrackHandle, String, String)> = loop {
                        match add_one(&db, &src) {
                            Ok(triple) => break Some(triple),
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
                    if let Some((handle, fp, audio_fp)) = added {
                        manifest.tracks.push(entry_from(&src, &handle, &fp, &audio_fp));
                    }
                    progress.track_done();
                }
                Action::MetadataOnly { source, entry } => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("METADATA {}", source.path.display()),
                    );
                    // Whole metadata-update sequence is bundled into
                    // `do_metadata_only` so the retry loop has a single
                    // fallible call to wrap. Skip leaves manifest as-is.
                    let updated: Option<ManifestEntry> = loop {
                        match do_metadata_only(&db, &source, &entry) {
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
                                        progress.log(format!(
                                            "Skipped MetadataOnly for {}",
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
        manifest::save_atomic(&config.manifest_path, &manifest)?;

        if config.save_config {
            let config_path = config_file::default_path()?;
            config_file::save(&config_path, &config.to_persisted())?;
            progress.log(format!("config saved to {}", config_path.display()));
        }

        progress.log("Done. Eject the iPod before unplugging.");
        Ok(())
    })();

    if let Err(e) = &sync_result {
        progress.error(format!("Sync failed: {e}"));
        progress.error("The iPod may now contain orphan track files (added but not".to_string());
        progress.error("in the iTunesDB), and the manifest has NOT been updated.".to_string());
        progress.error("To recover: re-run with --rebuild-manifest, which will read the iPod's".to_string());
        progress.error("current DB and create a fresh manifest. Then run normally.".to_string());
    }
    // progress.finish() is called by main() after this fn returns.
    sync_result
}

/// Run the full MetadataOnly path for one entry: probe + tag-extract +
/// optional embedded-art extract + libgpod update + recompute source
/// fingerprint. Returns the freshly-built ManifestEntry to push.
///
/// Bundled into one fallible call so the per-track retry/skip loop in `run`
/// has a single closure to retry — anything failing inside re-runs the lot.
fn do_metadata_only(
    db: &OwnedDb,
    source: &SourceEntry,
    entry: &ManifestEntry,
) -> Result<ManifestEntry> {
    let probe = transcode::probe(&source.path)
        .with_context(|| format!("probe {}", source.path.display()))?;
    let tags = tags_from_probe(&probe);
    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&source.path, &art_path)?;
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
    })
}

/// Transcode + add one source file. Returns the iPod-side handle plus the
/// freshly-computed source fingerprint AND audio-only fingerprint. Both
/// fingerprints are computed here (not by the walker) because Add/Modify are
/// the only code paths that need them for the manifest entry — the
/// steady-state Unchanged path stays stat-only. The audio fingerprint lets
/// future runs detect tag-only edits and take the MetadataOnly fast path.
fn add_one(db: &OwnedDb, src: &SourceEntry) -> Result<(TrackHandle, String, String)> {
    let probe = transcode::probe(&src.path)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);

    let temp = transcode::temp_alac_path();
    if let Some(parent) = temp.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    transcode::transcode_to_alac(&src.path, &temp)
        .with_context(|| format!("transcode {}", src.path.display()))?;

    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };

    let handle = db.add_track_with_file(&temp, &tags, art.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);

    let fingerprint = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;
    let audio_fp = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;
    Ok((handle, fingerprint, audio_fp))
}

fn tags_from_probe(p: &ProbeOutput) -> Tags {
    let pt: &ProbeTags = match &p.format.tags {
        Some(t) => t,
        None => return Tags::default(),
    };

    let track_nr = pt.track.as_deref().and_then(|s| parse_int_first_field(s));
    let tracks_from_total = pt.track_total.as_deref().and_then(|s| s.trim().parse().ok());
    let tracks_from_slash = pt.track.as_deref().and_then(parse_int_second_field);
    let tracks = tracks_from_total.or(tracks_from_slash);

    let disc_nr = pt.disc.as_deref().and_then(|s| parse_int_first_field(s));
    let discs_from_total = pt.disc_total.as_deref().and_then(|s| s.trim().parse().ok());
    let discs_from_slash = pt.disc.as_deref().and_then(parse_int_second_field);
    let discs = discs_from_total.or(discs_from_slash);

    let year = pt.date.as_deref().and_then(parse_year);

    Tags {
        title: pt.title.clone(),
        artist: pt.artist.clone(),
        album: pt.album.clone(),
        album_artist: pt.album_artist.clone(),
        genre: pt.genre.clone(),
        composer: pt.composer.clone(),
        year,
        track_nr,
        tracks,
        disc_nr,
        discs,
    }
}

/// "9/12" -> Some(9). "9" -> Some(9). "" / garbage -> None.
fn parse_int_first_field(s: &str) -> Option<i32> {
    s.split('/').next()?.trim().parse().ok()
}

/// "9/12" -> Some(12). "9" -> None.
fn parse_int_second_field(s: &str) -> Option<i32> {
    s.split('/').nth(1)?.trim().parse().ok()
}

fn parse_year(s: &str) -> Option<i32> {
    s.split('-').next()?.trim().parse().ok()
}

fn count_actions(actions: &[Action]) -> (usize, usize, usize, usize, usize) {
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

fn entry_from(
    src: &SourceEntry,
    handle: &TrackHandle,
    fingerprint: &str,
    audio_fingerprint: &str,
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
    }
}

fn build_rebuild_manifest(db: &OwnedDb) -> Manifest {
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
    }).collect();
    Manifest { version: 1, ipod_serial: None, tracks }
}

fn ensure_trailing_backslash(s: &str) -> String {
    if s.ends_with('\\') { s.to_string() } else { format!("{s}\\") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_first_field_handles_slash_and_lone() {
        assert_eq!(parse_int_first_field("9/12"), Some(9));
        assert_eq!(parse_int_first_field("9"), Some(9));
        assert_eq!(parse_int_first_field(""), None);
        assert_eq!(parse_int_first_field("abc"), None);
    }

    #[test]
    fn parse_int_second_field_only_returns_after_slash() {
        assert_eq!(parse_int_second_field("9/12"), Some(12));
        assert_eq!(parse_int_second_field("9"), None);
    }

    #[test]
    fn parse_year_handles_iso_date_and_lone_year() {
        assert_eq!(parse_year("2002-09-24"), Some(2002));
        assert_eq!(parse_year("2002"), Some(2002));
        assert_eq!(parse_year(""), None);
    }
}
