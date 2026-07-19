//! Spawns the per-sync `classick.exe --ipc-mode --apply --ipod <drive>`
//! subprocess. Forwards every IpcEvent line to the broadcast channel so
//! UI clients see live progress. Counts per-track errors against
//! `Summary.total_planned` and bails through coordinated cancellation when
//! `tracks_errored * 2 > total_planned`.

use crate::daemon::history::SyncSummary;
use crate::daemon::session_admission::{EventContext, SessionPhase};
use crate::ipc_daemon::DaemonEvent;
use crate::progress::StopReason;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::Instant;

/// Emergency backstop for a finalizing subprocess that has stopped producing
/// progress. Every valid progress event resets this grace period.
pub const FINALIZATION_STALL_GRACE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorOutcome {
    Completed {
        summary: SyncSummary,
        /// Mirrors the subprocess `finish` event's `db_restored` field
        /// (Task 4's auto-restore-from-backup path). Only successful syncs
        /// expose it because interrupted and user-stopped attempts have their
        /// own terminal outcomes even when a trailing `finish` is drained.
        db_restored: bool,
    },
    Aborted {
        reason: String,
        summary: Option<SyncSummary>,
    },
    Cancelled {
        summary: Option<SyncSummary>,
    },
    /// The subprocess emitted `{"type":"paused"}` (graceful drain +
    /// checkpoint) and then exited on its own. Distinct from `Aborted`:
    /// nothing failed, the user asked to stop, and a later `TriggerSync`
    /// resumes from the checkpoint via the normal diff.
    Paused {
        summary: Option<SyncSummary>,
    },
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
pub fn build_command(exe: &std::path::Path, drive: &str, rockbox_compat: bool) -> Command {
    let mut cmd = base_command(exe, "--apply", Some(drive));
    if rockbox_compat {
        cmd.arg("--rockbox-compat");
    }
    cmd
}

/// Build the one-shot backfill subprocess command: same stdio/no-console
/// setup as `build_command`, but `--backfill-rockbox` instead of `--apply`
/// — it embeds tags + art into the existing on-iPod library in place
/// rather than running a full add/modify/remove sync.
pub fn build_backfill_command(exe: &std::path::Path, drive: &str) -> Command {
    base_command(exe, "--backfill-rockbox", Some(drive))
}

/// Build the replace-library subprocess command: same stdio/no-console
/// setup as `build_command`, but `--replace-library --apply` instead of
/// plain `--apply` — it wipes every track on the iPod before falling
/// through to a normal sync of the current selection. `--apply` is what
/// makes `apply_loop::replace_library` skip its interactive confirmation
/// prompt (see `should_skip_replace_confirmation`); the UI does its own
/// typed confirmation before ever sending the `replace_library` command.
pub fn build_replace_library_command(exe: &std::path::Path, drive: &str) -> Command {
    let mut cmd = base_command(exe, "--replace-library", Some(drive));
    cmd.arg("--apply");
    cmd
}

/// Build the library-scan subprocess command. No --ipod: a scan only reads
/// the source tree and writes the index cache.
pub fn build_scan_command(exe: &std::path::Path) -> Command {
    base_command(exe, "--scan-library", None)
}

/// Shared stdio/no-console setup for the sync/backfill/scan subprocess
/// commands. See `build_command`'s doc comment for why `kill_on_drop(true)`
/// is load-bearing. `drive` is `None` for a scan (no device involved).
fn base_command(exe: &std::path::Path, mode_flag: &str, drive: Option<&str>) -> Command {
    use crate::windows_proc::NoConsoleWindow;
    let mut cmd = Command::new(exe);
    cmd.arg("--ipc-mode").arg(mode_flag);
    if let Some(d) = drive {
        cmd.arg("--ipod").arg(d);
    }
    cmd.stdin(Stdio::piped())
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
        self.total_planned > 0
            && self.tracks_errored > 0
            && self.tracks_errored * 2 > self.total_planned
    }
}

