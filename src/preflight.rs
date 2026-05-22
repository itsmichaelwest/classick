//! Pre-resolve retry-prompt loops: ffmpeg/ffprobe availability, iPod mount
//! detection, and source-library walk. Each wraps a fallible step from the
//! original `run` in a TUI Retry/Abort (or Retry/Change/Abort) loop so the
//! user can recover transient issues without restarting the process.

use anyhow::{anyhow, Result};
use std::path::Path;
use std::sync::mpsc::Receiver;

use crate::config::Config;
use crate::ipod::detect_ipod_mount;
use crate::progress::{Decision, Progress};
use crate::source::{self, SourceEntry};
use crate::transcode;
use crate::try_with_prompt::{await_prompt, PromptOutcome};
use crate::wizard;

/// Append a trailing backslash to a Windows path if missing. Lives here
/// (rather than the orchestrator) because the only caller is mount resolution.
pub fn ensure_trailing_backslash(s: &str) -> String {
    if s.ends_with('\\') { s.to_string() } else { format!("{s}\\") }
}

/// Loop on `transcode::verify_tools_available` until ffmpeg/ffprobe are on
/// PATH or the user aborts.
pub fn verify_ffmpeg(progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<()> {
    loop {
        match transcode::verify_tools_available() {
            Ok(()) => return Ok(()),
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
}

/// Resolve the iPod mount path. Explicit `--ipod` keeps its early-return
/// validation; auto-detect gets a retry loop so the user can plug in the
/// device and retry without restarting.
pub fn resolve_ipod_mount(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<String> {
    match &config.ipod {
        Some(m) => {
            let p = ensure_trailing_backslash(m);
            if !Path::new(&p).join("iPod_Control").join("iTunes").join("iTunesDB").exists() {
                return Err(anyhow!("explicit --ipod {} does not contain iPod_Control\\iTunes\\iTunesDB", p));
            }
            Ok(p)
        }
        None => loop {
            match detect_ipod_mount() {
                Ok(m) => return Ok(m),
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
    }
}

/// Walk the source library with retry/change/abort prompting on failure.
///
/// Returns:
/// - `Ok(Some(entries))` — normal success.
/// - `Ok(None)` — user picked "Change source path"; the wizard ran and wrote
///   a new source to config.toml. The caller MUST short-circuit (return Ok)
///   because the v1 limitation is that `Config` is borrowed immutably so we
///   can't swap `config.source` mid-run; the user must re-launch.
/// - `Err(_)` — user aborted, or a non-prompt error bubbled up.
pub fn walk_source(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<Option<Vec<SourceEntry>>> {
    loop {
        match source::walk(&config.source) {
            Ok(s) => return Ok(Some(s)),
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
                        // v1 limitation: Config is borrowed immutably in
                        // apply_loop::run, so we can't swap config.source
                        // mid-run. Persist the new source to config.toml via
                        // the wizard and ask the user to re-launch.
                        let new_source = wizard::run(progress, decision_rx)?;
                        progress.log(format!(
                            "Source updated to {}. Re-launch ipod-sync to use it.",
                            new_source.display()
                        ));
                        return Ok(None);
                    }
                    _ => return Err(anyhow!("source unreachable; aborted")),
                }
            }
        }
    }
}
