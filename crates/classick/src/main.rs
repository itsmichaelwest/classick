use anyhow::Result;
use clap::Parser;
use classick::cli::Cli;
use classick::orchestrator;
use classick::progress::Progress;
use std::io::IsTerminal;

fn main() -> Result<()> {
    // Windows-only: point gdk-pixbuf at the loaders.cache build.rs staged
    // next to the binary. On Linux/macOS, gdk-pixbuf is system-installed and
    // discovers loaders via $XDG_DATA_DIRS; we don't override it (setting
    // GDK_PIXBUF_MODULE_FILE on Unix would actively break the system path).
    #[cfg(windows)]
    unsafe {
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE"));
    }

    let cli = Cli::parse();

    // Daemon mode bypasses TUI / progress / orchestrate entirely. Logging is
    // routed to a file (like ipc-mode) since stdout/stderr are not the wire
    // here — clients talk to the daemon over a named pipe (Windows) or a
    // Unix-domain socket (everywhere else).
    if cli.daemon {
        classick::logging::init(cli.verbose, /*use_tui*/ false, /*ipc_mode*/ true);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(classick::daemon::runtime::run_daemon());
    }

    // Pre-flight: decide if we're going TUI, plain, or IPC. IPC mode always
    // wins — when --ipc-mode is set, a GUI frontend owns the presentation
    // layer and the Rust core just speaks JSON over stdio. TUI is suppressed
    // regardless of --no-tui / TTY-ness in that case.
    //
    // This decision MUST happen BEFORE logging::init so that, in TUI mode,
    // tracing output is routed to io::sink() instead of stderr (otherwise
    // GLib WARN lines routed through tracing::warn! leak past the alternate
    // screen and corrupt the visible terminal below the TUI render), and in
    // IPC mode tracing goes to a file (stdout is the JSON wire and must
    // stay clean).
    let use_tui = !cli.no_tui && !cli.ipc_mode && std::io::stdout().is_terminal();

    // Logging next. We use the raw verbose flag from the CLI since Config
    // resolution may itself prompt through the TUI/IPC; we want tracing
    // wired up before any of that.
    classick::logging::init(cli.verbose, use_tui, cli.ipc_mode);

    let (progress, decision_rx) = Progress::start(use_tui, cli.ipc_mode)?;

    // Everything else runs while the TUI / IPC backend is up. Any error from
    // here on routes through progress.error before we tear down. In IPC mode
    // that becomes an `error` event on the wire; in TUI mode it lands in the
    // log tail; in plain mode it goes to stderr.
    let result = orchestrator::orchestrate(cli, &progress, &decision_rx);

    // On orchestrator failure: surface the full anyhow context chain as an
    // `error` event BEFORE Finish so the UI has a useful message to display.
    // `{:#}` formatting walks the context chain (e.g. "manifest write failed:
    // disk full" instead of just "io error").
    if let Err(e) = &result {
        progress.error(format!("{e:#}"));
    }

    // Tear down the backend. `finish` consumes `progress` and now carries
    // success so the IPC `finish.success` field agrees with the process exit
    // code. anyhow's Termination impl maps `Err(_)` to exit code 1 already;
    // this just makes sure the wire event tells the same story.
    //
    // `finish` returns Result so a panicked backend thread (e.g. crossterm
    // setup failure on an odd terminal, or an IPC stdout write failing
    // because the UI was killed) is surfaced. Prefer the orchestrator's
    // error if both fail — that's the user's intent error; backend death is
    // secondary.
    let finish_result = progress.finish(result.is_ok());

    result.and(finish_result)
}
