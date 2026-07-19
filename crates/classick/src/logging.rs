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
use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};
use tracing_subscriber::filter::EnvFilter;

/// Initialize tracing and install the GLib log handler. Call exactly once
/// from `main` after the CLI has been parsed.
///
/// Writer dispatch:
/// - `ipc_mode == true`: tracing writes to a timestamped file under
///   `%LOCALAPPDATA%\classick\logs\core-{secs}-{nanos}-{pid}.log` (see
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
        builder.with_writer(std::io::stderr).init();
    }

    install_glib_handler();
    debug!("logging initialized (verbose={verbose}, use_tui={use_tui}, ipc_mode={ipc_mode})");
}

/// Compute the directory for IPC-mode tracing logs.
///
/// Layout: `<data_local_dir>/classick/logs/`.
/// On Windows this resolves to `%LOCALAPPDATA%\classick\logs\`. Falls back
/// to the OS temp dir if `dirs::data_local_dir()` returns None (extremely
/// unusual on supported platforms).
fn ipc_log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(crate::PROJECT_DIR)
        .join("logs")
}

fn open_ipc_log_file() -> io::Result<File> {
    let base = ipc_log_dir();
    std::fs::create_dir_all(&base)?;
    open_unique_ipc_log_file_in(&base, SystemTime::now(), std::process::id())
}

fn open_unique_ipc_log_file_in(
    base: &std::path::Path,
    now: SystemTime,
    pid: u32,
) -> io::Result<File> {
    let timestamp = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let stem = format!(
        "core-{}-{:09}-{pid}",
        timestamp.as_secs(),
        timestamp.subsec_nanos()
    );
    let mut collision = 0_u64;
    loop {
        let name = if collision == 0 {
            format!("{stem}.log")
        } else {
            format!("{stem}-{collision}.log")
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(base.join(name))
        {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => collision += 1,
            Err(error) => return Err(error),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("logging-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn unique_log_name_contains_subsecond_timestamp_and_pid() {
        let base = test_dir("qualified-name");
        let now = UNIX_EPOCH + Duration::new(1234, 5678);

        drop(open_unique_ipc_log_file_in(&base, now, 42).unwrap());

        assert!(base.join("core-1234-000005678-42.log").is_file());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn colliding_log_name_does_not_truncate_existing_file() {
        let base = test_dir("collision");
        let now = UNIX_EPOCH + Duration::new(1234, 5678);
        let original = base.join("core-1234-000005678-42.log");
        std::fs::write(&original, b"keep me").unwrap();

        drop(open_unique_ipc_log_file_in(&base, now, 42).unwrap());

        assert_eq!(std::fs::read(&original).unwrap(), b"keep me");
        assert!(base.join("core-1234-000005678-42-1.log").is_file());
        std::fs::remove_dir_all(base).unwrap();
    }
}
