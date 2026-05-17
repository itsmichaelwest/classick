//! First-launch source picker. Runs as a self-contained ratatui app BEFORE
//! the main Progress UI starts. Prompts for the source library path, writes
//! the result to %APPDATA%\ipod-sync\config.toml, and returns the path.
//!
//! Used by main.rs when no source is set via CLI flag, env var, or config file.

use crate::config_file::{self, PersistedConfig};
use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::path::PathBuf;

/// Launch the source-picker wizard. On Enter with a non-empty path, saves the
/// path to the persisted config and returns it. On Esc or Ctrl+C, returns an
/// error indicating the user aborted setup.
pub fn run() -> Result<PathBuf> {
    let mut input = String::new();
    let mut terminal = setup_terminal()?;

    let result: Result<PathBuf> = loop {
        if let Err(e) = terminal.draw(|f| render(f, &input)) {
            break Err(anyhow!("wizard render: {e}"));
        }

        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    | (KeyCode::Esc, _) => {
                        break Err(anyhow!("setup wizard aborted"));
                    }
                    (KeyCode::Enter, _) => {
                        let trimmed = input.trim();
                        if trimmed.is_empty() {
                            continue; // ignore empty-input Enter
                        }
                        break Ok(PathBuf::from(trimmed));
                    }
                    (KeyCode::Backspace, _) => {
                        input.pop();
                    }
                    (KeyCode::Char(c), _) => {
                        input.push(c);
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(e) => break Err(anyhow!("wizard read: {e}")),
        }
    };

    teardown_terminal(&mut terminal)?;
    let chosen = result?;

    // Persist immediately. Subsequent ipod-sync invocations will read this
    // value via config_file::load — no need to also set the env var.
    let config_path = config_file::default_path()?;
    let existing = config_file::load(&config_path)?.unwrap_or_default();
    let updated = PersistedConfig { source: Some(chosen.clone()), ..existing };
    config_file::save(&config_path, &updated)?;

    Ok(chosen)
}

fn setup_terminal() -> Result<ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    Ok(ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?)
}

fn teardown_terminal(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

fn render(f: &mut ratatui::Frame, input: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // header
            Constraint::Length(3), // input box
            Constraint::Min(3),    // hint
        ])
        .split(f.area());

    let header = vec![
        Line::from("ipod-sync — first-launch setup"),
        Line::from(""),
        Line::from("Enter the path to your FLAC source library. For an SMB share use UNC notation"),
        Line::from(r"(e.g. \\server\music). The path is saved to %APPDATA%\ipod-sync\config.toml."),
    ];
    f.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL).title(" setup ")),
        chunks[0],
    );

    f.render_widget(
        Paragraph::new(input)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL).title(" source path ")),
        chunks[1],
    );

    let hint = "Enter to save and continue   Esc or Ctrl+C to abort";
    f.render_widget(
        Paragraph::new(hint)
            .style(Style::default().add_modifier(Modifier::DIM))
            .block(Block::default().borders(Borders::ALL).title(" keys ")),
        chunks[2],
    );
}