/// Drive the spawned child to completion, until bail, until cancelled, or
/// until paused.
///
/// `cancel_rx` fires when the user clicks Cancel in the UI. The orchestrator
/// writes exactly one Cancel command, keeps stdin/stdout open, and forwards
/// progress through `cancelled` and EOF.
///
/// `pause_rx` fires when the user clicks Pause in the UI; the orchestrator
/// writes a Pause command to the subprocess stdin and, unlike cancel, does
/// NOT immediately force-kill — pause is graceful, so the subprocess is
/// given a chance to finish draining its in-flight window, checkpoint, emit
/// `{"type":"paused"}`, and exit on its own.
///
/// Both stop paths use an inactivity watchdog: every progress line resets a
/// 120-second grace. A child that stops producing progress is killed and
/// reported as aborted; it is never reported as cancelled or paused.
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
    rockbox_compat: bool,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_command(&exe, &drive, rockbox_compat);
    drive_child(
        exe,
        cmd,
        cancel_rx,
        pause_rx,
        prompt_decisions_rx,
        event_tx,
        event_context,
    )
    .await
}

/// Run the one-shot backfill subprocess (`--backfill-rockbox`) through the
/// same drive-to-completion machinery as `run` — identical event
/// forwarding, failure-bail threshold, cancel/pause handling — so the UI
/// sees sync-style progress for a backfill with no special-casing on its
/// side. `event_tx` is the SAME broadcast channel `run` uses, so a
/// `DecidePrompt`/`CancelSync`/`Pause` sent while a backfill is in flight
/// behaves exactly as it would during a normal sync.
pub async fn run_backfill(
    exe: PathBuf,
    drive: String,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_backfill_command(&exe, &drive);
    drive_child(
        exe,
        cmd,
        cancel_rx,
        pause_rx,
        prompt_decisions_rx,
        event_tx,
        event_context,
    )
    .await
}

/// Run the one-shot replace-library subprocess (`--replace-library
/// --apply`) through the same drive-to-completion machinery as `run`/
/// `run_backfill` — identical event forwarding, failure-bail threshold,
/// cancel/pause handling — so the UI sees ordinary sync-style progress for
/// a replace with no special-casing on its side. `event_tx` is the SAME
/// broadcast channel `run`/`run_backfill` use, so a `DecidePrompt`/
/// `CancelSync`/`Pause` sent while a replace is in flight behaves exactly
/// as it would during a normal sync.
pub async fn run_replace_library(
    exe: PathBuf,
    drive: String,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_replace_library_command(&exe, &drive);
    drive_child(
        exe,
        cmd,
        cancel_rx,
        pause_rx,
        prompt_decisions_rx,
        event_tx,
        event_context,
    )
    .await
}

/// Run a --scan-library subprocess through the same drive-to-completion
/// machinery as syncs/backfills (event forwarding, cancel/pause, bail
/// threshold — mostly inert for a scan, but shared code is shared behavior).
pub async fn run_scan(
    exe: PathBuf,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_scan_command(&exe);
    drive_child(
        exe,
        cmd,
        cancel_rx,
        pause_rx,
        prompt_decisions_rx,
        event_tx,
        event_context,
    )
    .await
}

