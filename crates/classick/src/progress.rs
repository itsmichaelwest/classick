//! Progress reporting: ratatui TUI when stdout is a TTY + --no-tui is off,
//! plain log lines otherwise. Main thread sends events; a dedicated thread
//! drains the channel and renders.

use anyhow::{anyhow, Context, Result};
use crossterm::event::{Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::ipc::{ArtworkSummary, SkippedForSpace};

/// Whole-run-average sync ETA. Shared by the TUI and IPC progress backends so
/// both surface an identical estimate. Deliberately simple: elapsed time since
/// the first track divided by completed-track count, projected over the
/// remaining tracks. A rolling window is a possible later refinement.
pub struct EtaEstimator {
    started_at: Instant,
    done: usize,
}

impl Default for EtaEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl EtaEstimator {
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// Test seam: inject the start instant so elapsed time is deterministic.
    pub fn new_at(started_at: Instant) -> Self {
        Self { started_at, done: 0 }
    }

    /// Call once per completed track (on `TrackDone`).
    pub fn record_track_done(&mut self) {
        self.done += 1;
    }

    /// Estimated seconds remaining given the 1-based `current` track and
    /// `total`. `None` until at least one track has completed, or when nothing
    /// remains — so the UI shows a plain "X of Y" early instead of a wild guess.
    pub fn eta_secs(&self, current: usize, total: usize) -> Option<u64> {
        let _ = current; // remaining is derived from done/total, not current
        if self.done == 0 || total == 0 {
            return None;
        }
        let remaining = total.saturating_sub(self.done);
        if remaining == 0 {
            return None;
        }
        let per_track = self.started_at.elapsed().as_secs_f64() / self.done as f64;
        Some((per_track * remaining as f64).round() as u64)
    }
}

/// Snapshot of the action plan sent to the TUI for the Review state.
#[derive(Debug, Clone, Copy)]
pub struct ActionPlanSummary {
    pub add: usize,
    pub modify: usize,
    pub metadata_only: usize,
    pub remove: usize,
    pub unchanged: usize,
}

/// User's decision from the Review state. Sent from the TUI thread back to
/// the main thread via the decision channel, wrapped in `Decision::Review`.
#[derive(Debug, Clone, Copy)]
pub enum ReviewDecision {
    /// Proceed with the apply loop. `no_delete` carries the user's possibly-toggled value.
    Apply { no_delete: bool },
    /// Skip the apply loop and exit cleanly (effectively a one-shot --dry-run).
    DryRun,
    /// Exit cleanly without applying anything or saving any state.
    Quit,
}

/// Choice prompt for try_with_prompt and ad-hoc error dialogs.
/// `id` correlates the request with the user's response on the back-channel.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub id: u64,
    pub message: String,
    pub options: Vec<String>,
}

/// Text-input prompt for ad-hoc user input (wizard, path edits, etc.)
#[derive(Debug, Clone)]
pub struct FormRequest {
    pub id: u64,
    pub label: String,
    /// Pre-fills the input box; empty for fresh entries.
    pub initial: String,
    /// Shown below the input box.
    pub hint: String,
}

/// Anything the TUI thread sends back to the orchestrator. The Phase 3.y
/// ReviewDecision is folded in as `Decision::Review`; new variants land here too.
#[derive(Debug, Clone)]
pub enum Decision {
    Review(ReviewDecision),
    Prompt { id: u64, choice: usize },
    /// `value: None` means the user aborted (Esc / Ctrl+C).
    Form { id: u64, value: Option<String> },
    /// Graceful pause: drain in-flight tracks, checkpoint, then stop.
    /// Bare variant for now — IPC command/event + terminal outcome land in
    /// a later task; the apply loop's decision poll already matches it.
    Pause,
}

/// Events sent from the main thread to the progress thread.
pub enum ProgressEvent {
    Header { source: String, ipod: String, manifest: String },
    Summary { add: usize, modify: usize, metadata_only: usize, remove: usize, unchanged: usize, total_planned: usize },
    Review { summary: ActionPlanSummary, no_delete: bool },
    Prompt(PromptRequest),
    Form(FormRequest),
    TrackStart { current: usize, total: usize, label: String },
    TrackDone,
    Log(String),
    Error(String),
    /// Terminal event. `success` reflects whether the orchestrator returned
    /// `Ok` (true) or `Err` (false). Used by the IPC backend to populate
    /// `finish.success`, and by the process exit code in main (anyhow's
    /// `Termination` impl on the Err path already maps to a non-zero exit;
    /// this just makes sure the Finish event itself agrees).
    ///
    /// `skipped_for_space`/`artwork`/`db_restored` are populated from
    /// `Progress`'s internal `finish_details` state (set via `note_*`
    /// methods during the run) rather than passed as a `finish()` argument —
    /// see `Progress::note_db_restored`/`note_skipped_for_space`.
    Finish {
        success: bool,
        skipped_for_space: Option<SkippedForSpace>,
        artwork: Option<ArtworkSummary>,
        db_restored: bool,
    },
    /// Terminal event: graceful pause (see `Decision::Pause`). Completed
    /// tracks were already committed to the iTunesDB + manifest by the apply
    /// loop's final checkpoint before this is sent. No fields.
    Paused,
}

