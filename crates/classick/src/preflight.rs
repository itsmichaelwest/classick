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

/// Append the platform's path separator if missing. Lives here (rather than
/// the orchestrator) because the only caller is mount resolution. On Windows
/// this is `\`; on Linux/macOS this is `/` — appending `\` on those targets
/// would corrupt the path (`\` is a valid Linux filename character) and the
/// subsequent `Path::join` would look up `/mnt/ipod\/iPod_Control` instead
/// of `/mnt/ipod/iPod_Control`.
pub fn ensure_trailing_separator(s: &str) -> String {
    if s.ends_with('\\') || s.ends_with('/') {
        s.to_string()
    } else {
        format!("{}{}", s, std::path::MAIN_SEPARATOR)
    }
}

/// Loop on `transcode::verify_tools_available` until ffmpeg/ffprobe are
/// reachable (at the configured path) or the user aborts.
pub fn verify_ffmpeg(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    loop {
        match transcode::verify_tools_available(&config.ffmpeg) {
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

/// Refuse to sync if iTunes (or its mobile-device helper) is running.
/// Background: libgpod's signed iTunesDB writes are byte-compatible with
/// what the iPod firmware accepts, but iTunes itself uses a stricter
/// signature check and will (a) reject the iPod with a "cannot read
/// contents — please Restore" dialog if it polls the device while
/// libgpod has been touching it, and (b) race us for exclusive access
/// to the iPod's pipe/file handles during a sync, producing partial
/// writes. Both failure modes have bitten the user; both are eliminated
/// by simply not letting the two coexist on the same machine while a
/// sync is in flight. We don't kill iTunes — we tell the user, give
/// them a Retry, and let them choose to close it. Windows-only because
/// iTunes is Windows/macOS, and macOS handling is left to the
/// macOS-port-someday work.
///
/// Side note on Apple's services: `AppleMobileDeviceService` is the
/// long-running device daemon iTunes installs. It owns the iPod's
/// IOCTL pipe even when iTunes itself is closed and is the more
/// invasive of the two — we surface it in the message but don't
/// refuse on its presence alone (some users keep it running for
/// iPhone/iPad syncing and forcing them to disable it system-wide is
/// hostile). Only `iTunes.exe` is hard-fail.
#[cfg(windows)]
pub fn verify_itunes_not_running(
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    loop {
        let conflicts = detect_apple_processes();
        let blocking = conflicts.iter().filter(|p| p.is_blocking).collect::<Vec<_>>();
        if blocking.is_empty() {
            return Ok(());
        }
        let names = blocking
            .iter()
            .map(|p| format!("{} (PID {})", p.name, p.pid))
            .collect::<Vec<_>>()
            .join(", ");
        let advisory = if conflicts.iter().any(|p| !p.is_blocking) {
            let advisory_names = conflicts
                .iter()
                .filter(|p| !p.is_blocking)
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n\nAlso detected (not blocking, but may cause flaky sync): {}.",
                advisory_names
            )
        } else {
            String::new()
        };
        let msg = format!(
            "Cannot sync while iTunes is running.\n\n\
             Detected: {names}.\n\n\
             iTunes and Classick both want exclusive access to the iPod.\n\
             If iTunes auto-launched on plug-in, close it (do NOT click\n\
             Restore if it asks — your iPod is fine).{advisory}\n\n\
             Choose:"
        );
        let outcome = await_prompt(
            progress,
            decision_rx,
            msg,
            &["Retry (after closing iTunes)", "Abort"],
            &[PromptOutcome::Retry, PromptOutcome::Abort],
        )?;
        match outcome {
            PromptOutcome::Retry => continue,
            _ => return Err(anyhow!("iTunes is running; aborted")),
        }
    }
}

/// Non-Windows no-op stub so the apply_loop call site doesn't need cfg
/// guards. macOS port: implement a `pgrep -x iTunes` equivalent later.
#[cfg(not(windows))]
pub fn verify_itunes_not_running(
    _progress: &Progress,
    _decision_rx: &Receiver<Decision>,
) -> Result<()> {
    Ok(())
}

/// One Apple-side process whose presence affects sync safety. `is_blocking`
/// is true when running it concurrently with a sync is known to corrupt
/// state (iTunes proper). False means "advisory, mention it" (the daemon).
#[cfg(windows)]
#[derive(Debug, Clone)]
struct AppleProcess {
    name: String,
    pid: u32,
    is_blocking: bool,
}

/// Enumerate `iTunes.exe` and `AppleMobileDeviceService.exe` via the
/// Toolhelp32 process-snapshot API. Single-digit milliseconds vs. the
/// 200-400ms PowerShell shellout this replaced, no console-flash risk,
/// no locale fragility, no extra spawn surface for the user's AV to
/// flag. If the snapshot itself fails (rare — permission-denied on
/// locked-down corporate machines is the realistic case), log and
/// return empty so the sync proceeds rather than blocking on a faulty
/// diagnostic.
#[cfg(windows)]
fn detect_apple_processes() -> Vec<AppleProcess> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        tracing::warn!(
            "preflight: CreateToolhelp32Snapshot failed ({}); skipping iTunes check",
            std::io::Error::last_os_error()
        );
        return Vec::new();
    }

    // SAFETY-ish: zero-init a PROCESSENTRY32W and set dwSize per the
    // documented contract before the first call.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;

    let mut out = Vec::new();
    let mut have_entry = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while have_entry {
        // szExeFile is a null-terminated UTF-16 array of MAX_PATH. PowerShell's
        // Get-Process surfaced the process name without the .exe suffix, so we
        // strip it for parity — the user-visible message says "iTunes (PID …)".
        let nul = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..nul]);
        let stem = name.strip_suffix(".exe").unwrap_or(&name);
        let is_itunes = stem.eq_ignore_ascii_case("iTunes");
        let is_amds = stem.eq_ignore_ascii_case("AppleMobileDeviceService");
        if is_itunes || is_amds {
            out.push(AppleProcess {
                name: stem.to_string(),
                pid: entry.th32ProcessID,
                is_blocking: is_itunes,
            });
        }
        have_entry = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }

    unsafe {
        CloseHandle(snap);
    }
    out
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
            let p = ensure_trailing_separator(m);
            loop {
                if crate::ipod::layout::itunes_db_path(Path::new(&p)).exists() {
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
