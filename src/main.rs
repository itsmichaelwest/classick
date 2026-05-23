use anyhow::Result;
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::orchestrator;
use ipod_sync::progress::Progress;
use std::io::IsTerminal;

fn main() -> Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();

    // Pre-flight: decide if we're going TUI or plain. Mirrors the logic
    // Config::resolve uses (--no-tui flag, stdout is a TTY). Needs to be
    // decided BEFORE logging::init so that, in TUI mode, tracing output is
    // routed to io::sink() instead of stderr — otherwise GLib WARN lines
    // (routed through tracing::warn!) leak past the alternate screen and
    // corrupt the visible terminal below the TUI render.
    let use_tui = !cli.no_tui && std::io::stdout().is_terminal();

    // Logging next. We use the raw verbose flag from the CLI since Config
    // resolution may itself prompt through the TUI; we want tracing wired up
    // before any of that.
    // NOTE: Wave 3 (Task 3 of Phase 6 M1) will replace these `false` literals
    // with `cli.ipc_mode` and adjust `use_tui` to be suppressed when ipc_mode
    // is set. Task 2 ships only the building blocks (signatures + IpcBackend).
    ipod_sync::logging::init(cli.verbose, use_tui, false);

    let (progress, decision_rx) = Progress::start(use_tui, false)?;

    // Everything else runs while the TUI is up. Any error from here on
    // routes through progress.error / progress.prompt before exiting.
    let result = orchestrator::orchestrate(cli, &progress, &decision_rx);

    // Make sure the TUI tears down even on error. `finish` consumes `progress`,
    // so it must run after `orchestrate` returns (which only borrows it).
    // `finish` now returns Result so a panicked TUI thread (e.g. crossterm
    // setup failure on an odd terminal) is surfaced instead of being silently
    // swallowed. Prefer the orchestrator's error if both fail — that's the
    // user's intent error; the TUI thread death is secondary.
    let finish_result = progress.finish();

    result.and(finish_result)
}
