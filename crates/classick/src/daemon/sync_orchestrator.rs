//! Spawns the per-sync `classick.exe --ipc-mode --apply --ipod <drive>`
//! subprocess. Forwards every IpcEvent line to the broadcast channel so
//! UI clients see live progress. Counts per-track errors against
//! `Summary.total_planned` and bails through coordinated cancellation when
//! `tracks_errored * 2 > total_planned`.

use crate::daemon::history::SyncSummary;
use crate::daemon::session_admission::{EventContext, SessionPhase};
use crate::device::DeviceId;
use crate::ipc_daemon::DaemonEvent;
use crate::progress::StopReason;
use crate::wire::{
    decode_admitted_message, decode_initial_hello, validate_peer_hello, AdmittedStream,
    CapabilityName, DecodedWireMessage, EndpointRole, OwnedSessionRoute,
    SessionId as WireSessionId, WireEvent, WireMessage,
};
use anyhow::{Context, Result};
#[cfg(test)]
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

struct WorkerStreamDecoder {
    stream: AdmittedStream,
    hello_seen: bool,
    lifecycle: WorkerLifecycle,
}

#[derive(Clone, Copy)]
enum WorkerLifecycle {
    DeviceRunning {
        saw_error: bool,
    },
    DeviceFinalizing {
        reason: crate::wire::StopReason,
        saw_error: bool,
    },
    DeviceGraceful(crate::wire::StopReason),
    ScanAwaitingStart,
    ScanRunning,
    Finished,
}

impl WorkerStreamDecoder {
    fn new(route: OwnedSessionRoute, scan: bool) -> Self {
        Self {
            stream: AdmittedStream::DaemonReceivingWorkerEvents(route),
            hello_seen: false,
            lifecycle: if scan {
                WorkerLifecycle::ScanAwaitingStart
            } else {
                WorkerLifecycle::DeviceRunning { saw_error: false }
            },
        }
    }

    fn decode(&mut self, line: &str) -> Result<Option<WireEvent>> {
        if !self.hello_seen {
            let hello = decode_initial_hello(line)?;
            validate_peer_hello(
                &hello,
                EndpointRole::Worker,
                &[CapabilityName::parse("typed_sync_progress")?],
            )?;
            self.hello_seen = true;
            return Ok(None);
        }
        if matches!(self.lifecycle, WorkerLifecycle::Finished) {
            anyhow::bail!("worker sent an event after its terminal event");
        }
        let decoded = decode_admitted_message(line, &self.stream)?;
        let DecodedWireMessage::Known(message) = decoded else {
            anyhow::bail!("worker output cannot contain unknown events");
        };
        let WireMessage::Event(event) = *message else {
            anyhow::bail!("worker output must contain an event");
        };
        self.advance(&event)?;
        Ok(Some(event))
    }

    fn on_eof(&self) -> Result<()> {
        if !self.hello_seen {
            anyhow::bail!("worker closed before hello");
        }
        if !matches!(self.lifecycle, WorkerLifecycle::Finished) {
            anyhow::bail!("worker closed before its terminal event");
        }
        Ok(())
    }

