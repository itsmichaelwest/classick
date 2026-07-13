//! Top-level orchestration: config-reset loop, source-or-wizard gate, then
//! handoff to `apply_loop::run`. Lives between `main` (which only handles
//! Progress setup/teardown) and the per-action machinery in `apply_loop`.

use anyhow::{anyhow, Result};
use std::io::IsTerminal;
use std::sync::mpsc::Receiver;

use crate::apply_loop::{self, RunOutcome};
use crate::cli::Cli;
use crate::config;
use crate::config_file;
use crate::progress::{Decision, Progress};
use crate::try_with_prompt::{await_prompt, PromptOutcome};
use crate::wizard;

/// Renamed wrapper that contains all the post-Progress work. Errors bubble up
/// through this and into main; progress.finish() runs unconditionally afterwards.
/// The `RunOutcome` (Completed vs Paused) is passed through from
/// `apply_loop::run` so main.rs can decide which terminal progress event to
/// emit before tearing down.
pub fn orchestrate(cli: Cli, progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<RunOutcome> {
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
    let mut config = config::resolve(cli)?;
    if config.backfill_rockbox {
        return apply_loop::backfill_rockbox(&mut config, progress, decision_rx);
    }
    apply_loop::run(&mut config, progress, decision_rx)
}

/// If no source is resolvable from CLI/env/persisted config AND we're on a TTY
/// AND --no-tui isn't set, launch the wizard. After it succeeds, the persisted
/// config has a source and the subsequent config::resolve will succeed.
///
/// Non-TTY or --no-tui: do nothing (resolve will produce its standard error).
pub fn ensure_source_or_wizard(
    cli: &Cli,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    // In IPC mode, the GUI owns first-launch onboarding (M2). The core
    // expects the source to already be resolvable from --source, the env
    // var, or persisted config.toml; if it isn't, config::resolve will fail
    // with its standard error which is then surfaced to the GUI as an
    // `error` IPC event. Crucially, we MUST NOT call wizard::run here —
    // it issues form prompts on the same channel the UI is driving, but
    // the UI's first-launch dialog (CoreLocator / source picker in T8)
    // isn't wired to reply to mid-orchestrate form events in M1.
    if cli.ipc_mode {
        return Ok(());
    }
    // Quick check: if CLI provided source, we don't need anything.
    if cli.source.is_some() {
        return Ok(());
    }
    if std::env::var(crate::config::SOURCE_ENV).is_ok() {
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
