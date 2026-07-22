use super::{
    ActionPlanSummary, ArtworkSummary, PromptId, RequestId, SessionId, SkippedForSpace, StopReason,
    TrackResult, WireEvent,
};
use crate::device::DeviceId;
use crate::ipc::{IpcActionPlanSummary, IpcEvent};
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerPhase {
    AwaitingHello,
    Running { saw_error: bool },
    Finalizing { reason: StopReason, saw_error: bool },
    GracefulOutcome(StopReason),
    Finished,
}

pub struct LegacyWorkerDecoder {
    device_id: DeviceId,
    session_id: SessionId,
    phase: WorkerPhase,
}

impl LegacyWorkerDecoder {
    pub fn new(device_id: DeviceId, session_id: SessionId) -> Self {
        Self {
            device_id,
            session_id,
            phase: WorkerPhase::AwaitingHello,
        }
    }

    pub fn decode(&mut self, line: &str) -> Result<Option<WireEvent>> {
        let event: IpcEvent = serde_json::from_str(line).context("decode legacy worker event")?;
        if self.phase == WorkerPhase::AwaitingHello {
            admit_hello(event)?;
            self.phase = WorkerPhase::Running { saw_error: false };
            return Ok(None);
        }
        if matches!(event, IpcEvent::Hello { .. }) {
            bail!("legacy worker sent a second hello");
        }

        self.advance(&event)?;
        map_legacy_worker_event(event, &self.device_id, self.session_id).map(Some)
    }

    pub fn on_eof(&self) -> Result<()> {
        if self.phase != WorkerPhase::Finished {
            bail!("legacy worker closed before its terminal finish event");
        }
        Ok(())
    }

    fn advance(&mut self, event: &IpcEvent) -> Result<()> {
        self.phase = match self.phase {
            WorkerPhase::AwaitingHello => unreachable!("hello is handled before lifecycle events"),
            WorkerPhase::Finished => bail!("legacy worker sent an event after finish"),
            WorkerPhase::Running { saw_error } => match event {
                IpcEvent::Finalizing { reason, .. } => WorkerPhase::Finalizing {
                    reason: convert_reason(*reason),
                    saw_error: false,
                },
                IpcEvent::Cancelled | IpcEvent::Paused => {
                    bail!("legacy worker sent a graceful outcome before finalizing")
                }
                IpcEvent::Error { .. } => WorkerPhase::Running { saw_error: true },
                IpcEvent::Finish { success: false, .. } if !saw_error => {
                    bail!("legacy worker failed without a preceding error")
                }
                IpcEvent::Finish { .. } => WorkerPhase::Finished,
                _ => WorkerPhase::Running { saw_error },
            },
            WorkerPhase::Finalizing { reason, saw_error } => match (reason, event) {
                (StopReason::Cancelled, IpcEvent::Cancelled) => {
                    WorkerPhase::GracefulOutcome(reason)
                }
                (StopReason::Paused, IpcEvent::Paused) => WorkerPhase::GracefulOutcome(reason),
                (
                    _,
                    IpcEvent::TrackStart { .. } | IpcEvent::TrackDone { .. } | IpcEvent::Log { .. },
                ) => WorkerPhase::Finalizing { reason, saw_error },
                (_, IpcEvent::Error { .. }) => WorkerPhase::Finalizing {
                    reason,
                    saw_error: true,
                },
                (_, IpcEvent::Cancelled | IpcEvent::Paused) => {
                    bail!("legacy worker graceful outcome contradicts finalizing reason")
                }
                (_, IpcEvent::Finish { success: false, .. }) if saw_error => WorkerPhase::Finished,
                (_, IpcEvent::Finish { .. }) => {
                    bail!("legacy worker finished before its graceful outcome")
                }
                _ => bail!("legacy worker sent an invalid event while finalizing"),
            },
            WorkerPhase::GracefulOutcome(_) => match event {
                IpcEvent::Finish { success: true, .. } => WorkerPhase::Finished,
                IpcEvent::Finish { success: false, .. } => {
                    bail!("legacy worker reported failure after a graceful outcome")
                }
                _ => bail!("legacy worker sent an event after its graceful outcome"),
            },
        };
        Ok(())
    }
}

pub struct LegacyScanDecoder {
    request_id: RequestId,
    session_id: SessionId,
    phase: WorkerPhase,
    header_seen: bool,
    files_scanned: u64,
    tracks_indexed: u64,
    failure_message: Option<String>,
}

