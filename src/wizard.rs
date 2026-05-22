//! First-launch source picker — saves the chosen path to
//! %APPDATA%\ipod-sync\config.toml and returns it.
//!
//! Phase 3.z: the actual TUI rendering lives in `progress::Form` now; this
//! module is a thin caller that bundles the prompt config + persistence.

use crate::config_file::{self, PersistedConfig};
use crate::progress::{Decision, FormRequest, Progress};
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// Show the wizard via the running Progress instance. Waits on the decision
/// channel for the user's text input, persists the chosen source path into
/// `%APPDATA%\ipod-sync\config.toml`, and returns it.
pub fn run(progress: &Progress, decision_rx: &Receiver<Decision>) -> Result<PathBuf> {
    let id = progress.next_prompt_id();
    progress.form(FormRequest {
        id,
        label: "ipod-sync — first-launch setup\n\
                Enter the path to your FLAC source library (UNC like \\\\server\\music)."
            .to_string(),
        initial: String::new(),
        hint: "Enter to save and continue   Esc or Ctrl+C to abort".to_string(),
    });

    // Drain decisions until we get our Form reply.
    let value = loop {
        match decision_rx.recv() {
            Ok(Decision::Form { id: rid, value }) if rid == id => break value,
            Ok(other) => {
                // Defensive: ignore stray decisions from earlier flows.
                tracing::warn!("wizard: ignoring unrelated decision {:?}", other);
            }
            Err(e) => return Err(anyhow!("wizard: decision channel closed: {e}")),
        }
    };

    let path_str = value.ok_or_else(|| anyhow!("setup wizard aborted"))?;
    let chosen = PathBuf::from(path_str);

    // Persist immediately. Subsequent ipod-sync invocations will read this
    // value via config_file::load — no need to also set the env var.
    let config_path = config_file::default_path()?;
    let existing = config_file::load(&config_path)?.unwrap_or_default();
    let updated = PersistedConfig {
        source: Some(chosen.clone()),
        ..existing
    };
    config_file::save(&config_path, &updated)?;

    Ok(chosen)
}