/// Data accumulated during a run via `Progress::note_*` and read once by
/// `finish()` to populate the terminal `Finish` event's optional fields.
/// Behind a `Mutex` because `Progress` is shared as `&Progress` across the
/// apply loop's call chain (preflight, apply_loop, the Task-4 auto-restore
/// closures) rather than threaded through as an explicit return value.
#[derive(Debug, Clone, Default)]
struct FinishDetails {
    skipped_for_space: Option<SkippedForSpace>,
    artwork: Option<ArtworkSummary>,
    db_restored: bool,
}

pub struct Progress {
    sender: Sender<ProgressEvent>,
    thread: Option<JoinHandle<()>>,
    finish_details: Mutex<FinishDetails>,
}

impl Progress {
    /// Spawn the progress-rendering thread.
    ///
    /// Three-way dispatch, in priority order:
    /// 1. `ipc_mode == true` → [`run_ipc`]: serializes events to stdout as
    ///    newline-delimited JSON per `docs/ipc-protocol.md`; parses commands
    ///    from stdin back into the decision channel. No terminal manipulation.
    ///    Tracing goes to a file (see [`crate::logging::init`]).
    /// 2. `use_tui == true` AND stdout is a TTY → [`run_tui`]: ratatui +
    ///    alternate screen.
    /// 3. otherwise → [`run_plain`]: line-by-line stdout/stderr.
    pub fn start(use_tui: bool, ipc_mode: bool) -> Result<(Self, Receiver<Decision>)> {
        let (event_tx, event_rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if ipc_mode {
                if let Err(e) = run_ipc(event_rx, decision_tx) {
                    // We CAN'T println — that's the wire. We CAN'T eprintln —
                    // the parent process may be capturing stderr for crash
                    // diagnostics, and a non-JSON line on stderr is at best
                    // confusing. Route through tracing, which in ipc_mode is
                    // wired to a file by `logging::init`.
                    tracing::error!("IPC backend failure: {e}");
                }
            } else {
                let is_tty = std::io::stdout().is_terminal();
                let active_tui = use_tui && is_tty;
                if active_tui {
                    if let Err(e) = run_tui(event_rx, decision_tx) {
                        eprintln!("TUI failure: {e}; falling back to plain mode is not possible mid-run");
                    }
                } else {
                    run_plain(event_rx, decision_tx);
                }
            }
        });
        Ok((
            Self {
                sender: event_tx,
                thread: Some(thread),
                finish_details: Mutex::new(FinishDetails::default()),
            },
            decision_rx,
        ))
    }

    pub fn header(&self, source: String, ipod: String, manifest: String) {
        let _ = self.sender.send(ProgressEvent::Header { source, ipod, manifest });
    }
    pub fn summary(&self, add: usize, modify: usize, metadata_only: usize, remove: usize, unchanged: usize, total_planned: usize) {
        let _ = self.sender.send(ProgressEvent::Summary { add, modify, metadata_only, remove, unchanged, total_planned });
    }
    /// Send the action plan to the TUI for interactive Review. The caller
    /// must then `recv()` on the decision channel to await the user's choice.
    pub fn review(&self, summary: ActionPlanSummary, no_delete: bool) {
        let _ = self.sender.send(ProgressEvent::Review { summary, no_delete });
    }
    /// Show a choice prompt in the TUI. Returns immediately; caller awaits
    /// the user's choice via the decision channel.
    pub fn prompt(&self, request: PromptRequest) {
        let _ = self.sender.send(ProgressEvent::Prompt(request));
    }
    /// Show a text-input form in the TUI. Returns immediately; caller awaits
    /// the user's reply via the decision channel.
    pub fn form(&self, request: FormRequest) {
        let _ = self.sender.send(ProgressEvent::Form(request));
    }
    /// Allocates a fresh prompt id. Caller uses it when building a PromptRequest
    /// (or FormRequest) and again when matching the response from the decision
    /// channel.
    pub fn next_prompt_id(&self) -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }
    pub fn track_start(&self, current: usize, total: usize, label: String) {
        let _ = self.sender.send(ProgressEvent::TrackStart { current, total, label });
    }
    pub fn track_done(&self) {
        let _ = self.sender.send(ProgressEvent::TrackDone);
    }
    pub fn log(&self, msg: impl Into<String>) {
        let _ = self.sender.send(ProgressEvent::Log(msg.into()));
    }
    pub fn error(&self, msg: impl Into<String>) {
        let _ = self.sender.send(ProgressEvent::Error(msg.into()));
    }
    /// Sends the terminal `Paused` event. Caller (main.rs) sends this before
    /// `finish(true)` so the wire/plain output carries the pause outcome.
    pub fn paused(&self) {
        let _ = self.sender.send(ProgressEvent::Paused);
    }

    /// Records that Task 4's auto-restore-from-backup path fired this run
    /// (iTunesDB failed to parse and was replaced from the session backup
    /// before the sync proceeded). Surfaced as `finish.db_restored: true`.
    /// Idempotent — safe to call more than once in the unlikely event both
    /// `open_with_auto_restore` call sites in `apply_loop::run` fire.
    pub fn note_db_restored(&self) {
        if let Ok(mut d) = self.finish_details.lock() {
            d.db_restored = true;
        }
    }

    /// Records the fit pass's (Task 8) final deferral rollup — whatever the
    /// end-of-run retry still couldn't fit. Surfaced as
    /// `finish.skipped_for_space`. Overwrites any previous value; callers
    /// are expected to call this at most once, with the final tally.
    pub fn note_skipped_for_space(&self, skipped: SkippedForSpace) {
        if let Ok(mut d) = self.finish_details.lock() {
            d.skipped_for_space = Some(skipped);
        }
    }

    /// Drains the channel and joins the worker thread. Call once at the end.
    ///
    /// Returns `Err` if the worker thread panicked (e.g. crossterm setup
    /// failure on an odd terminal); previously the panic was swallowed by
    /// `let _ = t.join()`, leaving the orchestrator silently writing to the
    /// iPod with no visible UI. Per-event `send` failures are still ignored
    /// (the channel is one-way and a closed channel is benign at teardown).
    ///
    /// If the thread doesn't reach a terminal state within `JOIN_DEADLINE`
    /// after we sent Finish, we force-exit the process with code 2 rather
    /// than wait indefinitely. The Phase 3.z gate produced a 2-hour zombie
    /// process this way: catastrophic Scenario 5 run finished applying its
    /// 1275 removes, but the TUI thread never returned — most likely
    /// crossterm's `LeaveAlternateScreen` or `disable_raw_mode` wedged on a
    /// Windows console handle after the gauge/panel rendering had already
    /// degraded visibly. Force-exit guarantees that "work done" leads to
    /// "process gone" inside a bounded window even if we don't fully
    /// understand the root cause.
    pub fn finish(mut self, success: bool) -> Result<()> {
        const JOIN_DEADLINE: Duration = Duration::from_secs(5);
        const POLL_INTERVAL: Duration = Duration::from_millis(50);

        let details = self.finish_details.lock().map(|d| d.clone()).unwrap_or_default();
        let _ = self.sender.send(ProgressEvent::Finish {
            success,
            skipped_for_space: details.skipped_for_space,
            artwork: details.artwork,
            db_restored: details.db_restored,
        });
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + JOIN_DEADLINE;
            while !t.is_finished() {
                if Instant::now() >= deadline {
                    eprintln!(
                        "WARN: TUI thread did not exit within {JOIN_DEADLINE:?} \
                         after Finish; force-exiting to avoid a zombie process. \
                         If this fires more than once, file a bug with terminal info."
                    );
                    std::process::exit(2);
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            // Thread is done; join is now non-blocking.
            t.join().map_err(|panic| {
                let msg = panic
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                anyhow!("TUI thread panicked: {msg}")
            })?;
        }
        Ok(())
    }
}

