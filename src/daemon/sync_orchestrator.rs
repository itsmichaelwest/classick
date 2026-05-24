//! Spawns the per-sync `ipod-sync.exe --ipc-mode --apply --ipod <drive>`
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
use tokio::sync::{broadcast, oneshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorOutcome {
    Completed { outcome: SyncOutcome, summary: Option<SyncSummary> },
    Aborted { reason: String, summary: Option<SyncSummary> },
}

/// Build the command to spawn. Extracted so tests can verify args
/// without actually spawning a process.
pub fn build_command(exe: &std::path::Path, drive: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode")
        .arg("--apply")
        .arg("--ipod")
        .arg(drive)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
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

/// Drive the spawned child to completion, until bail, or until cancelled.
/// `cancel_rx` fires when the user clicks Cancel in the UI; the orchestrator
/// writes a Cancel command to the subprocess stdin and force-kills after 5s.
pub async fn run(
    exe: PathBuf,
    drive: String,
    mut cancel_rx: oneshot::Receiver<()>,
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
        }
    }

    let _ = child.wait().await;

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
        let cmd = build_command(&PathBuf::from("ipod-sync.exe"), "G:\\");
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
}
