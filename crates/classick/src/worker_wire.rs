use crate::device::DeviceId;
use crate::progress::{Decision, EtaEstimator, ProgressEvent, ReviewDecision};
use crate::wire::{
    ActionPlanSummary, ArtworkSummary, CapabilityName, EndpointRole, PromptId, SessionId,
    SkippedForSpace, WireCommand, WireEvent, WireHello, WireMessage,
};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{Receiver, Sender};

pub(crate) const DEVICE_ID_ENV: &str = "CLASSICK_WORKER_DEVICE_ID";
pub(crate) const SESSION_ID_ENV: &str = "CLASSICK_WORKER_SESSION_ID";

enum WorkerRoute {
    Device {
        device_id: DeviceId,
        session_id: SessionId,
    },
    LibraryScan {
        session_id: SessionId,
    },
}

impl WorkerRoute {
    fn from_env() -> Result<Self> {
        let session_id = std::env::var(SESSION_ID_ENV)
            .context("worker session route is missing")?
            .parse::<u64>()
            .context("worker session route is not numeric")
            .and_then(SessionId::new)?;
        match std::env::var(DEVICE_ID_ENV) {
            Ok(device_id) => Ok(Self::Device {
                device_id: DeviceId::parse(&device_id)?,
                session_id,
            }),
            Err(std::env::VarError::NotPresent) => Ok(Self::LibraryScan { session_id }),
            Err(error) => Err(error).context("read worker device route"),
        }
    }
}

pub(crate) fn run(rx: Receiver<ProgressEvent>, decision_tx: Sender<Decision>) -> Result<()> {
    let route = WorkerRoute::from_env()?;
    let hello = WireMessage::Hello(WireHello::new(
        EndpointRole::Worker,
        env!("CARGO_PKG_VERSION"),
        [CapabilityName::parse("typed_sync_progress")?],
    )?);
    write_message(&hello).context("worker hello write failed")?;

    if let WorkerRoute::Device {
        device_id,
        session_id,
    } = &route
    {
        start_command_reader(device_id.clone(), *session_id, decision_tx);
    }

    match route {
        WorkerRoute::Device {
            device_id,
            session_id,
        } => run_device(rx, device_id, session_id),
        WorkerRoute::LibraryScan { session_id } => run_scan(rx, session_id),
    }
}

fn start_command_reader(device_id: DeviceId, session_id: SessionId, decision_tx: Sender<Decision>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in BufReader::new(stdin.lock()).lines() {
            let Ok(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let command = match serde_json::from_str::<WireCommand>(&line) {
                Ok(command) => command,
                Err(error) => {
                    tracing::warn!("worker rejected malformed command: {error}");
                    continue;
                }
            };
            let decision = match command {
                WireCommand::ApplyReview {
                    device_id: actual,
                    session_id: actual_session,
                    no_delete,
                    ..
                } if actual == device_id && actual_session == session_id => {
                    Some(Decision::Review(ReviewDecision::Apply { no_delete }))
                }
                WireCommand::DryRunReview {
                    device_id: actual,
                    session_id: actual_session,
                    ..
                } if actual == device_id && actual_session == session_id => {
                    Some(Decision::Review(ReviewDecision::DryRun))
                }
                WireCommand::QuitReview {
                    device_id: actual,
                    session_id: actual_session,
                    ..
                } if actual == device_id && actual_session == session_id => {
                    Some(Decision::Review(ReviewDecision::Quit))
                }
                WireCommand::PromptDecision {
                    device_id: actual,
                    session_id: actual_session,
                    prompt_id,
                    choice,
                    ..
                } if actual == device_id && actual_session == session_id => {
                    Some(Decision::Prompt {
                        id: prompt_id.get(),
                        choice: choice as usize,
                    })
                }
                WireCommand::FormDecision {
                    device_id: actual,
                    session_id: actual_session,
                    prompt_id,
                    value,
                    ..
                } if actual == device_id && actual_session == session_id => Some(Decision::Form {
                    id: prompt_id.get(),
                    value,
                }),
                WireCommand::CancelSync {
                    device_id: actual,
                    session_id: actual_session,
                    ..
                } if actual == device_id && actual_session == session_id => {
                    Some(Decision::Review(ReviewDecision::Quit))
                }
                WireCommand::PauseSync {
                    device_id: actual,
                    session_id: actual_session,
                    ..
                } if actual == device_id && actual_session == session_id => Some(Decision::Pause),
                _ => {
                    tracing::warn!("worker rejected command outside its owned session");
                    None
                }
            };
            if decision.is_some_and(|decision| decision_tx.send(decision).is_err()) {
                break;
            }
        }
    });
}