/// Plain mode: dump events as lines. Stdout for normal stuff, stderr for errors.
fn run_plain(rx: Receiver<ProgressEvent>, decision_tx: Sender<Decision>) {
    for event in rx {
        match event {
            ProgressEvent::Header { source, ipod, manifest } => {
                println!("Source  : {source}");
                println!("iPod    : {ipod}");
                println!("Manifest: {manifest}");
            }
            ProgressEvent::Summary { add, modify, metadata_only, remove, unchanged, .. } => {
                println!();
                println!("Action plan: add={add} modify={modify} metadata={metadata_only} remove={remove} unchanged={unchanged}");
            }
            ProgressEvent::Review { .. } => {
                // Non-TTY can't interactively review. The orchestrator should
                // have errored at startup if neither --dry-run nor --apply
                // was set; this is a safety net.
                eprintln!("ERROR: interactive Review is not supported in plain mode; \
                          pass --dry-run or --apply explicitly.");
                let _ = decision_tx.send(Decision::Review(ReviewDecision::Quit));
            }
            ProgressEvent::Prompt(req) => {
                eprintln!("ERROR: interactive prompt is not supported in plain mode.");
                eprintln!("  {}", req.message);
                // Send an out-of-range choice so await_prompt's
                // outcomes.get(choice).unwrap_or(Abort) falls back to Abort —
                // choice: 0 would have mapped to whichever PromptOutcome the
                // caller put first, triggering retries (infinite loop) or
                // destructive side-effects (e.g. config reset).
                let _ = decision_tx.send(Decision::Prompt { id: req.id, choice: usize::MAX });
            }
            ProgressEvent::Form(req) => {
                eprintln!("ERROR: interactive form is not supported in plain mode.");
                eprintln!("  {}", req.label);
                let _ = decision_tx.send(Decision::Form { id: req.id, value: None });
            }
            ProgressEvent::TrackStart { current, total, label } => {
                println!("[{current}/{total}] {label}");
            }
            ProgressEvent::TrackDone => {}  // already printed at start
            ProgressEvent::Log(s) => println!("{s}"),
            ProgressEvent::Error(s) => eprintln!("ERROR: {s}"),
            // success bool is ignored in plain mode (the process exit code
            // conveys it; we don't print a banner either way). db_restored
            // is skipped too — the on_restore closure already logged it.
            ProgressEvent::Finish { skipped_for_space, .. } => {
                if let Some(s) = &skipped_for_space {
                    println!("{}", s.describe());
                }
                break;
            }
            ProgressEvent::Paused => {
                println!("Paused. Completed tracks were saved.");
                break;
            }
        }
    }
}

