//! Spawns the per-sync `classick.exe --ipc-mode --apply --ipod <drive>`
//! subprocess. Forwards every IpcEvent line to the broadcast channel so
//! UI clients see live progress. Counts per-track errors against
//! `Summary.total_planned` and bails (Cancel + 5s force-kill) when
//! `tracks_errored * 2 > total_planned`.

use crate::daemon::history::{SyncOutcome, SyncSummary};
use crate::ipc_daemon::DaemonEvent;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorOutcome {
    Completed { outcome: SyncOutcome, summary: Option<SyncSummary> },
    Aborted { reason: String, summary: Option<SyncSummary> },
    /// The subprocess emitted `{"type":"paused"}` (graceful drain +
    /// checkpoint) and then exited on its own. Distinct from `Aborted`:
    /// nothing failed, the user asked to stop, and a later `TriggerSync`
    /// resumes from the checkpoint via the normal diff.
    Paused { summary: Option<SyncSummary> },
}

/// Build the command to spawn. Extracted so tests can verify args
/// without actually spawning a process.
///
/// `kill_on_drop(true)` is load-bearing: if the orchestrator task is
/// dropped (daemon shutdown, runtime teardown, panic), tokio's Child
/// Drop runs TerminateProcess on the subprocess so it doesn't outlive
/// its parent. Without it, a graceful daemon Shutdown leaves an
/// orphaned sync subprocess transcoding for hours and holding ffmpeg
/// children — observed in the wild on 2026-05-24.
pub fn build_command(exe: &std::path::Path, drive: &str) -> Command {
    use crate::windows_proc::NoConsoleWindow;
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode")
        .arg("--apply")
        .arg("--ipod")
        .arg(drive)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        // Without CREATE_NO_WINDOW the sync subprocess gets its own
        // freshly-allocated console window (the daemon is windowless
        // when launched from the UI, so there's nothing to inherit).
        // That console would flash on screen at every Sync Now click.
        .no_console();
    cmd
}

/// Track running stats and decide if the >50% bail threshold tripped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FailureTracker {
    pub total_planned: usize,
    pub tracks_completed: usize,
    pub tracks_errored: usize,
}

impl FailureTracker {
    pub fn should_bail(&self) -> bool {
        self.total_planned > 0 && self.tracks_errored > 0
            && self.tracks_errored * 2 > self.total_planned
    }
}

/// Drive the spawned child to completion, until bail, until cancelled, or
/// until paused.
///
/// `cancel_rx` fires when the user clicks Cancel in the UI; the orchestrator
/// writes a Cancel command to the subprocess stdin and force-kills after 5s.
///
/// `pause_rx` fires when the user clicks Pause in the UI; the orchestrator
/// writes a Pause command to the subprocess stdin and, unlike cancel, does
/// NOT force-kill — pause is graceful, so the subprocess finishes draining
/// its in-flight window, checkpoints, emits `{"type":"paused"}`, and exits
/// on its own. The existing `bounded_kill` 5s grace remains only as the
/// cancel/bail backstop; a paused sync is trusted to exit cleanly.
///
/// `prompt_decisions_rx` carries `(id, choice)` pairs from
/// `DaemonCommand::DecidePrompt`; each is serialised as
/// `{"type":"prompt_decision","id":N,"choice":N}\n` and written to
/// the subprocess stdin. Without this channel, daemon-relayed
/// prompts (source-change safeguard, retry/skip/abort, etc.) would
/// block the sync indefinitely — the popover UI emits its reply via
/// DecidePrompt, the daemon ferries it here, and the orchestrator
/// hands it to the subprocess's prompt-waiting code.
pub async fn run(
    exe: PathBuf,
    drive: String,
    mut cancel_rx: oneshot::Receiver<()>,
    mut pause_rx: oneshot::Receiver<()>,
    mut prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
) -> Result<OrchestratorOutcome> {
    let mut cmd = build_command(&exe, &drive);
    let mut child = cmd.spawn().with_context(|| format!("spawn {}", exe.display()))?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let mut stdin = child.stdin.take().context("child stdin missing")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut tracker = FailureTracker::default();
    let mut last_summary: Option<SyncSummary> = None;
    let mut finish_success: Option<bool> = None;
    let mut paused = false;
    // Guards re-polling `pause_rx` after it has already fired once — a
    // oneshot::Receiver panics if polled again past Ready. Once the pause
    // is forwarded we just keep relaying lines until the subprocess exits.
    let mut pause_requested = false;

    loop {
        tokio::select! {
            line_res = reader.next_line() => {
                let line = match line_res? {
                    Some(l) => l,
                    None => break,  // subprocess closed stdout (normal completion or crash)
                };

                // Forward EVERY parseable line to the daemon's broadcast channel
                // so UI clients see live sync progress. Wrapping the raw line in
                // a SyncEvent envelope keeps the daemon protocol independent
                // from M1 stdio-IPC semver.
                let _ = event_tx.send(DaemonEvent::SyncEvent { line: line.clone() });

                let Some(value) = serde_json::from_str::<Value>(&line).ok() else { continue };
                let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match ty {
                    "summary" => {
                        tracker.total_planned = value.get("total_planned")
                            .and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        last_summary = Some(summary_from_value(&value));
                    }
                    "track_done" => { tracker.tracks_completed += 1; }
                    "error" => {
                        tracker.tracks_errored += 1;
                        if tracker.should_bail() {
                            let _ = stdin.write_all(b"{\"type\":\"cancel\"}\n").await;
                            let _ = stdin.flush().await;
                            drop(stdin);
                            bounded_kill(&mut child, crate::daemon::SYNC_KILL_GRACE).await;
                            return Ok(OrchestratorOutcome::Aborted {
                                reason: format!(
                                    "too_many_failures: {} of {} tracks failed",
                                    tracker.tracks_errored, tracker.total_planned
                                ),
                                summary: last_summary,
                            });
                        }
                    }
                    "finish" => {
                        finish_success = value.get("success").and_then(|v| v.as_bool());
                    }
                    "paused" => {
                        paused = true;
                    }
                    _ => {}
                }
            }
            _ = &mut cancel_rx => {
                // User cancelled. Same teardown sequence as the >50% bail.
                let _ = stdin.write_all(b"{\"type\":\"cancel\"}\n").await;
                let _ = stdin.flush().await;
                drop(stdin);
                bounded_kill(&mut child, crate::daemon::SYNC_KILL_GRACE).await;
                return Ok(OrchestratorOutcome::Aborted {
                    reason: "user_cancelled".to_string(),
                    summary: last_summary,
                });
            }
            _ = &mut pause_rx, if !pause_requested => {
                pause_requested = true;
                let _ = stdin.write_all(b"{\"type\":\"pause\"}\n").await;
                let _ = stdin.flush().await;
                // No force-kill: keep looping, relaying lines, until the
                // subprocess drains + checkpoints + emits "paused" + exits.
            }
            Some((id, choice)) = prompt_decisions_rx.recv() => {
                // User replied to a daemon-relayed prompt. Forward the
                // decision to the subprocess via stdin so its
                // apply_loop's await_prompt call returns. Errors
                // writing to stdin are non-fatal here — if the
                // subprocess died between the prompt emit and the
                // user's click, the SyncCompleted event from the
                // exited child will handle teardown normally.
                let line = format!("{{\"type\":\"prompt_decision\",\"id\":{id},\"choice\":{choice}}}\n");
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    tracing::warn!("orchestrator: failed to forward prompt_decision to subprocess: {e}");
                }
                let _ = stdin.flush().await;
            }
        }
    }

    let _ = child.wait().await;

    if paused {
        return Ok(OrchestratorOutcome::Paused { summary: last_summary });
    }

    let outcome = match finish_success {
        Some(true) => SyncOutcome::Ok,
        _ => SyncOutcome::Error,
    };
    Ok(OrchestratorOutcome::Completed { outcome, summary: last_summary })
}