fn run_device(
    rx: Receiver<ProgressEvent>,
    device_id: DeviceId,
    session_id: SessionId,
) -> Result<()> {
    let mut eta = EtaEstimator::new();
    for event in rx {
        if matches!(event, ProgressEvent::TrackDone(_)) {
            eta.record_track_done();
        }
        let terminal = matches!(event, ProgressEvent::Finish { .. });
        let wire = device_event(event, &device_id, session_id, &eta)?;
        write_message(&WireMessage::Event(wire))?;
        if terminal {
            break;
        }
    }
    Ok(())
}

fn device_event(
    event: ProgressEvent,
    device_id: &DeviceId,
    session_id: SessionId,
    eta: &EtaEstimator,
) -> Result<WireEvent> {
    let route = || (device_id.clone(), session_id);
    Ok(match event {
        ProgressEvent::Header {
            source,
            ipod,
            manifest,
        } => {
            let (device_id, session_id) = route();
            WireEvent::RunHeader {
                device_id,
                session_id,
                source,
                ipod,
                manifest,
            }
        }
        ProgressEvent::Summary {
            add,
            modify,
            metadata_only,
            remove,
            unchanged,
            total_planned,
        } => {
            let (device_id, session_id) = route();
            WireEvent::SyncSummary {
                device_id,
                session_id,
                summary: ActionPlanSummary {
                    add: add as u64,
                    modify: modify as u64,
                    metadata_only: metadata_only as u64,
                    remove: remove as u64,
                    unchanged: unchanged as u64,
                    total_planned: total_planned as u64,
                },
            }
        }
        ProgressEvent::Review { summary, no_delete } => {
            let (device_id, session_id) = route();
            WireEvent::ReviewRequested {
                device_id,
                session_id,
                summary: ActionPlanSummary {
                    add: summary.add as u64,
                    modify: summary.modify as u64,
                    metadata_only: summary.metadata_only as u64,
                    remove: summary.remove as u64,
                    unchanged: summary.unchanged as u64,
                    total_planned: (summary.add
                        + summary.modify
                        + summary.metadata_only
                        + summary.remove) as u64,
                },
                no_delete,
            }
        }
        ProgressEvent::Prompt(prompt) => {
            let (device_id, session_id) = route();
            WireEvent::Prompt {
                device_id,
                session_id,
                prompt_id: PromptId::new(prompt.id)?,
                message: prompt.message,
                options: prompt.options,
            }
        }
        ProgressEvent::Form(form) => {
            let (device_id, session_id) = route();
            WireEvent::Form {
                device_id,
                session_id,
                prompt_id: PromptId::new(form.id)?,
                label: form.label,
                initial: form.initial,
                hint: form.hint,
            }
        }
        ProgressEvent::TrackStart {
            current,
            total,
            label,
        } => {
            let (device_id, session_id) = route();
            WireEvent::TrackStart {
                device_id,
                session_id,
                current: current as u64,
                total: total as u64,
                label,
                eta_secs: eta.eta_secs(current, total),
            }
        }
        ProgressEvent::TrackDone(result) => {
            let (device_id, session_id) = route();
            WireEvent::TrackDone {
                device_id,
                session_id,
                result: match result {
                    crate::progress::TrackResult::Applied => crate::wire::TrackResult::Applied,
                    crate::progress::TrackResult::Skipped => crate::wire::TrackResult::Skipped,
                },
            }
        }
        ProgressEvent::Finalizing {
            reason,
            staged_albums,
            staged_tracks,
        } => {
            let (device_id, session_id) = route();
            WireEvent::Finalizing {
                device_id,
                session_id,
                reason: match reason {
                    crate::progress::StopReason::Cancelled => crate::wire::StopReason::Cancelled,
                    crate::progress::StopReason::Paused => crate::wire::StopReason::Paused,
                },
                staged_albums: staged_albums as u64,
                staged_tracks: staged_tracks as u64,
            }
        }
        ProgressEvent::Cancelled => {
            let (device_id, session_id) = route();
            WireEvent::SyncCancelled {
                device_id,
                session_id,
            }
        }
        ProgressEvent::Paused => {
            let (device_id, session_id) = route();
            WireEvent::SyncPaused {
                device_id,
                session_id,
            }
        }
        ProgressEvent::Log(message) => {
            let (device_id, session_id) = route();
            WireEvent::SyncLog {
                device_id,
                session_id,
                message,
            }
        }
        ProgressEvent::Error {
            message,
            recovery_hints,
        } => {
            let (device_id, session_id) = route();
            WireEvent::SyncError {
                device_id,
                session_id,
                message,
                recovery_hints,
            }
        }
        ProgressEvent::Finish {
            success,
            skipped_for_space,
            artwork,
            db_restored,
        } => {
            let (device_id, session_id) = route();
            WireEvent::SyncFinished {
                device_id,
                session_id,
                success,
                skipped_for_space: skipped_for_space.map(|value| SkippedForSpace {
                    albums: value.albums as u64,
                    tracks: value.tracks as u64,
                    bytes: value.bytes,
                }),
                artwork: artwork.map(|value| ArtworkSummary {
                    embedded: value.embedded as u64,
                    eligible: value.eligible as u64,
                    failed_sources: value.failed_sources as u64,
                }),
                db_restored,
            }
        }
    })
}