/// JSON-over-stdio backend for `--ipc-mode`. See `docs/ipc-protocol.md`.
///
/// Wire model:
/// - Stdout carries events: one `IpcEvent` per `\n`-terminated line, UTF-8.
///   Explicit `flush()` after every line because Rust's stdout is
///   block-buffered when attached to a pipe; without the flush the UI would
///   wait indefinitely.
/// - Stdin carries commands: one `IpcCommand` per line. A dedicated reader
///   thread parses each line and pushes a translated `Decision` into the
///   same channel the TUI uses, so the orchestrator code path is identical.
///
/// stdout MUST stay clean — no `println!` from anywhere, no tracing on stdout.
/// `logging::init` routes tracing to a file in this mode; errors from this
/// function go through `tracing::error!` (called by `Progress::start`), never
/// to stdout.
fn run_ipc(rx: Receiver<ProgressEvent>, decision_tx: Sender<Decision>) -> Result<()> {
    use crate::ipc::{IpcCommand, IpcEvent, PROTOCOL_VERSION};
    use std::io::{BufRead, BufReader};

    // 1. Handshake. MUST be the first byte on stdout so the UI can validate
    //    protocol compatibility before sending anything (see ipc-protocol §1).
    let hello = IpcEvent::Hello {
        protocol_version: PROTOCOL_VERSION.to_string(),
        core_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    write_ipc_event(&hello).context("ipc: hello write failed")?;

    // 2. Stdin reader. Lives in its own thread so the main event loop never
    //    blocks waiting for the UI. Exits on EOF (parent closed our stdin —
    //    treat as a graceful shutdown signal), on parse-loop error, or when
    //    the decision channel is closed (orchestrator gone).
    let cmd_tx = decision_tx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let reader = BufReader::new(stdin.lock());
        tracing::info!("ipc: stdin reader thread started");
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    tracing::info!("ipc: stdin read returned error (likely EOF / pipe broken): {e}");
                    break;
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            tracing::info!("ipc: received line: {trimmed}");
            match serde_json::from_str::<IpcCommand>(trimmed) {
                Ok(cmd) => {
                    tracing::info!("ipc: parsed command: {cmd:?}");
                    if let Some(decision) = cmd.to_decision() {
                        tracing::info!("ipc: dispatching decision: {decision:?}");
                        if cmd_tx.send(decision).is_err() {
                            tracing::info!("ipc: decision channel closed, exiting reader");
                            break;
                        }
                    } else {
                        tracing::info!("ipc: command has no decision payload (e.g. Start); silently consumed");
                    }
                }
                Err(e) => {
                    // Per ipc-protocol §2: malformed input is not fatal.
                    // Log and continue reading.
                    tracing::warn!("ipc: unparseable command line {trimmed:?}: {e}");
                }
            }
        }
        tracing::info!("ipc: stdin reader thread exiting");
    });

    // 3. Event loop: drain internal channel, write each event as a JSON line.
    //    Loop ends when we see Finish or Paused (both terminal) or the
    //    channel closes (sender dropped without sending either — should not
    //    happen in practice).
    tracing::info!("ipc: event loop entering");
    let mut eta = EtaEstimator::new();
    for event in rx {
        let is_terminal = matches!(
            event,
            ProgressEvent::Finish { .. } | ProgressEvent::Paused
        );
        if matches!(event, ProgressEvent::TrackDone) {
            eta.record_track_done();
        }
        if let Some(mut ipc_event) = IpcEvent::from_progress(&event) {
            if let crate::ipc::IpcEvent::TrackStart { current, total, eta_secs, .. } = &mut ipc_event {
                *eta_secs = eta.eta_secs(*current, *total);
            }
            tracing::info!("ipc: emitting event: {ipc_event:?}");
            write_ipc_event(&ipc_event).context("ipc: event write failed")?;
        }
        if is_terminal {
            tracing::info!("ipc: received terminal event, exiting event loop");
            break;
        }
    }
    tracing::info!("ipc: event loop exited (channel closed or terminal event seen)");

    Ok(())
}

