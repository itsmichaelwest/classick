use super::{PromptId, RequestId, SessionId};
use crate::device::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Cancelled,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackResult {
    Applied,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPlanSummary {
    pub add: u64,
    pub modify: u64,
    pub metadata_only: u64,
    pub remove: u64,
    pub unchanged: u64,
    pub total_planned: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedForSpace {
    pub albums: u64,
    pub tracks: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtworkSummary {
    pub embedded: u64,
    pub eligible: u64,
    pub failed_sources: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireEvent {
    RunHeader {
        device_id: DeviceId,
        session_id: SessionId,
        source: String,
        ipod: String,
        manifest: String,
    },
    SyncSummary {
        device_id: DeviceId,
        session_id: SessionId,
        summary: ActionPlanSummary,
    },
    ReviewRequested {
        device_id: DeviceId,
        session_id: SessionId,
        summary: ActionPlanSummary,
        no_delete: bool,
    },
    Prompt {
        device_id: DeviceId,
        session_id: SessionId,
        prompt_id: PromptId,
        message: String,
        options: Vec<String>,
    },
    Form {
        device_id: DeviceId,
        session_id: SessionId,
        prompt_id: PromptId,
        label: String,
        initial: String,
        hint: String,
    },
    TrackStart {
        device_id: DeviceId,
        session_id: SessionId,
        current: u64,
        total: u64,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eta_secs: Option<u64>,
    },
    TrackDone {
        device_id: DeviceId,
        session_id: SessionId,
        result: TrackResult,
    },
    Finalizing {
        device_id: DeviceId,
        session_id: SessionId,
        reason: StopReason,
        staged_albums: u64,
        staged_tracks: u64,
    },
    SyncCancelled {
        device_id: DeviceId,
        session_id: SessionId,
    },
    SyncPaused {
        device_id: DeviceId,
        session_id: SessionId,
    },
    SyncLog {
        device_id: DeviceId,
        session_id: SessionId,
        message: String,
    },
    SyncError {
        device_id: DeviceId,
        session_id: SessionId,
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        recovery_hints: Vec<String>,
    },
    SyncFinished {
        device_id: DeviceId,
        session_id: SessionId,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skipped_for_space: Option<SkippedForSpace>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artwork: Option<ArtworkSummary>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        db_restored: bool,
    },
    CommandFailed {
        request_id: RequestId,
        message: String,
    },
}

impl WireEvent {
    pub(super) fn kind(&self) -> super::MessageKind {
        match self {
            Self::RunHeader { .. } => super::MessageKind::RunHeader,
            Self::SyncSummary { .. } => super::MessageKind::SyncSummary,
            Self::ReviewRequested { .. } => super::MessageKind::ReviewRequested,
            Self::Prompt { .. } => super::MessageKind::Prompt,
            Self::Form { .. } => super::MessageKind::Form,
            Self::TrackStart { .. } => super::MessageKind::TrackStart,
            Self::TrackDone { .. } => super::MessageKind::TrackDone,
            Self::Finalizing { .. } => super::MessageKind::Finalizing,
            Self::SyncCancelled { .. } => super::MessageKind::SyncCancelled,
            Self::SyncPaused { .. } => super::MessageKind::SyncPaused,
            Self::SyncLog { .. } => super::MessageKind::SyncLog,
            Self::SyncError { .. } => super::MessageKind::SyncError,
            Self::SyncFinished { .. } => super::MessageKind::SyncFinished,
            Self::CommandFailed { .. } => super::MessageKind::CommandFailed,
        }
    }

    pub(super) fn allowed_from_worker(&self) -> bool {
        !matches!(self, Self::CommandFailed { .. })
    }

    pub(super) fn route(&self) -> Option<(&DeviceId, SessionId)> {
        match self {
            Self::RunHeader {
                device_id,
                session_id,
                ..
            }
            | Self::SyncSummary {
                device_id,
                session_id,
                ..
            }
            | Self::ReviewRequested {
                device_id,
                session_id,
                ..
            }
            | Self::Prompt {
                device_id,
                session_id,
                ..
            }
            | Self::Form {
                device_id,
                session_id,
                ..
            }
            | Self::TrackStart {
                device_id,
                session_id,
                ..
            }
            | Self::TrackDone {
                device_id,
                session_id,
                ..
            }
            | Self::Finalizing {
                device_id,
                session_id,
                ..
            }
            | Self::SyncCancelled {
                device_id,
                session_id,
            }
            | Self::SyncPaused {
                device_id,
                session_id,
            }
            | Self::SyncLog {
                device_id,
                session_id,
                ..
            }
            | Self::SyncError {
                device_id,
                session_id,
                ..
            }
            | Self::SyncFinished {
                device_id,
                session_id,
                ..
            } => Some((device_id, *session_id)),
            Self::CommandFailed { .. } => None,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::RunHeader {
                source,
                ipod,
                manifest,
                ..
            } if source.is_empty() || ipod.is_empty() || manifest.is_empty() => {
                bail!("run header paths must not be empty")
            }
            Self::SyncSummary { summary, .. } | Self::ReviewRequested { summary, .. } => {
                summary.validate()?
            }
            Self::Prompt {
                message, options, ..
            } if message.is_empty()
                || options.is_empty()
                || options.iter().any(String::is_empty) =>
            {
                bail!("prompt requires a message and non-empty options")
            }
            Self::Form { label, .. } if label.is_empty() => bail!("form label must not be empty"),
            Self::TrackStart {
                current,
                total,
                label,
                ..
            } if *total == 0 || *current == 0 || current > total || label.is_empty() => {
                bail!("track start requires a 1-based position within a non-empty total")
            }
            Self::SyncLog { message, .. }
            | Self::SyncError { message, .. }
            | Self::CommandFailed { message, .. }
                if message.is_empty() =>
            {
                bail!("wire diagnostic message must not be empty")
            }
            Self::SyncFinished {
                skipped_for_space: Some(skipped),
                ..
            } if skipped.albums == 0 || skipped.tracks == 0 || skipped.bytes == 0 => {
                bail!("skipped-for-space summary must describe nonzero skipped content")
            }
            Self::SyncFinished {
                artwork: Some(artwork),
                ..
            } if artwork.embedded > artwork.eligible
                || artwork.failed_sources > artwork.eligible
                || artwork
                    .embedded
                    .checked_add(artwork.failed_sources)
                    .is_none_or(|processed| processed > artwork.eligible) =>
            {
                bail!("artwork summary counts are inconsistent")
            }
            _ => {}
        }
        Ok(())
    }
}

impl ActionPlanSummary {
    fn validate(&self) -> Result<()> {
        let without_removals = self
            .add
            .checked_add(self.modify)
            .and_then(|value| value.checked_add(self.metadata_only))
            .ok_or_else(|| anyhow::anyhow!("action-plan count overflow"))?;
        let with_removals = without_removals
            .checked_add(self.remove)
            .ok_or_else(|| anyhow::anyhow!("action-plan count overflow"))?;
        if self.total_planned != without_removals && self.total_planned != with_removals {
            bail!("action-plan total does not match its component counts");
        }
        Ok(())
    }
}
use anyhow::{bail, Result};
