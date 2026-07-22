use super::{PromptId, RequestId, SessionId};
use crate::device::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireCommand {
    ApplyReview {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
        no_delete: bool,
    },
    DryRunReview {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
    },
    QuitReview {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
    },
    PromptDecision {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
        prompt_id: PromptId,
        choice: u32,
    },
    FormDecision {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
        prompt_id: PromptId,
        value: Option<String>,
    },
    CancelSync {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
    },
    PauseSync {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
    },
}

impl WireCommand {
    pub(super) fn kind(&self) -> super::MessageKind {
        match self {
            Self::ApplyReview { .. } => super::MessageKind::ApplyReview,
            Self::DryRunReview { .. } => super::MessageKind::DryRunReview,
            Self::QuitReview { .. } => super::MessageKind::QuitReview,
            Self::PromptDecision { .. } => super::MessageKind::PromptDecision,
            Self::FormDecision { .. } => super::MessageKind::FormDecision,
            Self::CancelSync { .. } => super::MessageKind::CancelSync,
            Self::PauseSync { .. } => super::MessageKind::PauseSync,
        }
    }

    pub(super) fn route(&self) -> (&DeviceId, SessionId) {
        match self {
            Self::ApplyReview {
                device_id,
                session_id,
                ..
            }
            | Self::DryRunReview {
                device_id,
                session_id,
                ..
            }
            | Self::QuitReview {
                device_id,
                session_id,
                ..
            }
            | Self::PromptDecision {
                device_id,
                session_id,
                ..
            }
            | Self::FormDecision {
                device_id,
                session_id,
                ..
            }
            | Self::CancelSync {
                device_id,
                session_id,
                ..
            }
            | Self::PauseSync {
                device_id,
                session_id,
                ..
            } => (device_id, *session_id),
        }
    }
}