/// Write one `IpcEvent` as a single newline-terminated JSON line on stdout,
/// then flush. Flushing every line is required per `docs/ipc-protocol.md` §3:
/// stdout is block-buffered when piped, and an unflushed write would leave
/// the UI hanging. Cost is negligible at our event rate (~10/s peak).
fn write_ipc_event(event: &crate::ipc::IpcEvent) -> Result<()> {
    use std::io::Write;
    let line = serde_json::to_string(event).context("ipc: serialize event")?;
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    writeln!(locked, "{line}").context("ipc: stdout write")?;
    locked.flush().context("ipc: stdout flush")?;
    Ok(())
}

struct TuiState {
    source: String,
    ipod: String,
    manifest: String,
    add: usize,
    modify: usize,
    remove: usize,
    unchanged: usize,
    total_planned: usize,
    done: usize,
    current_label: String,
    current_index: usize,
    current_total: usize,
    started_at: Instant,
    log_tail: VecDeque<String>,
    /// When Some, we're in the Review state — pause the normal apply-progress
    /// rendering and show the action plan + key hints instead.
    review: Option<ReviewState>,
    /// When Some, render a modal choice prompt instead of the normal screen.
    prompt: Option<PromptRequest>,
    /// When Some, render a text-input form instead of the normal screen.
    form: Option<FormState>,
    /// Flag set by the form keypress handler when it has just sent a decision,
    /// so the post-handler can clear `form` after dispatch (avoids holding a
    /// mutable borrow on `form` across the send).
    form_done: bool,
}

struct ReviewState {
    summary: ActionPlanSummary,
    no_delete: bool,
}

struct FormState {
    request: FormRequest,
    input: String,
}

impl TuiState {
    fn new() -> Self {
        Self {
            source: String::new(), ipod: String::new(), manifest: String::new(),
            add: 0, modify: 0, remove: 0, unchanged: 0, total_planned: 0,
            done: 0, current_label: String::new(),
            current_index: 0, current_total: 0,
            started_at: Instant::now(),
            log_tail: VecDeque::with_capacity(LOG_TAIL_CAPACITY),
            review: None,
            prompt: None,
            form: None,
            form_done: false,
        }
    }

    fn push_log(&mut self, line: String) {
        if self.log_tail.len() == LOG_TAIL_CAPACITY {
            self.log_tail.pop_front();
        }
        self.log_tail.push_back(line);
    }

    fn fraction(&self) -> f64 {
        if self.total_planned == 0 { 0.0 } else {
            (self.done as f64) / (self.total_planned as f64)
        }
    }

    fn eta(&self) -> Option<Duration> {
        if self.done == 0 || self.total_planned == 0 { return None; }
        let elapsed = self.started_at.elapsed();
        let per_track = elapsed.as_secs_f64() / (self.done as f64);
        let remaining = self.total_planned.saturating_sub(self.done);
        if remaining == 0 { return None; }
        Some(Duration::from_secs_f64(per_track * remaining as f64))
    }
}

const LOG_TAIL_CAPACITY: usize = 12;

