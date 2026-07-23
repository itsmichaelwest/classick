use super::{PromptId, RequestId, SessionId};
use crate::device::DeviceId;
use crate::portable::profile::MutationId;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigComponent {
    Selection,
    Settings,
    Subscriptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFailureStage {
    HostAcceptance,
    DeviceDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireEvent {
    GlobalConfig {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_root: Option<super::SourceRoot>,
        settings: super::GlobalSettings,
    },
    SourceAvailability {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        state: super::SourceAvailabilityState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_root: Option<super::SourceRoot>,
    },
    DeviceInventory {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        #[serde(flatten)]
        snapshot: super::DeviceInventorySnapshot,
    },
    InventorySubscriptionChanged {
        request_id: RequestId,
        subscribed: bool,
    },
    DeviceConfig {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        #[serde(flatten)]
        config: super::DeviceConfigSnapshot,
    },
    ConfigMutationFailed {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        component: ConfigComponent,
        stage: ConfigFailureStage,
        message: String,
    },
    DeviceForgotten {
        device_id: DeviceId,
        request_id: RequestId,
    },
    SyncAccepted {
        device_id: DeviceId,
        session_id: SessionId,
        request_id: RequestId,
        operation: super::SyncOperation,
    },
    SyncRejected {
        device_id: DeviceId,
        request_id: RequestId,
        operation: super::SyncOperation,
        reason: super::SyncRejectReason,
        message: String,
    },
    History {
        request_id: RequestId,
        entries: Vec<super::HistoryEntry>,
    },
    Library {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        #[serde(flatten)]
        snapshot: super::LibrarySnapshot,
    },
    LibraryScanStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        session_id: SessionId,
    },
    LibraryScanProgress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        session_id: SessionId,
        files_scanned: u64,
        tracks_indexed: u64,
    },
    LibraryScanFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        session_id: SessionId,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    SelectionPreview {
        device_id: DeviceId,
        request_id: RequestId,
        #[serde(flatten)]
        preview: super::SelectionPreview,
    },
    DevicePreview {
        device_id: DeviceId,
        request_id: RequestId,
        #[serde(flatten)]
        preview: super::DevicePreview,
    },
    ResolvedTracks {
        request_id: RequestId,
        tracks: Vec<crate::portable::profile::ProfilePath>,
    },
    Playlists {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        revision: u64,
        playlists: Vec<super::PlaylistSummary>,
    },
    PlaylistDetail {
        request_id: RequestId,
        revision: u64,
        slug: crate::portable::profile::PlaylistSlug,
        result: super::PlaylistDetailResult,
    },
    PlaylistSaved {
        request_id: RequestId,
        revision: u64,
        playlist: super::StoredPlaylist,
    },
    DeviceSelectionAdded {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        matched_tracks: u64,
        missing_tracks: u64,
        selection_changed: bool,
        selection_revision: u64,
        selection: crate::portable::profile::SelectionValue,
        delivery: super::ConfigDelivery,
        sync: super::DropSyncDisposition,
    },
    PlaylistSelectionAppended {
        request_id: RequestId,
        slug: crate::portable::profile::PlaylistSlug,
        appended_tracks: u64,
        revision: u64,
        playlist: super::StoredPlaylist,
    },
    LibraryMutationRejected {
        request_id: RequestId,
        target: super::LibraryMutationTarget,
        code: String,
        message: String,
    },
    DaemonShutdownStarted {
        request_id: RequestId,
    },
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
            Self::GlobalConfig { .. } => super::MessageKind::GlobalConfig,
            Self::SourceAvailability { .. } => super::MessageKind::SourceAvailability,
            Self::DeviceInventory { .. } => super::MessageKind::DeviceInventory,
            Self::InventorySubscriptionChanged { .. } => {
                super::MessageKind::InventorySubscriptionChanged
            }
            Self::DeviceConfig { .. } => super::MessageKind::DeviceConfig,
            Self::ConfigMutationFailed { .. } => super::MessageKind::ConfigMutationFailed,
            Self::DeviceForgotten { .. } => super::MessageKind::DeviceForgotten,
            Self::SyncAccepted { .. } => super::MessageKind::SyncAccepted,
            Self::SyncRejected { .. } => super::MessageKind::SyncRejected,
            Self::History { .. } => super::MessageKind::History,
            Self::Library { .. } => super::MessageKind::Library,
            Self::LibraryScanStarted { .. } => super::MessageKind::LibraryScanStarted,
            Self::LibraryScanProgress { .. } => super::MessageKind::LibraryScanProgress,
            Self::LibraryScanFinished { .. } => super::MessageKind::LibraryScanFinished,
            Self::SelectionPreview { .. } => super::MessageKind::SelectionPreview,
            Self::DevicePreview { .. } => super::MessageKind::DevicePreview,
            Self::ResolvedTracks { .. } => super::MessageKind::ResolvedTracks,
            Self::Playlists { .. } => super::MessageKind::Playlists,
            Self::PlaylistDetail { .. } => super::MessageKind::PlaylistDetail,
            Self::PlaylistSaved { .. } => super::MessageKind::PlaylistSaved,
            Self::DeviceSelectionAdded { .. } => super::MessageKind::DeviceSelectionAdded,
            Self::PlaylistSelectionAppended { .. } => super::MessageKind::PlaylistSelectionAppended,
            Self::LibraryMutationRejected { .. } => super::MessageKind::LibraryMutationRejected,
            Self::DaemonShutdownStarted { .. } => super::MessageKind::DaemonShutdownStarted,
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
        !matches!(
            self,
            Self::GlobalConfig { .. }
                | Self::SourceAvailability { .. }
                | Self::DeviceInventory { .. }
                | Self::InventorySubscriptionChanged { .. }
                | Self::DeviceConfig { .. }
                | Self::ConfigMutationFailed { .. }
                | Self::DeviceForgotten { .. }
                | Self::SyncAccepted { .. }
                | Self::SyncRejected { .. }
                | Self::History { .. }
                | Self::Library { .. }
                | Self::SelectionPreview { .. }
                | Self::DevicePreview { .. }
                | Self::ResolvedTracks { .. }
                | Self::Playlists { .. }
                | Self::PlaylistDetail { .. }
                | Self::PlaylistSaved { .. }
                | Self::DeviceSelectionAdded { .. }
                | Self::PlaylistSelectionAppended { .. }
                | Self::LibraryMutationRejected { .. }
                | Self::DaemonShutdownStarted { .. }
                | Self::CommandFailed { .. }
        )
    }

    pub(super) fn worker_route(&self) -> Option<super::WorkerEventRoute<'_>> {
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
            } => Some(super::WorkerEventRoute::Device(device_id, *session_id)),
            Self::LibraryScanStarted { session_id, .. }
            | Self::LibraryScanProgress { session_id, .. }
            | Self::LibraryScanFinished { session_id, .. } => {
                Some(super::WorkerEventRoute::LibraryScan(*session_id))
            }
            Self::GlobalConfig { .. }
            | Self::SourceAvailability { .. }
            | Self::DeviceInventory { .. }
            | Self::InventorySubscriptionChanged { .. }
            | Self::DeviceConfig { .. }
            | Self::ConfigMutationFailed { .. }
            | Self::DeviceForgotten { .. }
            | Self::SyncAccepted { .. }
            | Self::SyncRejected { .. }
            | Self::History { .. }
            | Self::Library { .. }
            | Self::SelectionPreview { .. }
            | Self::DevicePreview { .. }
            | Self::ResolvedTracks { .. }
            | Self::Playlists { .. }
            | Self::PlaylistDetail { .. }
            | Self::PlaylistSaved { .. }
            | Self::DeviceSelectionAdded { .. }
            | Self::PlaylistSelectionAppended { .. }
            | Self::LibraryMutationRejected { .. }
            | Self::DaemonShutdownStarted { .. }
            | Self::CommandFailed { .. } => None,
        }
    }
}
