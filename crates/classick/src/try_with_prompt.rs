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
use std::time::Duration;

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
    assert_eq!(
        options.len(),
        outcomes.len(),
        "options/outcomes length mismatch"
    );
    let id = progress.next_prompt_id();
    progress.prompt(PromptRequest {
        id,
        message,
        options: options.iter().map(|s| s.to_string()).collect(),
    });
    loop {
        match decision_rx.recv() {
            Ok(Decision::Prompt { id: rid, choice }) if rid == id => {
                return Ok(outcomes
                    .get(choice)
                    .copied()
                    .unwrap_or(PromptOutcome::Abort));
            }
            Ok(_) => continue, // stray decision; ignore
            Err(e) => return Err(anyhow!("await_prompt: decision channel closed: {e}")),
        }
    }
}

/// Run `op`, retrying on `Err` after each delay in `backoff` (so a 3-element
/// backoff means up to 3 retries = 4 total attempts). Returns the first `Ok`,
/// or the LAST error after the schedule is exhausted. Use ONLY for transient
/// I/O (iPod writes); deterministic failures (e.g. a bad transcode) must not
/// be retried.
pub fn retry_transient<T>(
    backoff: &[Duration],
    mut op: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut attempt = 0usize;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= backoff.len() {
                    return Err(e);
                }
                let delay = backoff[attempt];
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                attempt += 1;
            }
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
        let outs: OutcomeMap = &[
            PromptOutcome::Retry,
            PromptOutcome::Skip,
            PromptOutcome::Abort,
        ];
        assert_eq!(outs[0], PromptOutcome::Retry);
        assert_eq!(outs[1], PromptOutcome::Skip);
        assert_eq!(outs[2], PromptOutcome::Abort);
    }
}

#[cfg(test)]
mod retry_tests {
    use super::retry_transient;
    use anyhow::anyhow;
    use std::cell::Cell;
    use std::time::Duration;

    const NODELAY: [Duration; 3] = [Duration::ZERO, Duration::ZERO, Duration::ZERO];

    #[test]
    fn succeeds_first_try_without_retrying() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || {
            calls.set(calls.get() + 1);
            Ok(7)
        });
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retries_then_succeeds() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(anyhow!("transient"))
            } else {
                Ok(42)
            }
        });
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn returns_last_error_after_exhaustion() {
        let calls = Cell::new(0);
        let r: anyhow::Result<i32> = retry_transient(&NODELAY, || {
            calls.set(calls.get() + 1);
            Err(anyhow!("fail {}", calls.get()))
        });
        assert_eq!(calls.get(), 4); // 1 initial + 3 retries
        assert_eq!(r.unwrap_err().to_string(), "fail 4");
    }
}
