//! Progress reporting: ratatui TUI when stdout is a TTY + --no-tui is off,
//! plain log lines otherwise. Main thread sends events; a dedicated thread
//! drains the channel and renders.

use anyhow::Result;
use crossterm::event::{Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
/// the main thread via the decision channel.
#[derive(Debug, Clone, Copy)]
pub enum ReviewDecision {
    /// Proceed with the apply loop. `no_delete` carries the user's possibly-toggled value.
    Apply { no_delete: bool },
    /// Skip the apply loop and exit cleanly (effectively a one-shot --dry-run).
    DryRun,
    /// Exit cleanly without applying anything or saving any state.
    Quit,
}

/// Events sent from the main thread to the progress thread.
pub enum ProgressEvent {
    Header { source: String, ipod: String, manifest: String },
    Summary { add: usize, modify: usize, remove: usize, unchanged: usize, total_planned: usize },
    Review { summary: ActionPlanSummary, no_delete: bool },
    TrackStart { current: usize, total: usize, label: String },
    TrackDone,
    Log(String),
    Error(String),
    Finish,
}

pub struct Progress {
    sender: Sender<ProgressEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Progress {
    pub fn start(use_tui: bool) -> Result<(Self, Receiver<ReviewDecision>)> {
        let is_tty = std::io::stdout().is_terminal();
        let active_tui = use_tui && is_tty;
        let (event_tx, event_rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if active_tui {
                if let Err(e) = run_tui(event_rx, decision_tx) {
                    eprintln!("TUI failure: {e}; falling back to plain mode is not possible mid-run");
                }
            } else {
                run_plain(event_rx, decision_tx);
            }
        });
        Ok((
            Self { sender: event_tx, thread: Some(thread) },
            decision_rx,
        ))
    }

    pub fn header(&self, source: String, ipod: String, manifest: String) {
        let _ = self.sender.send(ProgressEvent::Header { source, ipod, manifest });
    }
    pub fn summary(&self, add: usize, modify: usize, remove: usize, unchanged: usize, total_planned: usize) {
        let _ = self.sender.send(ProgressEvent::Summary { add, modify, remove, unchanged, total_planned });
    }
    /// Send the action plan to the TUI for interactive Review. The caller
    /// must then `recv()` on the decision channel to await the user's choice.
    pub fn review(&self, summary: ActionPlanSummary, no_delete: bool) {
        let _ = self.sender.send(ProgressEvent::Review { summary, no_delete });
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

    /// Drains the channel and joins the worker thread. Call once at the end.
    pub fn finish(mut self) {
        let _ = self.sender.send(ProgressEvent::Finish);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Plain mode: dump events as lines. Stdout for normal stuff, stderr for errors.
fn run_plain(rx: Receiver<ProgressEvent>, decision_tx: Sender<ReviewDecision>) {
    for event in rx {
        match event {
            ProgressEvent::Header { source, ipod, manifest } => {
                println!("Source  : {source}");
                println!("iPod    : {ipod}");
                println!("Manifest: {manifest}");
            }
            ProgressEvent::Summary { add, modify, remove, unchanged, .. } => {
                println!();
                println!("Action plan: add={add} modify={modify} remove={remove} unchanged={unchanged}");
            }
            ProgressEvent::Review { .. } => {
                // Non-TTY can't interactively review. The orchestrator should
                // have errored at startup if neither --dry-run nor --apply
                // was set; this is a safety net.
                eprintln!("ERROR: interactive Review is not supported in plain mode; \
                          pass --dry-run or --apply explicitly.");
                let _ = decision_tx.send(ReviewDecision::Quit);
            }
            ProgressEvent::TrackStart { current, total, label } => {
                println!("[{current}/{total}] {label}");
            }
            ProgressEvent::TrackDone => {}  // already printed at start
            ProgressEvent::Log(s) => println!("{s}"),
            ProgressEvent::Error(s) => eprintln!("ERROR: {s}"),
            ProgressEvent::Finish => break,
        }
    }
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
}

struct ReviewState {
    summary: ActionPlanSummary,
    no_delete: bool,
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

fn run_tui(rx: Receiver<ProgressEvent>, decision_tx: Sender<ReviewDecision>) -> Result<()> {
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
                if state.review.is_some() {
                    handle_review_key(&mut state, key, &decision_tx);
                } else if key.code == KeyCode::Char('q') {
                    // 'q' is a request-stop; we just exit the draw loop. The sync
                    // thread keeps running until it next sends an event and finds
                    // the channel closed.
                    finished = true;
                }
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
    decision_tx: &Sender<ReviewDecision>,
) {
    let review = match state.review.as_mut() {
        Some(r) => r,
        None => return,
    };
    match key.code {
        KeyCode::Char('a') => {
            let _ = decision_tx.send(ReviewDecision::Apply { no_delete: review.no_delete });
            // Exit Review state so subsequent track progress can render.
            state.review = None;
        }
        KeyCode::Char('d') => {
            let _ = decision_tx.send(ReviewDecision::DryRun);
            // Caller will send Finish next.
        }
        KeyCode::Char('t') => {
            review.no_delete = !review.no_delete;
            // Re-render happens on next loop iteration; no decision sent yet.
        }
        KeyCode::Char('q') => {
            let _ = decision_tx.send(ReviewDecision::Quit);
        }
        _ => {}
    }
}

fn apply_event(state: &mut TuiState, event: ProgressEvent, finished: &mut bool) {
    match event {
        ProgressEvent::Header { source, ipod, manifest } => {
            state.source = source; state.ipod = ipod; state.manifest = manifest;
        }
        ProgressEvent::Summary { add, modify, remove, unchanged, total_planned } => {
            state.add = add; state.modify = modify; state.remove = remove;
            state.unchanged = unchanged; state.total_planned = total_planned;
            state.started_at = Instant::now();  // reset clock for ETA
        }
        ProgressEvent::Review { summary, no_delete } => {
            state.review = Some(ReviewState { summary, no_delete });
        }
        ProgressEvent::TrackStart { current, total, label } => {
            state.current_index = current; state.current_total = total;
            state.current_label = label;
        }
        ProgressEvent::TrackDone => { state.done += 1; }
        ProgressEvent::Log(s) => state.push_log(s),
        ProgressEvent::Error(s) => state.push_log(format!("ERROR: {s}")),
        ProgressEvent::Finish => { *finished = true; }
    }
}

fn render(f: &mut ratatui::Frame, state: &TuiState) {
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
            .block(Block::default().borders(Borders::ALL).title(" ipod-sync ")),
        chunks[0],
    );

    let pct = (state.fraction() * 100.0) as u16;
    let eta = state.eta()
        .map(|d| format!(" ETA {}", format_duration(d)))
        .unwrap_or_default();
    let progress_label = format!("{}/{} ({}%){}", state.done, state.total_planned, pct, eta);
    let plan_line = Line::from(vec![
        Span::raw(format!(
            "add={} modify={} remove={} unchanged={}",
            state.add, state.modify, state.remove, state.unchanged
        )),
    ]);
    let progress_lines = vec![plan_line];
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

    let _ = progress_lines;  // silence unused-warning if future refactor drops plan_line
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
            .block(Block::default().borders(Borders::ALL).title(" ipod-sync — review ")),
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