fn run_tui(rx: Receiver<ProgressEvent>, decision_tx: Sender<Decision>) -> Result<()> {
    let mut state = TuiState::new();
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut finished = false;
    while !finished {
        // Drain any pending events without blocking; cap per-frame so a flood
        // doesn't starve the redraw.
        for _ in 0..32 {
            match rx.try_recv() {
                Ok(event) => apply_event(&mut state, event, &mut finished),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => { finished = true; break; }
            }
        }

        terminal.draw(|f| render(f, &state))?;

        // Allow Ctrl+C / 'q' to bail out of the TUI (caller still owns sync flow).
        if crossterm::event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = crossterm::event::read()? {
                // Windows crossterm fires key events on Press AND Release by
                // default; without this filter, every keystroke double-fires
                // ('s' becomes "ss" in form inputs, '1' picks option twice in
                // prompts). KeyEventKind::Repeat is also filtered to keep
                // held-down keys from runaway-repeating in our discrete-action
                // dispatch (we'd rather the user press again deliberately).
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }
                // Dispatch order matches render precedence: form first, then
                // prompt, then review, then the global 'q' shortcut.
                if let Some(form) = state.form.as_mut() {
                    handle_form_key(form, key, &decision_tx, &mut state.form_done);
                } else if let Some(prompt) = state.prompt.as_ref() {
                    let prompt = prompt.clone();
                    handle_prompt_key(&mut state, key, prompt, &decision_tx);
                } else if state.review.is_some() {
                    handle_review_key(&mut state, key, &decision_tx);
                } else if key.code == KeyCode::Char('q') {
                    // 'q' is a request-stop; we just exit the draw loop. The sync
                    // thread keeps running until it next sends an event and finds
                    // the channel closed.
                    finished = true;
                }
            }
            // Clear form state if it was just submitted/aborted.
            if state.form_done {
                state.form = None;
                state.form_done = false;
            }
        }
    }

    // Teardown.
    crossterm::execute!(terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

fn handle_review_key(
    state: &mut TuiState,
    key: crossterm::event::KeyEvent,
    decision_tx: &Sender<Decision>,
) {
    let review = match state.review.as_mut() {
        Some(r) => r,
        None => return,
    };
    match key.code {
        KeyCode::Char('a') => {
            let _ = decision_tx.send(Decision::Review(ReviewDecision::Apply { no_delete: review.no_delete }));
            // Exit Review state so subsequent track progress can render.
            state.review = None;
        }
        KeyCode::Char('d') => {
            let _ = decision_tx.send(Decision::Review(ReviewDecision::DryRun));
            // Caller will send Finish next; clear review so a stray second
            // recv() can't re-fire this branch with stale state.
            state.review = None;
        }
        KeyCode::Char('t') => {
            review.no_delete = !review.no_delete;
            // Re-render happens on next loop iteration; no decision sent yet.
        }
        KeyCode::Char('q') => {
            let _ = decision_tx.send(Decision::Review(ReviewDecision::Quit));
            state.review = None;
        }
        _ => {}
    }
}

fn handle_prompt_key(
    state: &mut TuiState,
    key: crossterm::event::KeyEvent,
    prompt: PromptRequest,
    decision_tx: &Sender<Decision>,
) {
    // Number keys '1'..='9' pick options[0..=8]. Other keys are ignored
    // (callers should always include an explicit "Abort" option rather than
    // relying on a magic Esc key, since the prompt is the only thing the user
    // can interact with at this moment).
    if let KeyCode::Char(c) = key.code {
        if let Some(digit) = c.to_digit(10) {
            // Options are 1-indexed in the on-screen UI ([1] foo, [2] bar).
            // Ignore '0' — previously `0.saturating_sub(1) == 0` mapped to
            // option[0], so a typo'd '0' could fire a destructive option
            // (e.g. config Reset, retry on a fixed problem-state).
            if digit == 0 {
                return;
            }
            let choice = (digit as usize) - 1;
            if choice < prompt.options.len() {
                let _ = decision_tx.send(Decision::Prompt { id: prompt.id, choice });
                state.prompt = None; // exit prompt state; caller's next event takes over
            }
        }
    }
}

fn handle_form_key(
    form: &mut FormState,
    key: crossterm::event::KeyEvent,
    decision_tx: &Sender<Decision>,
    form_done: &mut bool,
) {
    use crossterm::event::KeyModifiers;
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
            let _ = decision_tx.send(Decision::Form { id: form.request.id, value: None });
            *form_done = true;
        }
        (KeyCode::Enter, _) => {
            let trimmed = form.input.trim();
            if !trimmed.is_empty() {
                let _ = decision_tx.send(Decision::Form {
                    id: form.request.id,
                    value: Some(trimmed.to_string()),
                });
                *form_done = true;
            }
            // Empty input: ignore Enter (require either text or Esc).
        }
        (KeyCode::Backspace, _) => { form.input.pop(); }
        (KeyCode::Char(c), _) => { form.input.push(c); }
        _ => {}
    }
}