impl LegacyScanDecoder {
    pub fn new(request_id: RequestId, session_id: SessionId) -> Self {
        Self {
            request_id,
            session_id,
            phase: WorkerPhase::AwaitingHello,
            header_seen: false,
            files_scanned: 0,
            tracks_indexed: 0,
            failure_message: None,
        }
    }

    pub fn decode(&mut self, line: &str) -> Result<Option<WireEvent>> {
        let event: IpcEvent = serde_json::from_str(line).context("decode legacy scan event")?;
        if self.phase == WorkerPhase::AwaitingHello {
            admit_hello(event)?;
            self.phase = WorkerPhase::Running { saw_error: false };
            return Ok(Some(WireEvent::LibraryScanStarted {
                request_id: self.request_id.clone(),
                session_id: self.session_id,
            }));
        }
        if self.phase == WorkerPhase::Finished {
            bail!("legacy scan sent an event after finish");
        }
        if matches!(event, IpcEvent::Hello { .. }) {
            bail!("legacy scan sent a second hello");
        }

        let translated = match event {
            IpcEvent::Header {
                source,
                ipod,
                manifest,
            } => {
                if self.header_seen || source.is_empty() || !ipod.is_empty() || manifest.is_empty()
                {
                    bail!("legacy scan sent an invalid header");
                }
                self.header_seen = true;
                None
            }
            IpcEvent::Summary {
                add,
                modify,
                metadata_only,
                remove,
                unchanged,
                total_planned,
            } => {
                require_scan_header(self.header_seen)?;
                if modify != 0 || metadata_only != 0 || remove != 0 || add != total_planned {
                    bail!("legacy scan sent an invalid plan summary");
                }
                self.files_scanned = u64::try_from(add)?
                    .checked_add(u64::try_from(unchanged)?)
                    .context("legacy scan file count overflow")?;
                self.progress_event()
            }
            IpcEvent::TrackStart { .. } => {
                require_scan_header(self.header_seen)?;
                None
            }
            IpcEvent::TrackDone { .. } => {
                require_scan_header(self.header_seen)?;
                self.tracks_indexed = self.tracks_indexed.saturating_add(1);
                if self.tracks_indexed > self.files_scanned {
                    bail!("legacy scan indexed more tracks than it scanned");
                }
                self.progress_event()
            }
            IpcEvent::Log { .. } => None,
            IpcEvent::Error { message, .. } => {
                if message.is_empty() {
                    bail!("legacy scan error requires a message");
                }
                self.failure_message = Some(message);
                self.phase = WorkerPhase::Running { saw_error: true };
                None
            }
            IpcEvent::Finish { success, .. } => {
                if success {
                    require_scan_header(self.header_seen)?;
                }
                if success && self.failure_message.is_some() {
                    bail!("legacy scan succeeded after reporting a fatal error");
                }
                if !success && self.failure_message.is_none() {
                    bail!("legacy scan failed without a preceding error");
                }
                self.phase = WorkerPhase::Finished;
                Some(WireEvent::LibraryScanFinished {
                    request_id: self.request_id.clone(),
                    session_id: self.session_id,
                    success,
                    message: if success {
                        None
                    } else {
                        self.failure_message.take()
                    },
                })
            }
            IpcEvent::Review { .. }
            | IpcEvent::Prompt { .. }
            | IpcEvent::Form { .. }
            | IpcEvent::Finalizing { .. }
            | IpcEvent::Cancelled
            | IpcEvent::Paused => bail!("legacy scan sent a device-sync-only event"),
            IpcEvent::Hello { .. } => unreachable!("scan hello handled before event mapping"),
        };
        if let Some(event) = translated.as_ref() {
            event.validate()?;
        }
        Ok(translated)
    }

    pub fn on_eof(&self) -> Result<()> {
        if self.phase != WorkerPhase::Finished {
            bail!("legacy scan closed before its terminal finish event");
        }
        Ok(())
    }

    fn progress_event(&self) -> Option<WireEvent> {
        Some(WireEvent::LibraryScanProgress {
            request_id: self.request_id.clone(),
            session_id: self.session_id,
            files_scanned: self.files_scanned,
            tracks_indexed: self.tracks_indexed,
        })
    }
}

fn admit_hello(event: IpcEvent) -> Result<()> {
    let IpcEvent::Hello {
        protocol_version,
        core_version,
    } = event
    else {
        bail!("legacy subprocess must send hello first");
    };
    if protocol_version != crate::ipc::PROTOCOL_VERSION || core_version.is_empty() {
        bail!("legacy subprocess hello is incompatible");
    }
    Ok(())
}

fn require_scan_header(header_seen: bool) -> Result<()> {
    if !header_seen {
        bail!("legacy scan sent progress before its header");
    }
    Ok(())
}