    fn advance(&mut self, event: &WireEvent) -> Result<()> {
        use WorkerLifecycle as Lifecycle;
        self.lifecycle = match self.lifecycle {
            Lifecycle::Finished => anyhow::bail!("worker sent an event after finish"),
            Lifecycle::ScanAwaitingStart => match event {
                WireEvent::LibraryScanStarted { .. } => Lifecycle::ScanRunning,
                _ => anyhow::bail!("library scan worker must emit scan_started first"),
            },
            Lifecycle::ScanRunning => match event {
                WireEvent::LibraryScanProgress { .. } => Lifecycle::ScanRunning,
                WireEvent::LibraryScanFinished { .. } => Lifecycle::Finished,
                _ => anyhow::bail!("library scan worker emitted a device event"),
            },
            Lifecycle::DeviceRunning { saw_error } => match event {
                WireEvent::SyncError { .. } => Lifecycle::DeviceRunning { saw_error: true },
                WireEvent::Finalizing { reason, .. } => Lifecycle::DeviceFinalizing {
                    reason: *reason,
                    saw_error,
                },
                WireEvent::SyncCancelled { .. } | WireEvent::SyncPaused { .. } => {
                    anyhow::bail!("worker emitted graceful outcome before finalizing")
                }
                WireEvent::SyncFinished { success: false, .. } if !saw_error => {
                    anyhow::bail!("worker failed without a preceding error")
                }
                WireEvent::SyncFinished { .. } => Lifecycle::Finished,
                _ => Lifecycle::DeviceRunning { saw_error },
            },
            Lifecycle::DeviceFinalizing { reason, saw_error } => match event {
                WireEvent::SyncCancelled { .. } if reason == crate::wire::StopReason::Cancelled => {
                    Lifecycle::DeviceGraceful(reason)
                }
                WireEvent::SyncPaused { .. } if reason == crate::wire::StopReason::Paused => {
                    Lifecycle::DeviceGraceful(reason)
                }
                WireEvent::TrackStart { .. }
                | WireEvent::TrackDone { .. }
                | WireEvent::SyncLog { .. } => Lifecycle::DeviceFinalizing { reason, saw_error },
                WireEvent::SyncError { .. } => Lifecycle::DeviceFinalizing {
                    reason,
                    saw_error: true,
                },
                WireEvent::SyncFinished { success: false, .. } if saw_error => Lifecycle::Finished,
                WireEvent::SyncCancelled { .. } | WireEvent::SyncPaused { .. } => {
                    anyhow::bail!("worker graceful outcome contradicts finalizing reason")
                }
                WireEvent::SyncFinished { .. } => {
                    anyhow::bail!("worker finished before its graceful outcome")
                }
                _ => anyhow::bail!("worker emitted an invalid event while finalizing"),
            },
            Lifecycle::DeviceGraceful(reason) => match event {
                WireEvent::SyncFinished { success: true, .. } => Lifecycle::Finished,
                WireEvent::SyncFinished { success: false, .. } => {
                    anyhow::bail!("worker failed after graceful {reason:?}")
                }
                _ => anyhow::bail!("worker emitted an event after its graceful outcome"),
            },
        };
        Ok(())
    }
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
pub fn build_command(
    exe: &std::path::Path,
    drive: &str,
    rockbox_compat: bool,
    transcode_profile: crate::portable::profile::TranscodeProfile,
) -> Command {
    let mut cmd = base_command(exe, "--apply", Some(drive));
    cmd.arg("--transcode-profile")
        .arg(transcode_profile.as_str());
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
pub fn build_replace_library_command(
    exe: &std::path::Path,
    drive: &str,
    transcode_profile: crate::portable::profile::TranscodeProfile,
) -> Command {
    let mut cmd = base_command(exe, "--replace-library", Some(drive));
    cmd.arg("--apply")
        .arg("--transcode-profile")
        .arg(transcode_profile.as_str());
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
    transcode_profile: crate::portable::profile::TranscodeProfile,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_command(&exe, &drive, rockbox_compat, transcode_profile);
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
    transcode_profile: crate::portable::profile::TranscodeProfile,
    cancel_rx: oneshot::Receiver<()>,
    pause_rx: oneshot::Receiver<()>,
    prompt_decisions_rx: mpsc::UnboundedReceiver<(u64, i32)>,
    event_tx: broadcast::Sender<DaemonEvent>,
    event_context: EventContext,
) -> Result<OrchestratorOutcome> {
    let cmd = build_replace_library_command(&exe, &drive, transcode_profile);
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
    let wire_session_id = WireSessionId::new(event_context.session_id)
        .context("validate admitted worker session ID")?;
    cmd.env(
        crate::worker_wire::SESSION_ID_ENV,
        event_context.session_id.to_string(),
    );
    let is_scan = event_context.serial.is_none();
    let route = match event_context.serial.as_deref() {
        Some(serial) => {
            let device_id =
                DeviceId::parse(serial).context("validate admitted worker device ID")?;
            cmd.env(crate::worker_wire::DEVICE_ID_ENV, device_id.as_str());
            OwnedSessionRoute::new(device_id, wire_session_id)
        }
        None => {
            cmd.env_remove(crate::worker_wire::DEVICE_ID_ENV);
            OwnedSessionRoute::library_scan(wire_session_id)
        }
    };
    let mut worker_decoder = WorkerStreamDecoder::new(route, is_scan);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", exe.display()))?;
    let stdout = child.stdout.take().context("child stdout missing")?;
    let mut stdin = child.stdin.take().context("child stdin missing")?;
    let mut reader = BufReader::new(stdout).lines();

    let mut tracker = FailureTracker::default();
    let mut last_summary: Option<SyncSummary> = None;
    let mut last_sync_error: Option<String> = None;
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
                let line = match line_res {
                    Ok(Some(line)) => line,
                    Ok(None) => break, // subprocess closed stdout (normal completion or crash)
                    Err(error) => {
                        tracing::warn!(
                            session_id = event_context.session_id,
                            "orchestrator: rejected unreadable worker output: {error}"
                        );
                        force_kill(&mut child).await;
                        return Ok(OrchestratorOutcome::Aborted {
                            reason: "worker_protocol_error".to_string(),
                            summary: last_summary,
                        });
                    }
                };

                let wire_event = match worker_decoder.decode(&line) {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(
                            session_id = event_context.session_id,
                            "orchestrator: rejected malformed worker output: {error:#}"
                        );
                        force_kill(&mut child).await;
                        return Ok(OrchestratorOutcome::Aborted {
                            reason: "worker_protocol_error".to_string(),
                            summary: last_summary,
                        });
                    }
                };
                let Some(wire_event) = wire_event else {
                    continue;
                };
                let _ = event_tx.send(event_context.wrap(line.clone(), Some(wire_event.clone())));
                watchdog.record_progress();
                match &wire_event {
                    WireEvent::SyncSummary { summary, .. } => {
                        tracker.total_planned = summary.total_planned as usize;
                        last_summary = Some(summary_from_wire(summary));
                    }
                    WireEvent::TrackDone { .. } => { tracker.tracks_completed += 1; }
                    WireEvent::SyncError { message, .. } => {
                        if let Some(detail) = message.strip_prefix("Sync failed: ") {
                            last_sync_error = Some(detail.to_string());
                        } else if last_sync_error.is_none() {
                            last_sync_error = Some(message.clone());
                        }
                        tracker.tracks_errored += 1;
                        if tracker.should_bail() && stop_disposition.is_none() {
                            write_stop_command(&mut stdin, StopReason::Cancelled, &event_context).await;
                            stop_disposition = Some(StopDisposition::Aborted(format!(
                                "too_many_failures: {} of {} tracks failed",
                                tracker.tracks_errored, tracker.total_planned
                                )));
                            watchdog.begin(StopReason::Cancelled);
                        }
                    }
                    WireEvent::Finalizing { reason, .. } => {
                        let reason = progress_stop_reason(*reason);
                        watchdog.begin(reason);
                        stop_disposition.get_or_insert(match reason {
                            StopReason::Cancelled => StopDisposition::Cancelled,
                            StopReason::Paused => StopDisposition::Paused,
                        });
                    }
                    WireEvent::SyncCancelled { .. } => cancelled = true,
                    WireEvent::SyncFinished {
                        success,
                        db_restored,
                        skipped_for_space,
                        artwork,
                        ..
                    } => {
                        finish_success = Some(*success);
                        finish_db_restored = *db_restored;
                        let summary = last_summary.get_or_insert(SyncSummary {
                            add: 0, modify: 0, remove: 0, unchanged: 0, skipped: 0,
                            metadata_only: 0, skipped_for_space_tracks: 0,
                            skipped_for_space_bytes: 0, artwork_failed_sources: 0,
                        });
                        summary.skipped_for_space_tracks = skipped_for_space
                            .as_ref()
                            .map_or(0, |value| value.tracks as usize);
                        summary.skipped_for_space_bytes =
                            skipped_for_space.as_ref().map_or(0, |value| value.bytes);
                        summary.artwork_failed_sources =
                            artwork.as_ref().map_or(0, |value| value.failed_sources as usize);
                    }
                    WireEvent::LibraryScanFinished { success, .. } => {
                        finish_success = Some(*success);
                    }
                    WireEvent::SyncPaused { .. } => paused = true,
                    _ => {}
                }
            }
            cancel_result = &mut cancel_rx, if cancel_channel_open => {
                cancel_channel_open = false;
                match classify_control_signal(cancel_result) {
                    ControlSignal::Requested => {
                        if stop_disposition.is_none() {
                            if event_context.serial.is_none() {
                                force_kill(&mut child).await;
                                return Ok(OrchestratorOutcome::Cancelled {
                                    summary: last_summary,
                                });
                            }
                            write_stop_command(&mut stdin, StopReason::Cancelled, &event_context).await;
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
                    if event_context.serial.is_none() {
                        force_kill(&mut child).await;
                        return Ok(OrchestratorOutcome::Paused {
                            summary: last_summary,
                        });
                    }
                    write_stop_command(&mut stdin, StopReason::Paused, &event_context).await;
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
                let Some(device_id) = event_context.serial.as_deref()
                    .and_then(|value| DeviceId::parse(value).ok()) else { continue };
                let Ok(session_id) = WireSessionId::new(event_context.session_id) else { continue };
                let Ok(prompt_id) = crate::wire::PromptId::new(id) else { continue };
                let Ok(request_id) = synthetic_request_id(event_context.session_id, id) else { continue };
                let command = crate::wire::WireCommand::PromptDecision {
                    device_id,
                    session_id,
                    request_id,
                    prompt_id,
                    choice: choice.max(0) as u32,
                };
                let line = serde_json::to_string(&WireMessage::Command(command))
                    .expect("valid prompt decision") + "\n";
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    tracing::warn!("orchestrator: failed to forward prompt_decision to subprocess: {e}");
                }
                let _ = stdin.flush().await;
            }
        }
    }