fn apply_event(state: &mut TuiState, event: ProgressEvent, finished: &mut bool) {
    match event {
        ProgressEvent::Header { source, ipod, manifest } => {
            state.source = source; state.ipod = ipod; state.manifest = manifest;
        }
        ProgressEvent::Summary { add, modify, metadata_only: _, remove, unchanged, total_planned } => {
            state.add = add; state.modify = modify; state.remove = remove;
            state.unchanged = unchanged; state.total_planned = total_planned;
            state.started_at = Instant::now();  // reset clock for ETA
        }
        ProgressEvent::Review { summary, no_delete } => {
            state.review = Some(ReviewState { summary, no_delete });
        }
        ProgressEvent::Prompt(req) => {
            state.prompt = Some(req);
        }
        ProgressEvent::Form(req) => {
            state.form = Some(FormState {
                input: req.initial.clone(),
                request: req,
            });
        }
        ProgressEvent::TrackStart { current, total, label } => {
            state.current_index = current; state.current_total = total;
            state.current_label = label;
        }
        ProgressEvent::TrackDone => { state.done += 1; }
        ProgressEvent::Log(s) => state.push_log(s),
        ProgressEvent::Error(s) => state.push_log(format!("ERROR: {s}")),
        // TUI doesn't surface success/failure directly here — the process
        // exit code carries it. We just tear down the draw loop. One more
        // `terminal.draw` runs after this (see `run_tui`'s loop body) before
        // teardown, so a log line pushed here does get one frame of
        // visibility.
        ProgressEvent::Finish { skipped_for_space, .. } => {
            if let Some(s) = &skipped_for_space {
                state.push_log(s.describe());
            }
            *finished = true;
        }
        ProgressEvent::Paused => {
            state.push_log("Paused. Completed tracks were saved.".to_string());
            *finished = true;
        }
    }
}

fn render(f: &mut ratatui::Frame, state: &TuiState) {
    // Precedence: form > prompt > review > normal progress. A form is the
    // most modal of the three (Phase 3.z wizard) and should win even if a
    // prompt or review state happens to be set under it.
    if let Some(form) = &state.form {
        render_form(f, form);
        return;
    }
    if let Some(prompt) = &state.prompt {
        render_prompt(f, state, prompt);
        return;
    }
    if let Some(review) = &state.review {
        render_review(f, state, review);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // header
            Constraint::Length(4),  // progress
            Constraint::Length(3),  // current track
            Constraint::Min(5),     // log tail
        ])
        .split(f.area());

    let header_text = vec![
        Line::from(format!("Source  : {}", state.source)),
        Line::from(format!("iPod    : {}", state.ipod)),
        Line::from(format!("Manifest: {}", state.manifest)),
    ];
    f.render_widget(
        Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).title(format!(" {} ", crate::DISPLAY_NAME))),
        chunks[0],
    );

    // Until the Summary event lands (i.e. between user pressing Apply and the
    // orchestrator computing total_planned), total_planned is 0 and the gauge
    // would otherwise render the confusing "0/0 (0%)". Show "(preparing...)"
    // in that window so the user knows the apply phase is initializing.
    let progress_label = if state.total_planned == 0 {
        "(preparing...)".to_string()
    } else {
        let pct = (state.fraction() * 100.0) as u16;
        let eta = state.eta()
            .map(|d| format!(" ETA {}", format_duration(d)))
            .unwrap_or_default();
        format!("{}/{} ({}%){}", state.done, state.total_planned, pct, eta)
    };
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" progress "))
            .ratio(state.fraction().clamp(0.0, 1.0))
            .label(progress_label),
        chunks[1],
    );

    let current = if state.current_total > 0 {
        format!("[{}/{}] {}", state.current_index, state.current_total, state.current_label)
    } else {
        "(idle)".to_string()
    };
    f.render_widget(
        Paragraph::new(current).block(Block::default().borders(Borders::ALL).title(" current ")),
        chunks[2],
    );

    let log_items: Vec<ListItem> = state.log_tail.iter()
        .map(|l| ListItem::new(Line::from(l.as_str())))
        .collect();
    f.render_widget(
        List::new(log_items)
            .block(Block::default().borders(Borders::ALL).title(" log "))
            .style(Style::default().add_modifier(Modifier::DIM)),
        chunks[3],
    );
}

