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

/// Loop on `transcode::verify_refalac_available` until refalac64 is reachable
/// or the user aborts. Only invoked when `config.encoder == EncoderChoice::Refalac`
/// (caller's responsibility — default ffmpeg encoder doesn't need this probe).
///
/// Returns the resolved version string on success; threaded through
/// `apply_loop::run` so Wave 3 Task 6 can record it in `ManifestEntry.encoder_version`.
pub fn verify_refalac(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<String> {
    loop {
        match transcode::verify_refalac_available(&config.refalac_path) {
            Ok(version) => return Ok(version),
            Err(e) => {
                let msg = format!(
                    "You picked --encoder refalac but refalac64 wasn't reachable at {}.\n  {e}\n\n\
                     To install (one-time setup):\n\
                     1. Download the latest qaac release:\n\
                        https://github.com/nu774/qaac/releases\n\
                     2. Extract refalac64.exe + libFLAC.dll from the zip\n\
                     3. Drop both files into the project's vendor/refalac/ directory\n\
                        (or put refalac64.exe on PATH, or pass --refalac-path <path>)\n\
                     4. Rebuild (cargo build --release) so build.rs picks them up\n\n\
                     Don't want to install qaac? Re-run without --encoder refalac \
                     (or with --encoder ffmpeg) to use the default ffmpeg encoder, \
                     which is already working on this machine.",
                    config.refalac_path.display()
                );
                let outcome = await_prompt(
                    progress,
                    decision_rx,
                    msg,
                    &["Retry (after installing)", "Abort"],
                    &[PromptOutcome::Retry, PromptOutcome::Abort],
                )?;
                match outcome {
                    PromptOutcome::Retry => continue,
                    _ => return Err(anyhow!(
                        "refalac required for --encoder refalac; aborted (see prompt for install steps, or use --encoder ffmpeg)"
                    )),
                }
            }
        }
    }
}

/// Resolve the iPod mount path. Both branches (explicit `--ipod` and
/// auto-detect) wrap the fallible check in a Retry/Abort prompt loop so the
/// user can plug in the device and retry without restarting.
pub fn resolve_ipod_mount(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<String> {
    match &config.ipod {
        Some(m) => {
            let p = ensure_trailing_backslash(m);
            loop {
                if Path::new(&p).join("iPod_Control").join("iTunes").join("iTunesDB").exists() {
                    return Ok(p);
                }
                let msg = format!(
                    "Explicit --ipod {p} does not contain iPod_Control\\iTunes\\iTunesDB.\n\n\
                     Make sure the iPod is mounted at {p} (or unplug + re-plug to re-mount), \
                     then press [1] Retry, or [2] Abort to quit."
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
/// On "Change source path" the wizard runs, persists the new path to
/// config.toml, AND writes it back into the in-memory `config.source` so the
/// next loop iteration walks the new path in the SAME run (no re-launch).
/// This is the only field on `Config` this function mutates.
pub fn walk_source(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<Vec<SourceEntry>> {
    loop {
        match source::walk(&config.source) {
            Ok(s) => return Ok(s),
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
                        // Wizard persists the new source to config.toml AND
                        // returns it; swap it into the live Config so the
                        // next iteration's source::walk uses the new path.
                        let new_source = wizard::run(progress, decision_rx)?;
                        progress.log(format!(
                            "Source updated to {}; retrying walk...",
                            new_source.display()
                        ));
                        config.source = new_source;
                        continue;
                    }
                    _ => return Err(anyhow!("source unreachable; aborted")),
                }
            }
        }
    }
}