    if let Err(error) = worker_decoder.on_eof() {
        tracing::warn!(
            session_id = event_context.session_id,
            "orchestrator: worker stream ended incorrectly: {error:#}"
        );
        force_kill(&mut child).await;
        return Ok(OrchestratorOutcome::Aborted {
            reason: "worker_protocol_error".to_string(),
            summary: last_summary,
        });
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
            last_sync_error.unwrap_or_else(|| "sync_subprocess_reported_failure".to_string())
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

async fn write_stop_command(
    stdin: &mut tokio::process::ChildStdin,
    reason: StopReason,
    context: &EventContext,
) {
    let Some(device_id) = context
        .serial
        .as_deref()
        .and_then(|value| DeviceId::parse(value).ok())
    else {
        return;
    };
    let Ok(session_id) = WireSessionId::new(context.session_id) else {
        return;
    };
    let Ok(request_id) = synthetic_request_id(
        context.session_id,
        if reason == StopReason::Cancelled {
            1
        } else {
            2
        },
    ) else {
        return;
    };
    let command = match reason {
        StopReason::Cancelled => crate::wire::WireCommand::CancelSync {
            device_id,
            session_id,
            request_id,
        },
        StopReason::Paused => crate::wire::WireCommand::PauseSync {
            device_id,
            session_id,
            request_id,
        },
    };
    let mut command =
        serde_json::to_vec(&WireMessage::Command(command)).expect("valid stop command");
    command.push(b'\n');
    if let Err(error) = stdin.write_all(&command).await {
        tracing::warn!("orchestrator: failed to write stop command: {error}");
        return;
    }
    if let Err(error) = stdin.flush().await {
        tracing::warn!("orchestrator: failed to flush stop command: {error}");
    }
}

fn synthetic_request_id(session_id: u64, discriminator: u64) -> Result<crate::wire::RequestId> {
    crate::wire::RequestId::parse(&format!(
        "00000000-0000-0001-{:04x}-{:012x}",
        session_id & 0xffff,
        discriminator & 0xffff_ffff_ffff
    ))
}

fn progress_stop_reason(reason: crate::wire::StopReason) -> StopReason {
    match reason {
        crate::wire::StopReason::Cancelled => StopReason::Cancelled,
        crate::wire::StopReason::Paused => StopReason::Paused,
    }
}

fn summary_from_wire(v: &crate::wire::ActionPlanSummary) -> SyncSummary {
    SyncSummary {
        add: v.add as usize,
        modify: v.modify as usize,
        remove: v.remove as usize,
        unchanged: v.unchanged as usize,
        skipped: 0,
        metadata_only: v.metadata_only as usize,
        skipped_for_space_tracks: 0,
        skipped_for_space_bytes: 0,
        artwork_failed_sources: 0,
    }
}

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
    use serde_json::Value;

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
            serial: Some("000A27002138B0A8".to_string()),
        }
    }

    #[cfg(unix)]
    fn scan_event_context() -> EventContext {
        EventContext {
            session_id: 43,
            serial: None,
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
            printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
            printf '%s\n' '{"type":"sync_summary","device_id":"000A27002138B0A8","session_id":41,"summary":{"add":1,"modify":0,"metadata_only":0,"remove":0,"unchanged":2,"total_planned":1}}'
            IFS= read -r line
            printf '%s\n' "$line" > "$RECORD_PATH"
            printf '%s\n' '{"type":"finalizing","device_id":"000A27002138B0A8","session_id":41,"reason":"cancelled","staged_albums":1,"staged_tracks":1}'
            printf '%s\n' '{"type":"sync_cancelled","device_id":"000A27002138B0A8","session_id":41}'
            printf '%s\n' '{"type":"sync_finished","device_id":"000A27002138B0A8","session_id":41,"success":true}'
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
            "{\"type\":\"cancel_sync\",\"device_id\":\"000A27002138B0A8\",\"session_id\":41,\"request_id\":\"00000000-0000-0001-0029-000000000001\"}\n",
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
            [
                "sync_summary",
                "finalizing",
                "sync_cancelled",
                "sync_finished"
            ]
        );
        let _ = std::fs::remove_file(record);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ordinary_successful_finish_and_eof_are_completed() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '%s\n' '{"type":"sync_summary","device_id":"000A27002138B0A8","session_id":41,"summary":{"add":1,"modify":0,"metadata_only":0,"remove":0,"unchanged":2,"total_planned":1}}'
                printf '%s\n' '{"type":"sync_finished","device_id":"000A27002138B0A8","session_id":41,"success":true}'
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
    async fn failed_finish_preserves_the_worker_error_for_history_and_ui() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '%s\n' '{"type":"sync_summary","device_id":"000A27002138B0A8","session_id":41,"summary":{"add":13,"modify":0,"metadata_only":0,"remove":0,"unchanged":0,"total_planned":13}}'
                printf '%s\n' '{"type":"sync_error","device_id":"000A27002138B0A8","session_id":41,"message":"Sync failed: checkpoint generation fence: external device state changed"}'
                printf '%s\n' '{"type":"sync_finished","device_id":"000A27002138B0A8","session_id":41,"success":false}'
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

        assert!(matches!(
            outcome,
            OrchestratorOutcome::Aborted { reason, .. }
                if reason == "checkpoint generation fence: external device state changed"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scan_output_is_validated_before_legacy_broadcast() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '%s\n' '{"type":"library_scan_started","session_id":43}'
                printf '%s\n' '{"type":"library_scan_progress","session_id":43,"files_scanned":3,"tracks_indexed":0}'
                printf '%s\n' '{"type":"library_scan_progress","session_id":43,"files_scanned":3,"tracks_indexed":1}'
                printf '%s\n' '{"type":"library_scan_finished","session_id":43,"success":true}'
            "#,
        );
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(8);

        let outcome = drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            scan_event_context(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            OrchestratorOutcome::Completed {
                summary: SyncSummary::default(),
                db_restored: false,
            }
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
                .map(|line| serde_json::from_str::<Value>(line).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned())
                .collect::<Vec<_>>(),
            [
                "library_scan_started",
                "library_scan_progress",
                "library_scan_progress",
                "library_scan_finished"
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn device_sync_shape_on_scan_worker_is_not_broadcast() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '%s\n' '{"type":"run_header","device_id":"000A27002138B0A8","session_id":43,"source":"/Music","ipod":"/Volumes/iPod","manifest":"/state/library-index.json"}'
            "#,
        );
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(8);

        let outcome = drive_child_with_stall_grace(
            PathBuf::from("/bin/sh"),
            command,
            cancel_rx,
            pause_rx,
            prompt_rx,
            event_tx,
            scan_event_context(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            OrchestratorOutcome::Aborted { reason, .. } if reason == "worker_protocol_error"
        ));
        assert_eq!(
            std::iter::from_fn(|| event_rx.try_recv().ok()).count(),
            0,
            "misrouted worker output must not cross"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_finalization_is_killed_and_aborted() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                IFS= read -r line
                printf '%s\n' '{"type":"finalizing","device_id":"000A27002138B0A8","session_id":41,"reason":"cancelled","staged_albums":1,"staged_tracks":1}'
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
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                IFS= read -r line
                printf '%s\n' '{"type":"finalizing","device_id":"000A27002138B0A8","session_id":41,"reason":"cancelled","staged_albums":1,"staged_tracks":1}'
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
                if reason == "worker_protocol_error"
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_worker_output_is_not_broadcast() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '%s\n' '{"type":"sync_finished","device_id":"000A27002138B0A8","session_id":42,"success":false}'
            "#,
        );
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(8);

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

        assert!(matches!(
            outcome,
            OrchestratorOutcome::Aborted { reason, .. } if reason == "worker_protocol_error"
        ));
        let forwarded: Vec<String> = std::iter::from_fn(|| event_rx.try_recv().ok())
            .filter_map(|event| match event {
                DaemonEvent::SyncEvent { line, .. } => Some(line),
                _ => None,
            })
            .collect();
        assert!(forwarded.is_empty(), "invalid worker output must not cross");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_worker_output_is_killed_and_not_broadcast() {
        let command = scripted_command(
            r#"
                printf '%s\n' '{"type":"hello","protocol_version":"3.0.0","role":"worker","software_version":"0.0.1","capabilities":["typed_sync_progress"]}'
                printf '\377\n'
                sleep 10
            "#,
        );
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (_pause_tx, pause_rx) = oneshot::channel();
        let (_prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (event_tx, mut event_rx) = broadcast::channel(8);

        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            drive_child_with_stall_grace(
                PathBuf::from("/bin/sh"),
                command,
                cancel_rx,
                pause_rx,
                prompt_rx,
                event_tx,
                event_context(),
                Duration::from_secs(1),
            ),
        )
        .await
        .expect("unreadable worker must be killed")
        .unwrap();

        assert!(matches!(
            outcome,
            OrchestratorOutcome::Aborted { reason, .. } if reason == "worker_protocol_error"
        ));
        let forwarded: Vec<String> = std::iter::from_fn(|| event_rx.try_recv().ok())
            .filter_map(|event| match event {
                DaemonEvent::SyncEvent { line, .. } => Some(line),
                _ => None,
            })
            .collect();
        assert!(
            forwarded.is_empty(),
            "unreadable worker output must not cross"
        );
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
        let cmd = build_command(
            &PathBuf::from("classick.exe"),
            "G:\\",
            false,
            crate::portable::profile::TranscodeProfile::Aac192,
        );
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
        assert!(!dbg.contains("--rockbox-compat"));
        assert!(dbg.contains("--transcode-profile"));
        assert!(dbg.contains("aac_192"));
    }

    #[test]
    fn build_command_adds_rockbox_flag_when_enabled() {
        let cmd = build_command(
            &PathBuf::from("classick.exe"),
            "G:\\",
            true,
            crate::portable::profile::TranscodeProfile::Alac,
        );
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
        let cmd = build_replace_library_command(
            &PathBuf::from("classick.exe"),
            "G:\\",
            crate::portable::profile::TranscodeProfile::Aac128,
        );
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("--ipc-mode"));
        assert!(dbg.contains("--replace-library"));
        assert!(dbg.contains("--apply"));
        assert!(dbg.contains("--ipod"));
        assert!(dbg.contains("G:\\"));
        assert!(dbg.contains("--transcode-profile"));
        assert!(dbg.contains("aac_128"));
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