fn render_review(f: &mut ratatui::Frame, state: &TuiState, review: &ReviewState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // header (source/ipod/manifest + no_delete state)
            Constraint::Length(8), // plan
            Constraint::Min(3),    // key legend
        ])
        .split(f.area());

    let no_delete_str = if review.no_delete { "ON (Removes skipped)" } else { "OFF" };
    let header_text = vec![
        Line::from(format!("Source     : {}", state.source)),
        Line::from(format!("iPod       : {}", state.ipod)),
        Line::from(format!("Manifest   : {}", state.manifest)),
        Line::from(format!("--no-delete: {no_delete_str}")),
    ];
    f.render_widget(
        Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).title(format!(" {} — review ", crate::DISPLAY_NAME))),
        chunks[0],
    );

    let effective_remove = if review.no_delete { 0 } else { review.summary.remove };
    let plan_text = vec![
        Line::from(format!("Add          : {}", review.summary.add)),
        Line::from(format!("Modify       : {}", review.summary.modify)),
        Line::from(format!("MetadataOnly : {}", review.summary.metadata_only)),
        Line::from(format!(
            "Remove       : {}{}",
            effective_remove,
            if review.no_delete && review.summary.remove > 0 {
                format!(" ({} suppressed by --no-delete)", review.summary.remove)
            } else {
                String::new()
            },
        )),
        Line::from(format!("Unchanged    : {}", review.summary.unchanged)),
        Line::from(""),
        Line::from(format!(
            "Total to apply: {}",
            review.summary.add + review.summary.modify + review.summary.metadata_only + effective_remove
        )),
    ];
    f.render_widget(
        Paragraph::new(plan_text)
            .block(Block::default().borders(Borders::ALL).title(" action plan ")),
        chunks[1],
    );

    let legend = "[a] apply   [d] dry-run (exit)   [t] toggle --no-delete   [q] quit";
    f.render_widget(
        Paragraph::new(legend)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL).title(" keys ")),
        chunks[2],
    );
}

fn render_prompt(f: &mut ratatui::Frame, state: &TuiState, prompt: &PromptRequest) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),       // header
            Constraint::Min(5),          // message
            Constraint::Length(4 + prompt.options.len() as u16),  // options
        ])
        .split(f.area());

    let header_text = vec![
        Line::from(format!("Source     : {}", state.source)),
        Line::from(format!("iPod       : {}", state.ipod)),
    ];
    f.render_widget(
        Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).title(format!(" {} ", crate::DISPLAY_NAME))),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(prompt.message.as_str())
            .style(Style::default().add_modifier(Modifier::BOLD))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" attention ")),
        chunks[1],
    );

    let mut option_lines: Vec<Line> = prompt.options
        .iter()
        .enumerate()
        .map(|(i, opt)| Line::from(format!("[{}] {opt}", i + 1)))
        .collect();
    option_lines.push(Line::from(""));
    option_lines.push(Line::from("Press the number key for your choice."));
    f.render_widget(
        Paragraph::new(option_lines)
            .block(Block::default().borders(Borders::ALL).title(" options ")),
        chunks[2],
    );
}

fn render_form(f: &mut ratatui::Frame, form: &FormState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),  // label / header
            Constraint::Length(3),  // input box
            Constraint::Length(3),  // hint (1 line + borders; was Min(3) which ate the rest)
            Constraint::Min(0),     // spacer to absorb the rest of the screen
        ])
        .split(f.area());

    f.render_widget(
        Paragraph::new(form.request.label.as_str())
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(format!(" {} ", crate::DISPLAY_NAME))),
        chunks[0],
    );

    // Trailing underscore stands in for a visible cursor — the real terminal
    // cursor is hidden during the alternate-screen TUI session, so without
    // this the empty input box looks dead.
    let input_display = format!("{}_", form.input);
    f.render_widget(
        Paragraph::new(input_display)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL).title(" input ")),
        chunks[1],
    );

    f.render_widget(
        Paragraph::new(form.request.hint.as_str())
            .style(Style::default().add_modifier(Modifier::DIM))
            .block(Block::default().borders(Borders::ALL).title(" keys ")),
        chunks[2],
    );
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_estimator_none_until_a_track_completes() {
        let e = EtaEstimator::new_at(std::time::Instant::now());
        // No completed tracks yet → no estimate.
        assert_eq!(e.eta_secs(1, 10), None);
    }

    #[test]
    fn eta_estimator_projects_from_average_after_completions() {
        let start = std::time::Instant::now() - std::time::Duration::from_secs(20);
        let mut e = EtaEstimator::new_at(start);
        // 4 tracks done over ~20s → ~5s/track. 6 remaining → ~30s.
        for _ in 0..4 { e.record_track_done(); }
        let eta = e.eta_secs(5, 10).expect("estimate after completions");
        assert!((25..=35).contains(&eta), "eta {eta} not ~30s");
    }

    #[test]
    fn eta_estimator_none_when_nothing_remains() {
        let start = std::time::Instant::now() - std::time::Duration::from_secs(10);
        let mut e = EtaEstimator::new_at(start);
        for _ in 0..10 { e.record_track_done(); }
        assert_eq!(e.eta_secs(10, 10), None, "no remaining tracks → no eta");
    }
}
