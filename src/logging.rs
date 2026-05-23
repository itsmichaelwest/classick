//! Tracing-subscriber init + GLib log handler installation.
//!
//! Two concerns wired together so `main.rs` can do a single `logging::init(verbose, use_tui)`:
//!
//! 1. `tracing-subscriber` with an `EnvFilter`. Honors `RUST_LOG` when set;
//!    otherwise defaults to `ipod_sync=info` (or `ipod_sync=debug` with
//!    `--verbose`).
//! 2. `g_log_set_default_handler` so libgpod's GLib `WARNING`/`CRITICAL`
//!    messages (e.g. `Error parsing recent playcounts`,
//!    `itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed`)
//!    are routed through `tracing` instead of dumped bare to stderr.

use crate::ffi;
use std::ffi::CStr;
use tracing::{debug, info, warn};
use tracing_subscriber::filter::EnvFilter;

/// Initialize tracing and install the GLib log handler. Call exactly once
/// from `main` after config has been resolved.
///
/// `use_tui` controls where tracing output goes:
/// - `false` (plain mode): tracing writes to stderr as normal.
/// - `true`  (TUI mode): tracing writes to `io::sink()` (no-op) so that
///   GLib `WARNING`/`CRITICAL` lines (which we route through `tracing::warn!`)
///   don't leak past the alternate screen and corrupt the visible terminal
///   below the TUI render. Trade-off: ALL tracing output is suppressed when
///   the TUI is active. User-facing messages go through `progress.log` /
///   `progress.error` instead. To see tracing on screen (e.g. with
///   `--verbose` for debug sessions), pass `--no-tui`.
pub fn init(verbose: bool, use_tui: bool) {
    let default = if verbose { "ipod_sync=debug,info" } else { "ipod_sync=info,warn" };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact();
    if use_tui {
        builder.with_writer(std::io::sink).init();
    } else {
        builder.init();
    }

    install_glib_handler();
    debug!("logging initialized (verbose={verbose}, use_tui={use_tui})");
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