fn map_legacy_worker_event(
    event: IpcEvent,
    device_id: &DeviceId,
    session_id: SessionId,
) -> Result<WireEvent> {
    let event = match event {
        IpcEvent::Hello { .. } => unreachable!("worker hello handled before event mapping"),
        IpcEvent::Header {
            source,
            ipod,
            manifest,
        } => WireEvent::RunHeader {
            device_id: device_id.clone(),
            session_id,
            source,
            ipod,
            manifest,
        },
        IpcEvent::Summary {
            add,
            modify,
            metadata_only,
            remove,
            unchanged,
            total_planned,
        } => WireEvent::SyncSummary {
            device_id: device_id.clone(),
            session_id,
            summary: ActionPlanSummary {
                add: add as u64,
                modify: modify as u64,
                metadata_only: metadata_only as u64,
                remove: remove as u64,
                unchanged: unchanged as u64,
                total_planned: total_planned as u64,
            },
        },
        IpcEvent::Review { summary, no_delete } => WireEvent::ReviewRequested {
            device_id: device_id.clone(),
            session_id,
            summary: review_summary(summary),
            no_delete,
        },
        IpcEvent::Prompt {
            id,
            message,
            options,
        } => WireEvent::Prompt {
            device_id: device_id.clone(),
            session_id,
            prompt_id: PromptId::new(id)?,
            message,
            options,
        },
        IpcEvent::Form {
            id,
            label,
            initial,
            hint,
        } => WireEvent::Form {
            device_id: device_id.clone(),
            session_id,
            prompt_id: PromptId::new(id)?,
            label,
            initial,
            hint,
        },
        IpcEvent::TrackStart {
            current,
            total,
            label,
            eta_secs,
        } => WireEvent::TrackStart {
            device_id: device_id.clone(),
            session_id,
            current: current as u64,
            total: total as u64,
            label,
            eta_secs,
        },
        IpcEvent::TrackDone { result } => WireEvent::TrackDone {
            device_id: device_id.clone(),
            session_id,
            result: match result {
                crate::progress::TrackResult::Applied => TrackResult::Applied,
                crate::progress::TrackResult::Skipped => TrackResult::Skipped,
            },
        },
        IpcEvent::Finalizing {
            reason,
            staged_albums,
            staged_tracks,
        } => WireEvent::Finalizing {
            device_id: device_id.clone(),
            session_id,
            reason: convert_reason(reason),
            staged_albums: staged_albums as u64,
            staged_tracks: staged_tracks as u64,
        },
        IpcEvent::Cancelled => WireEvent::SyncCancelled {
            device_id: device_id.clone(),
            session_id,
        },
        IpcEvent::Log { message } => WireEvent::SyncLog {
            device_id: device_id.clone(),
            session_id,
            message,
        },
        IpcEvent::Error {
            message,
            recovery_hints,
        } => WireEvent::SyncError {
            device_id: device_id.clone(),
            session_id,
            message,
            recovery_hints,
        },
        IpcEvent::Finish {
            success,
            skipped_for_space,
            artwork,
            db_restored,
        } => WireEvent::SyncFinished {
            device_id: device_id.clone(),
            session_id,
            success,
            skipped_for_space: skipped_for_space.map(|summary| SkippedForSpace {
                albums: summary.albums as u64,
                tracks: summary.tracks as u64,
                bytes: summary.bytes,
            }),
            artwork: artwork.map(|summary| ArtworkSummary {
                embedded: summary.embedded as u64,
                eligible: summary.eligible as u64,
                failed_sources: summary.failed_sources as u64,
            }),
            db_restored,
        },
        IpcEvent::Paused => WireEvent::SyncPaused {
            device_id: device_id.clone(),
            session_id,
        },
    };
    event.validate()?;
    Ok(event)
}

fn convert_reason(reason: crate::progress::StopReason) -> StopReason {
    match reason {
        crate::progress::StopReason::Cancelled => StopReason::Cancelled,
        crate::progress::StopReason::Paused => StopReason::Paused,
    }
}

fn review_summary(summary: IpcActionPlanSummary) -> ActionPlanSummary {
    let total_planned = summary
        .add
        .saturating_add(summary.modify)
        .saturating_add(summary.metadata_only)
        .saturating_add(summary.remove);
    ActionPlanSummary {
        add: summary.add as u64,
        modify: summary.modify as u64,
        metadata_only: summary.metadata_only as u64,
        remove: summary.remove as u64,
        unchanged: summary.unchanged as u64,
        total_planned: total_planned as u64,
    }
}