fn run_scan(rx: Receiver<ProgressEvent>, session_id: SessionId) -> Result<()> {
    write_message(&WireMessage::Event(WireEvent::LibraryScanStarted {
        request_id: None,
        session_id,
    }))?;
    let mut files_scanned = 0_u64;
    let mut tracks_indexed = 0_u64;
    let mut failure = None;
    for event in rx {
        let wire = match event {
            ProgressEvent::Summary { add, unchanged, .. } => {
                files_scanned = (add + unchanged) as u64;
                Some(WireEvent::LibraryScanProgress {
                    request_id: None,
                    session_id,
                    files_scanned,
                    tracks_indexed,
                })
            }
            ProgressEvent::TrackDone(_) => {
                tracks_indexed += 1;
                Some(WireEvent::LibraryScanProgress {
                    request_id: None,
                    session_id,
                    files_scanned,
                    tracks_indexed,
                })
            }
            ProgressEvent::Error { message, .. } => {
                failure = Some(message);
                None
            }
            ProgressEvent::Finish { success, .. } => {
                let event = WireEvent::LibraryScanFinished {
                    request_id: None,
                    session_id,
                    success,
                    message: if success { None } else { failure.take() },
                };
                write_message(&WireMessage::Event(event))?;
                break;
            }
            ProgressEvent::Header { .. }
            | ProgressEvent::TrackStart { .. }
            | ProgressEvent::Log(_) => None,
            ProgressEvent::Review { .. }
            | ProgressEvent::Prompt(_)
            | ProgressEvent::Form(_)
            | ProgressEvent::Finalizing { .. }
            | ProgressEvent::Cancelled
            | ProgressEvent::Paused => bail!("library scan emitted a device-only progress event"),
        };
        if let Some(wire) = wire {
            write_message(&WireMessage::Event(wire))?;
        }
    }
    Ok(())
}

fn write_message(message: &WireMessage) -> Result<()> {
    let line = serde_json::to_string(message).context("serialize worker message")?;
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    writeln!(locked, "{line}").context("write worker message")?;
    locked.flush().context("flush worker message")
}