fn summary_from_value(v: &Value) -> SyncSummary {
    SyncSummary {
        add: v.get("add").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        modify: v.get("modify").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        remove: v.get("remove").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        unchanged: v.get("unchanged").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        skipped: 0,
        // Matches the `metadata_only` key on the wire `summary` event (see
        // `IpcEvent::Summary` in ipc.rs / §4.3 of docs/ipc-protocol.md).
        // Metadata-only tracks ARE in the source and ARE already on the
        // iPod, so dropping this made the daemon's cached library_count
        // undercount (runtime.rs), letting "X of Y synced" show X > Y after
        // a tag-only sync.
        metadata_only: v.get("metadata_only").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
    }
}

async fn bounded_kill(child: &mut Child, timeout: Duration) {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(_) => {}
        Err(_) => { let _ = child.kill().await; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_command_passes_apply_and_ipod_flags() {
        let cmd = build_command(&PathBuf::from("classick.exe"), "G:\\");
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
    }

    #[test]
    fn tracker_does_not_bail_below_threshold() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 5, tracks_errored: 4 };
        assert!(!t.should_bail(), "4/10 (40%) must not bail");
    }

    #[test]
    fn tracker_bails_above_50_percent() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 3, tracks_errored: 6 };
        assert!(t.should_bail(), "6/10 (60%) must bail");
    }

    #[test]
    fn tracker_does_not_bail_when_no_plan() {
        let t = FailureTracker { total_planned: 0, tracks_completed: 0, tracks_errored: 3 };
        assert!(!t.should_bail(), "no plan => no bail (avoids div-by-zero edge case)");
    }

    #[test]
    fn tracker_does_not_bail_at_exactly_50_percent() {
        let t = FailureTracker { total_planned: 10, tracks_completed: 5, tracks_errored: 5 };
        assert!(!t.should_bail(), "exactly 50% must not bail (strict >50%)");
    }

    #[test]
    fn summary_from_value_parses_metadata_only() {
        // Regression test: metadata_only tracks are already on the iPod, so
        // dropping this field from the parsed SyncSummary made the daemon's
        // cached library_count undercount (see runtime.rs), producing
        // "X of Y synced" with X > Y after a tag-only sync.
        let v = serde_json::json!({
            "type": "summary",
            "add": 12,
            "modify": 3,
            "metadata_only": 7,
            "remove": 0,
            "unchanged": 1260,
            "total_planned": 15
        });
        let summary = summary_from_value(&v);
        assert_eq!(summary.add, 12);
        assert_eq!(summary.modify, 3);
        assert_eq!(summary.metadata_only, 7);
        assert_eq!(summary.remove, 0);
        assert_eq!(summary.unchanged, 1260);
    }

    #[test]
    fn summary_from_value_defaults_metadata_only_when_absent() {
        let v = serde_json::json!({"add": 1, "modify": 0, "remove": 0, "unchanged": 5});
        let summary = summary_from_value(&v);
        assert_eq!(summary.metadata_only, 0);
    }
}
