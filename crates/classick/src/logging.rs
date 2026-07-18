//! Tracing-subscriber init + GLib log handler installation.
//!
//! Three concerns wired together so `main.rs` can do a single
//! `logging::init(verbose, use_tui, ipc_mode)`:
//!
//! 1. `tracing-subscriber` with an `EnvFilter`. Honors `RUST_LOG` when set;
//!    otherwise defaults to `classick=info` (or `classick=debug` with
//!    `--verbose`).
//! 2. `g_log_set_default_handler` so libgpod's GLib `WARNING`/`CRITICAL`
//!    messages (e.g. `Error parsing recent playcounts`,
//!    `itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed`)
//!    are routed through `tracing` instead of dumped bare to stderr.
//! 3. In `--ipc-mode`, stdout is reserved for the JSON event stream — any
//!    stray bytes corrupt it. Tracing is therefore routed to a timestamped
//!    file under `%LOCALAPPDATA%\classick\logs\` (or platform equivalent).

use crate::ffi;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::{debug, info, warn};
use tracing_subscriber::filter::EnvFilter;

/// Initialize tracing and install the GLib log handler. Call exactly once
/// from `main` after the CLI has been parsed.
///
/// Writer dispatch:
/// - `ipc_mode == true`: tracing writes to a timestamped file under
///   `%LOCALAPPDATA%\classick\logs\core-{unix_ts}.log` (see
///   `docs/ipc-protocol.md` §9). This MUST stay off stdout — stdout carries
///   the JSON event stream and any non-JSON bytes corrupt it. If the file
///   can't be opened, falls back to `io::sink()` (silent); we deliberately
///   don't fall back to stderr because the parent process may be reading
///   stderr for crash diagnostics.
/// - `use_tui == true` (and not IPC): tracing writes to `io::sink()` (no-op)
///   so that GLib `WARNING`/`CRITICAL` lines (which we route through
///   `tracing::warn!`) don't leak past the alternate screen and corrupt the
///   visible terminal below the TUI render. Trade-off: ALL tracing output is
///   suppressed when the TUI is active. User-facing messages go through
///   `progress.log` / `progress.error` instead. To see tracing on screen
///   (e.g. with `--verbose` for debug sessions), pass `--no-tui`.
/// - else (plain mode): tracing writes to stderr as normal.
pub fn init(verbose: bool, use_tui: bool, ipc_mode: bool) {
    let default = if verbose {
        "classick=debug,info"
    } else {
        "classick=info,warn"
    };
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact();
    if ipc_mode {
        match open_ipc_log_file() {
            Ok(file) => {
                // Mutex<File> implements `MakeWriter`, giving us the per-event
                // locking that the multi-threaded subscriber needs.
                builder
                    .with_ansi(false) // ANSI escape codes in a log file are noise
                    .with_writer(Mutex::new(file))
                    .init();
            }
            Err(e) => {
                // Last-ditch fallback: sink. Don't emit on stderr in case the
                // parent process is capturing it for crash diagnostics.
                let _ = e;
                builder.with_ansi(false).with_writer(std::io::sink).init();
            }
        }
    } else if use_tui {
        builder.with_writer(std::io::sink).init();
    } else {
        builder.init();
    }

    install_glib_handler();
    debug!("logging initialized (verbose={verbose}, use_tui={use_tui}, ipc_mode={ipc_mode})");
}

/// Compute the path for the IPC-mode tracing log and create the directory.
///
/// Layout: `<data_local_dir>/classick/logs/core-{unix_timestamp}.log`.
/// On Windows this resolves to `%LOCALAPPDATA%\classick\logs\`. Falls back
/// to the OS temp dir if `dirs::data_local_dir()` returns None (extremely
/// unusual on supported platforms).
fn ipc_log_path() -> PathBuf {
    let base = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(crate::PROJECT_DIR)
        .join("logs");
    let _ = std::fs::create_dir_all(&base);
    // SystemTime → unix seconds, used purely as a uniqueness suffix. We
    // deliberately avoid pulling in `chrono` for this; `chrono` is not yet a
    // project dep and this task is supposed to be additive-only.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    base.join(format!("core-{secs}.log"))
}

fn open_ipc_log_file() -> std::io::Result<std::fs::File> {
    let path = ipc_log_path();
    std::fs::File::create(path)
}

extern "C" fn glib_log_handler(
    log_domain: *const std::os::raw::c_char,
    log_level: ffi::GLogLevelFlags,
    message: *const std::os::raw::c_char,
    _user_data: *mut std::os::raw::c_void,
) {
    let domain = if log_domain.is_null() {
        "glib".to_string()
    } else {
        unsafe { CStr::from_ptr(log_domain).to_string_lossy().into_owned() }
    };
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() }
    };

    // GLib's level constants are a bitmask; check most-severe first.
    // CRITICAL/WARNING -> tracing warn (libgpod's playcount parse failure
    // and splr_validate UNKNOWN are both noisy-but-benign), MESSAGE -> info,
    // anything else (INFO/DEBUG bits) -> tracing debug.
    let is_critical = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_CRITICAL) != 0;
    let is_warning = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_WARNING) != 0;
    let is_message = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_MESSAGE) != 0;

    if is_critical || is_warning {
        warn!(target: "glib", "{domain}: {message}");
    } else if is_message {
        info!(target: "glib", "{domain}: {message}");
    } else {
        debug!(target: "glib", "{domain}: {message}");
    }
}

fn install_glib_handler() {
    // SAFETY: g_log_set_default_handler stores the function pointer in a
    // process-global slot; our handler has C ABI and a 'static lifetime.
    // We pass null user_data because the handler doesn't need any state.
    unsafe {
        ffi::g_log_set_default_handler(Some(glib_log_handler), std::ptr::null_mut());
    }
}
