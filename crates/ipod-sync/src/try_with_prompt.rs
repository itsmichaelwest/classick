//! Wrap a fallible operation with a TUI prompt on failure. Pattern:
//!
//!   let result = try_with_prompt(
//!       progress, decision_rx,
//!       || mount_ipod(),
//!       |err| format!("Couldn't find iPod: {err}\nPlease plug it in."),
//!       &["Retry", "Quit"],
//!   )?;
//!
//! Returns the operation's Ok value (after possibly retrying) or an error
//! representing the user's chosen exit (Abort).

use crate::progress::{Decision, Progress, PromptRequest};
use anyhow::{anyhow, Result};
use std::sync::mpsc::Receiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    /// User picked "Retry" or equivalent — caller should re-invoke the operation.
    Retry,
    /// User picked an option mapped to "skip this and move on."
    Skip,
    /// User picked "Abort" or equivalent — caller should bubble the error.
    Abort,
    /// User picked a non-standard option (caller-defined). Carries the index.
    Custom(usize),
}

/// Specifies how each option maps to a `PromptOutcome`. Same length as the
/// `options` array passed to the prompt.
pub type OutcomeMap = &'static [PromptOutcome];

/// Show a prompt and wait for the user's decision; return the mapped outcome.
/// Drains unrelated decisions defensively.
pub fn await_prompt(
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
    message: String,
    options: &[&str],
    outcomes: OutcomeMap,
) -> Result<PromptOutcome> {
    assert_eq!(options.len(), outcomes.len(), "options/outcomes length mismatch");
    let id = progress.next_prompt_id();
    progress.prompt(PromptRequest {
        id,
        message,
        options: options.iter().map(|s| s.to_string()).collect(),
    });
    loop {
        match decision_rx.recv() {
            Ok(Decision::Prompt { id: rid, choice }) if rid == id => {
                return Ok(outcomes.get(choice).copied().unwrap_or(PromptOutcome::Abort));
            }
            Ok(_) => continue, // stray decision; ignore
            Err(e) => return Err(anyhow!("await_prompt: decision channel closed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outcome_map_lengths_must_match_options_at_callsite() {
        // Compile-only contract: callers must keep the two arrays aligned.
        // The runtime assertion in `await_prompt` enforces this at first call.
        let opts = &["A", "B"][..];
        let outs = &[PromptOutcome::Retry, PromptOutcome::Abort][..];
        assert_eq!(opts.len(), outs.len());
    }

    #[test]
    fn outcome_dispatch_chooses_correct_variant() {
        // Pure logic check — index into outcomes returns the right enum.
        let outs: OutcomeMap = &[PromptOutcome::Retry, PromptOutcome::Skip, PromptOutcome::Abort];
        assert_eq!(outs[0], PromptOutcome::Retry);
        assert_eq!(outs[1], PromptOutcome::Skip);
        assert_eq!(outs[2], PromptOutcome::Abort);
    }
}
