//! Console-window suppression for subprocesses spawned by the daemon.
//!
//! The Rust core ships as a console-subsystem `.exe` but in production
//! it runs as a child of the WinUI host, which the C# side spawns with
//! `CreateNoWindow = true`. That gives the daemon (and its descendants)
//! no inherited console. When the daemon then spawns a console-
//! subsystem child via `std::process::Command` or `tokio::process::Command`,
//! Windows allocates a *fresh* console window for the child unless
//! `CREATE_NO_WINDOW` is set in the creation flags. The user sees this
//! as a black console box flashing on screen for every ffmpeg, ffprobe,
//! powershell, and sync-subprocess invocation — once per track over a
//! 1,000-track library is intolerable.
//!
//! Both `std` and `tokio` Commands expose `creation_flags` on Windows
//! (std via `std::os::windows::process::CommandExt`; tokio as a direct
//! method). This module wraps both in an extension trait so call sites
//! stay fluent: `Command::new(...).args(...).no_console().status()`.
//!
//! Non-Windows: no-op so call sites don't need cfg gates.

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Apply `CREATE_NO_WINDOW` so console-subsystem subprocesses don't
/// flash a console window. No-op on non-Windows. Chainable.
pub trait NoConsoleWindow {
    fn no_console(&mut self) -> &mut Self;
}

impl NoConsoleWindow for std::process::Command {
    fn no_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoConsoleWindow for tokio::process::Command {
    fn no_console(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            // tokio::process::Command has `creation_flags` as a direct
            // method on Windows (no trait import needed).
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}