async fn drive_child(
    exe: PathBuf,
    cmd: Command,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    drive_child_with_stall_grace(
        exe,
        cmd,
        cancel_rx,
        pause_rx,
        prompt_decisions_rx,
        event_tx,
        event_context,
        FINALIZATION_STALL_GRACE,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn drive_child_with_stall_grace(
    exe: PathBuf,
    mut cmd: Command,
    mut cancel_rx: oneshot::Receiver<()>,
    mut pause_rx: oneshot::Receiver<()>,
    mut prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
    stall_grace: Duration,
) -> Result<OrchestratorOutcome> {
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", exe.display()))?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let mut stdin = child.stdin.take().context("child stdin missing")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut tracker = FailureTracker::default();
    let mut last_summary: Option<SyncSummary> = None;
    let mut finish_success: Option<bool> = None;
    let mut finish_db_restored = false;
    let mut cancelled = false;
    let mut paused = false;
    let mut stop_disposition: Option<StopDisposition> = None;
    let mut watchdog = FinalizationWatchdog::new(stall_grace);
    // A one-shot receiver becomes ready for both an explicit signal and a
    // dropped sender. Track closure separately so sender teardown disables
    // the branch instead of being misread as a cancel/pause request or being
    // re-polled after completion.
    let mut cancel_channel_open = true;
    let mut pause_channel_open = true;
    let mut prompt_channel_open = true;

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
                let _ = event_tx.send(event_context.wrap(line.clone()));
                let Some(value) = serde_json::from_str::<Value>(&line).ok() else { continue };
                watchdog.record_progress();
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
                        if tracker.should_bail() && stop_disposition.is_none() {
                            write_stop_command(&mut stdin, StopReason::Cancelled).await;
                            stop_disposition = Some(StopDisposition::Aborted(format!(
                                    "too_many_failures: {} of {} tracks failed",
                                    tracker.tracks_errored, tracker.total_planned
                                )));
                            watchdog.begin(StopReason::Cancelled);
                        }
                    }
                    "finalizing" => {
                        if let Some(reason) = stop_reason_from_value(&value) {
                            watchdog.begin(reason);
                            stop_disposition.get_or_insert(match reason {
                                StopReason::Cancelled => StopDisposition::Cancelled,
                                StopReason::Paused => StopDisposition::Paused,
                            });
                        }
                    }
                    "cancelled" => cancelled = true,
                    "finish" => {
                        finish_success = value.get("success").and_then(|v| v.as_bool());
                        finish_db_restored = db_restored_from_finish_value(&value);
                        // The "summary" event (parsed above, into
                        // `last_summary`) always precedes "finish" in the
                        // wire stream, but the skipped-for-space/artwork
                        // fields only ride the *finish* event (Task 8), so
                        // merge them into the already-captured summary here.
                        // If a summary event was somehow never seen, fall
                        // back to a zeroed one rather than silently dropping
                        // the fit-pass/artwork rollup.
                        let summary = last_summary.get_or_insert(SyncSummary {
                            add: 0, modify: 0, remove: 0, unchanged: 0, skipped: 0,
                            metadata_only: 0, skipped_for_space_tracks: 0,
                            skipped_for_space_bytes: 0, artwork_failed_sources: 0,
                        });
                        merge_finish_fields_into_summary(summary, &value);
                    }
                    "paused" => paused = true,
                    _ => {}
                }
            }
            cancel_result = &mut cancel_rx, if cancel_channel_open => {
                cancel_channel_open = false;
                match classify_control_signal(cancel_result) {
                    ControlSignal::Requested => {
                        if stop_disposition.is_none() {
                            write_stop_command(&mut stdin, StopReason::Cancelled).await;
                            stop_disposition = Some(StopDisposition::Cancelled);
                            watchdog.begin(StopReason::Cancelled);
                        }
                    }
                    ControlSignal::Closed => {}
                }
            }
            pause_result = &mut pause_rx, if pause_channel_open => {
                pause_channel_open = false;
                if classify_control_signal(pause_result) == ControlSignal::Requested
                    && stop_disposition.is_none()
                {
                    write_stop_command(&mut stdin, StopReason::Paused).await;
                    stop_disposition = Some(StopDisposition::Paused);
                    watchdog.begin(StopReason::Paused);
                }
            }
            _ = finalization_stall_deadline(watchdog.deadline()) => {
                tracing::warn!(
                    "orchestrator: finalization made no progress for {:?}; force-killing",
                    stall_grace
                );
                force_kill(&mut child).await;
                return Ok(OrchestratorOutcome::Aborted {
                    reason: "finalization_stalled".to_string(),
                    summary: last_summary,
                });
            }
            prompt_decision = prompt_decisions_rx.recv(), if prompt_channel_open => {
                let Some((id, choice)) = prompt_decision else {
                    prompt_channel_open = false;
                    continue;
                };
                // User replied to a daemon-relayed prompt. Forward the
                // decision to the subprocess via stdin so its
                // apply_loop's await_prompt call returns. Errors
                // writing to stdin are non-fatal here — if the
                // subprocess died between the prompt emit and the
                // user's click, the SyncCompleted event from the
                // exited child will handle teardown normally.
                //
                // INVARIANT (wire audit 2026-07-18): `prompt_decision` is
                // the ONLY reply this relay can carry. The subprocess's
                // `form` and `review` events have no decision path through
                // the daemon (no form_decision/review_decision relay
                // exists, and the UIs can't answer them) — that's sound
                // only because a daemon-spawned subprocess gets full
                // config (the wizard that emits `form` never runs) and is
                // always spawned auto-apply (`review` is never emitted).
                // If either ever appears on a daemon-driven sync, the
                // subprocess will block forever awaiting a reply that
                // cannot arrive — add the relay before adding the emitter.
                let line = format!("{{\"type\":\"prompt_decision\",\"id\":{id},\"choice\":{choice}}}\n");
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    tracing::warn!("orchestrator: failed to forward prompt_decision to subprocess: {e}");
                }
                let _ = stdin.flush().await;
            }
        }
    }

    drop(stdin);
    let status = child.wait().await.context("wait for sync subprocess")?;

    match stop_disposition {
        Some(StopDisposition::Cancelled) if cancelled => {
            return Ok(OrchestratorOutcome::Cancelled {
                summary: last_summary,
            });
        }
        Some(StopDisposition::Paused) if paused => {
            return Ok(OrchestratorOutcome::Paused {
                summary: last_summary,
            });
        }
        Some(StopDisposition::Aborted(reason)) => {
            return Ok(OrchestratorOutcome::Aborted {
                reason,
                summary: last_summary,
            });
        }
        Some(_) => {
            return Ok(OrchestratorOutcome::Aborted {
                reason: "finalization_interrupted".to_string(),
                summary: last_summary,
            });
        }
        None if cancelled => {
            return Ok(OrchestratorOutcome::Cancelled {
                summary: last_summary,
            });
        }
        None if paused => {
            return Ok(OrchestratorOutcome::Paused {
                summary: last_summary,
            });
        }
        None => {}
    }

    if finish_success == Some(true) && status.success() {
        return Ok(OrchestratorOutcome::Completed {
            summary: last_summary.unwrap_or_default(),
            db_restored: finish_db_restored,
        });
    }

    Ok(OrchestratorOutcome::Aborted {
        reason: if finish_success == Some(false) {
            "sync_subprocess_reported_failure".to_string()
        } else {
            format!("sync_subprocess_exited_without_success: {status}")
        },
        summary: last_summary,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlSignal {
    Requested,
    Closed,
}

fn classify_control_signal(
    result: std::result::Result<(), oneshot::error::RecvError>,
) -> ControlSignal {
    match result {
        Ok(()) => ControlSignal::Requested,
        Err(_) => ControlSignal::Closed,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StopDisposition {
    Cancelled,
    Paused,
    Aborted(String),
}

#[derive(Debug)]
struct FinalizationWatchdog {
    grace: Duration,
    phase: SessionPhase,
    deadline: Option<Instant>,
}

impl FinalizationWatchdog {
    fn new(grace: Duration) -> Self {
        Self {
            grace,
            phase: SessionPhase::Running,
            deadline: None,
        }
    }

    fn begin(&mut self, reason: StopReason) {
        if self.phase == SessionPhase::Running {
            self.phase = SessionPhase::Finalizing { reason };
            self.deadline = Some(Instant::now() + self.grace);
        }
    }

    fn record_progress(&mut self) {
        if matches!(self.phase, SessionPhase::Finalizing { .. }) {
            self.deadline = Some(Instant::now() + self.grace);
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    #[cfg(test)]
    fn is_stalled(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

fn stop_reason_from_value(value: &Value) -> Option<StopReason> {
    match value.get("reason").and_then(Value::as_str) {
        Some("cancelled") => Some(StopReason::Cancelled),
        Some("paused") => Some(StopReason::Paused),
        _ => None,
    }
}

async fn write_stop_command(stdin: &mut tokio::process::ChildStdin, reason: StopReason) {
    let command = match reason {
        StopReason::Cancelled => b"{\"type\":\"cancel\"}\n".as_slice(),
        StopReason::Paused => b"{\"type\":\"pause\"}\n".as_slice(),
    };
    if let Err(error) = stdin.write_all(command).await {
        tracing::warn!("orchestrator: failed to write stop command: {error}");
        return;
    }
    if let Err(error) = stdin.flush().await {
        tracing::warn!("orchestrator: failed to flush stop command: {error}");
    }
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
        // Populated later, when the "finish" event arrives (see the
        // "finish" match arm above) — the "summary" event this function
        // parses doesn't carry them.
        skipped_for_space_tracks: 0,
        skipped_for_space_bytes: 0,
        artwork_failed_sources: 0,
    }
}

/// Extracts `db_restored` from a raw `finish` event `Value`. `false` when
/// absent, matching the wire's old-client-compat convention (the field is
/// omitted rather than sent as `false`).
fn db_restored_from_finish_value(v: &Value) -> bool {
    v.get("db_restored")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

/// Merges the `skipped_for_space`/`artwork` rollups from a raw `finish`
/// event `Value` into an already-captured `SyncSummary` (built from the
/// preceding `summary` event). Note `skipped_for_space.albums` is
/// deliberately NOT persisted — only `tracks`/`bytes` map onto
/// `SyncSummary`, per plan.
fn merge_finish_fields_into_summary(summary: &mut SyncSummary, v: &Value) {
    let skipped_for_space = v.get("skipped_for_space");
    summary.skipped_for_space_tracks = skipped_for_space
        .and_then(|s| s.get("tracks"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as usize;
    summary.skipped_for_space_bytes = skipped_for_space
        .and_then(|s| s.get("bytes"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    summary.artwork_failed_sources = v
        .get("artwork")
        .and_then(|a| a.get("failed_sources"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as usize;
}

async fn finalization_stall_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

async fn force_kill(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn scripted_command(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    #[cfg(unix)]
    fn event_context() -> EventContext {
        EventContext {
            session_id: 41,
            serial: Some("RAW-A".to_string()),
        }
    }

    #[cfg(unix)]
    fn summary() -> SyncSummary {
        SyncSummary {
            add: 1,
            modify: 0,
            remove: 0,
            unchanged: 2,
            skipped: 0,
            metadata_only: 0,
            skipped_for_space_tracks: 0,
            skipped_for_space_bytes: 0,
            artwork_failed_sources: 0,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_is_written_once_then_finalizing_cancelled_and_eof_are_drained() {
        let record = std::env::temp_dir().join(format!(
            "classick-cancel-write-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_file(&record);
        let script = r#"
            printf '%s\n' '{"type":"summary","add":1,"modify":0,"remove":0,"unchanged":2,"total_planned":1}'
            IFS= read -r line
            printf '%s\n' "$line" > "$RECORD_PATH"
            printf '%s\n' '{"type":"finalizing","reason":"cancelled","staged_albums":1,"staged_tracks":1}'
            printf '%s\n' '{"type":"cancelled"}'
            printf '%s\n' '{"type":"finish","success":true}'
        "#;
        let mut command = scripted_command(script);
        command.env("RECORD_PATH", &record);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(16);

        let task = tokio::spawn(drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            event_context(),
            Duration::from_secs(1),
        ));
        cancel_tx.send(()).unwrap();

        let outcome = task.await.unwrap().unwrap();
        assert_eq!(
            outcome,
            OrchestratorOutcome::Cancelled {
                summary: Some(summary())
            }
        );
        assert_eq!(
            std::fs::read_to_string(&record).unwrap(),
            "{\"type\":\"cancel\"}\n",
            "cancel must be written exactly once"
        );
        let forwarded: Vec<String> = std::iter::from_fn(|| event_rx.try_recv().ok())
            .filter_map(|event| match event {
                DaemonEvent::SyncEvent { line, .. } => Some(line),
                _ => None,
            })
            .collect();
        assert_eq!(
            forwarded
                .iter()
                .filter_map(|line| {
                    serde_json::from_str::<Value>(line)
                        .ok()
                        .and_then(|value| value["type"].as_str().map(str::to_string))
                })
                .collect::<Vec<_>>(),
            ["summary", "finalizing", "cancelled", "finish"]
        );
        let _ = std::fs::remove_file(record);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ordinary_successful_finish_and_eof_are_completed() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"summary","add":1,"modify":0,"remove":0,"unchanged":2,"total_planned":1}'
                printf '%s\n' '{"type":"finish","success":true}'
            "#,
        );
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(8);

        let outcome = drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            event_context(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            OrchestratorOutcome::Completed {
                summary: summary(),
                db_restored: false,
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_finalization_is_killed_and_aborted() {
        let command = scripted_command(
            r#"
                IFS= read -r line
                printf '%s\n' '{"type":"finalizing","reason":"cancelled","staged_albums":1,"staged_tracks":1}'
                sleep 10
            "#,
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(8);
        let task = tokio::spawn(drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            event_context(),
            Duration::from_millis(50),
        ));
        cancel_tx.send(()).unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("stalled child must be killed")
            .unwrap()
            .unwrap();
        assert!(matches!(
            outcome,
            OrchestratorOutcome::Aborted { reason, .. }
                if reason == "finalization_stalled"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_before_cancelled_is_interrupted_not_cancelled() {
        let command = scripted_command(
            r#"
                IFS= read -r line
                printf '%s\n' '{"type":"finalizing","reason":"cancelled","staged_albums":1,"staged_tracks":1}'
            "#,
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(8);
        let task = tokio::spawn(drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            event_context(),
            Duration::from_secs(1),
        ));
        cancel_tx.send(()).unwrap();

        assert!(matches!(
            task.await.unwrap().unwrap(),
            OrchestratorOutcome::Aborted { reason, .. }
                if reason == "finalization_interrupted"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn finalization_progress_resets_the_stall_grace() {
        let grace = Duration::from_secs(120);
        let mut watchdog = FinalizationWatchdog::new(grace);
        watchdog.begin(StopReason::Cancelled);
        let first_deadline = watchdog.deadline().unwrap();

        tokio::time::advance(Duration::from_secs(119)).await;
        watchdog.record_progress();
        let reset_deadline = watchdog.deadline().unwrap();

        assert!(reset_deadline > first_deadline);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(!watchdog.is_stalled());
        tokio::time::advance(Duration::from_secs(118)).await;
        assert!(watchdog.is_stalled());
    }

    #[tokio::test]
    async fn dropped_pause_sender_is_not_a_pause_request() {
        let (pause_tx, pause_rx) = oneshot::channel();
        drop(pause_tx);

        assert_eq!(
            classify_control_signal(pause_rx.await),
            ControlSignal::Closed
        );
    }

    #[tokio::test]
    async fn explicit_cancel_signal_is_a_control_request() {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        cancel_tx.send(()).unwrap();

        assert_eq!(
            classify_control_signal(cancel_rx.await),
            ControlSignal::Requested
        );
    }

    // Exercises `finalization_stall_deadline` directly rather than driving `run`
    // end-to-end. A real integration test would need a dummy subprocess
    // that ignores stdin and never exits — the task's suggested `cat` does
    // NOT work: `build_command` always appends
    // `--ipc-mode --apply --ipod <drive>`, and `cat --ipc-mode ...` treats
    // those as illegal options and exits immediately with an error,
    // defeating the "never exits" premise the test needs. Making `run`
    // accept an arbitrary pre-built `Command` (or an injected grace
    // parameter) just to route around that would touch the production
    // signature for a test-only concern, which the task said not to do.
    // The Windows daemon-integration suite (`tests/daemon_runtime_integration.rs`,
    // `#![cfg(windows)]`-gated) is the real end-to-end coverage for this
    // path but doesn't run on macOS. So: cover the new deadline logic in
    // isolation here, and rely on manual device smoke-testing for the
    // full `run()` behavior.

    #[tokio::test(start_paused = true)]
    async fn finalization_stall_deadline_resolves_once_armed_deadline_elapses() {
        let deadline = Instant::now() + Duration::from_secs(15);
        finalization_stall_deadline(Some(deadline)).await;
        assert!(
            Instant::now() >= deadline,
            "must not resolve before the armed deadline"
        );
    }

    #[tokio::test]
    async fn finalization_stall_deadline_never_resolves_when_unarmed() {
        // No deadline armed (mirrors `pause_deadline == None` before pause is
        // ever requested) — the backstop branch must stay pending forever so
        // it can safely live in `select!` without a guard.
        tokio::select! {
            _ = finalization_stall_deadline(None) => panic!("must never resolve without an armed deadline"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
    use std::path::PathBuf;

    #[test]
    fn build_command_passes_apply_and_ipod_flags() {
        let cmd = build_command(&PathBuf::from("classick.exe"), "G:\\", false);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
        assert!(!dbg.contains("--rockbox-compat"));
    }

    #[test]
    fn build_command_adds_rockbox_flag_when_enabled() {
        let cmd = build_command(&PathBuf::from("classick.exe"), "G:\\", true);
        assert!(format!("{cmd:?}").contains("--rockbox-compat"));
    }

    #[test]
    fn build_backfill_command_passes_backfill_and_ipod_flags() {
        let cmd = build_backfill_command(&PathBuf::from("classick.exe"), "G:\\");
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--backfill-rockbox"));
        assert!(!dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
    }

    #[test]
    fn build_replace_library_command_passes_replace_and_apply_flags() {
        let cmd = build_replace_library_command(&PathBuf::from("classick.exe"), "G:\\");
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--replace-library"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
    }

    #[test]
    fn build_scan_command_passes_scan_flag_without_ipod() {
        let cmd = build_scan_command(&PathBuf::from("classick"));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--scan-library"));
        assert!(!dbg.contains("--ipod"), "a scan involves no device");
        assert!(!dbg.contains("--apply"));
    }

    #[test]
    fn tracker_does_not_bail_below_threshold() {
        let t = FailureTracker {
            total_planned: 10,
            tracks_completed: 5,
            tracks_errored: 4,
        };
        assert!(!t.should_bail(), "4/10 (40%) must not bail");
    }

    #[test]
    fn tracker_bails_above_50_percent() {
        let t = FailureTracker {
            total_planned: 10,
            tracks_completed: 3,
            tracks_errored: 6,
        };
        assert!(t.should_bail(), "6/10 (60%) must bail");
    }

    #[test]
    fn tracker_does_not_bail_when_no_plan() {
        let t = FailureTracker {
            total_planned: 0,
            tracks_completed: 0,
            tracks_errored: 3,
        };
        assert!(
            !t.should_bail(),
            "no plan => no bail (avoids div-by-zero edge case)"
        );
    }

    #[test]
    fn tracker_does_not_bail_at_exactly_50_percent() {
        let t = FailureTracker {
            total_planned: 10,
            tracks_completed: 5,
            tracks_errored: 5,
        };
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

    // -- Task 9: finish-event fields merged onto SyncSummary/db_restored ---

    #[test]
    fn merge_finish_fields_maps_skipped_for_space_tracks_and_bytes_not_albums() {
        let v = serde_json::json!({
            "type": "finish",
            "success": true,
            "skipped_for_space": {"albums": 14, "tracks": 183, "bytes": 9_876_543_210u64},
        });
        let mut summary = summary_from_value(&serde_json::json!({}));
        merge_finish_fields_into_summary(&mut summary, &v);
        assert_eq!(summary.skipped_for_space_tracks, 183);
        assert_eq!(summary.skipped_for_space_bytes, 9_876_543_210);
        // `albums` is deliberately not part of SyncSummary at all — there's
        // no field to even accidentally populate; this test documents that.
    }

    #[test]
    fn merge_finish_fields_maps_artwork_failed_sources() {
        let v = serde_json::json!({
            "type": "finish",
            "success": true,
            "artwork": {"embedded": 10, "eligible": 12, "failed_sources": 2},
        });
        let mut summary = summary_from_value(&serde_json::json!({}));
        merge_finish_fields_into_summary(&mut summary, &v);
        assert_eq!(summary.artwork_failed_sources, 2);
    }

    #[test]
    fn merge_finish_fields_defaults_to_zero_when_absent() {
        let v = serde_json::json!({"type": "finish", "success": true});
        let mut summary = summary_from_value(&serde_json::json!({}));
        merge_finish_fields_into_summary(&mut summary, &v);
        assert_eq!(summary.skipped_for_space_tracks, 0);
        assert_eq!(summary.skipped_for_space_bytes, 0);
        assert_eq!(summary.artwork_failed_sources, 0);
    }

    #[test]
    fn db_restored_from_finish_value_parses_true() {
        let v = serde_json::json!({"type": "finish", "success": true, "db_restored": true});
        assert!(db_restored_from_finish_value(&v));
    }

    #[test]
    fn db_restored_from_finish_value_defaults_false_when_absent() {
        let v = serde_json::json!({"type": "finish", "success": true});
        assert!(!db_restored_from_finish_value(&v));
    }
}
