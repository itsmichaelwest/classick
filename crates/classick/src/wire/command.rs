use super::{PromptId, RequestId, SessionId};
use crate::device::DeviceId;
use crate::portable::outbox::PendingMutation;
use crate::portable::profile::{MutationId, SelectionValue, SettingsValue, SubscriptionsValue};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireCommand {
    GetGlobalConfig {
        request_id: RequestId,
    },
    SetSourceLocation {
        request_id: RequestId,
        source_root: Option<super::SourceRoot>,
    },
    SetGlobalSettings {
        request_id: RequestId,
        settings: super::GlobalSettings,
    },
    GetInventory {
        request_id: RequestId,
    },
    SubscribeInventory {
        request_id: RequestId,
    },
    UnsubscribeInventory {
        request_id: RequestId,
    },
    AdoptDevice {
        device_id: DeviceId,
        request_id: RequestId,
        selection_mutation_id: MutationId,
        selection: SelectionValue,
        settings_mutation_id: MutationId,
        settings: SettingsValue,
        subscriptions_mutation_id: MutationId,
        subscriptions: SubscriptionsValue,
    },
    ForgetDevice {
        device_id: DeviceId,
        request_id: RequestId,
    },
    GetDeviceConfig {
        device_id: DeviceId,
        request_id: RequestId,
    },
    SetSelection {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        selection: SelectionValue,
    },
    SetSettings {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        settings: SettingsValue,
    },
    SetSubscriptions {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        subscriptions: SubscriptionsValue,
    },
    TriggerSync {
        device_id: DeviceId,
        request_id: RequestId,
        trigger: super::SyncTrigger,
    },
    BackfillRockbox {
        device_id: DeviceId,
        request_id: RequestId,
    },
    ReplaceLibrary {
        device_id: DeviceId,
        request_id: RequestId,
    },
    GetHistory {
        request_id: RequestId,
        limit: u32,
    },
    GetLibrary {
        request_id: RequestId,
    },
    ScanLibrary {
        request_id: RequestId,
    },
    RetrySourceMount {
        request_id: RequestId,
        allow_ui: bool,
    },
    PreviewSelection {
        device_id: DeviceId,
        request_id: RequestId,
        selection: SelectionValue,
    },
    PreviewDevice {
        device_id: DeviceId,
        request_id: RequestId,
    },
    ResolveTracks {
        request_id: RequestId,
        rules: Vec<crate::portable::profile::SelectionRule>,
    },
    AddSelectionToDevice {
        device_id: DeviceId,
        request_id: RequestId,
        mutation_id: MutationId,
        rules: Vec<crate::portable::profile::SelectionRule>,
    },
    ListPlaylists {
        request_id: RequestId,
    },
    GetPlaylist {
        request_id: RequestId,
        slug: crate::portable::profile::PlaylistSlug,
    },
    SavePlaylist {
        request_id: RequestId,
        playlist: super::PlaylistDraft,
    },
    DeletePlaylist {
        request_id: RequestId,
        slug: crate::portable::profile::PlaylistSlug,
    },
    AppendSelectionToPlaylist {
        request_id: RequestId,
        slug: crate::portable::profile::PlaylistSlug,
        rules: Vec<crate::portable::profile::SelectionRule>,
    },
    Shutdown {
        request_id: RequestId,
    },
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
            Self::GetGlobalConfig { .. } => super::MessageKind::GetGlobalConfig,
            Self::SetSourceLocation { .. } => super::MessageKind::SetSourceLocation,
            Self::SetGlobalSettings { .. } => super::MessageKind::SetGlobalSettings,
            Self::GetInventory { .. } => super::MessageKind::GetInventory,
            Self::SubscribeInventory { .. } => super::MessageKind::SubscribeInventory,
            Self::UnsubscribeInventory { .. } => super::MessageKind::UnsubscribeInventory,
            Self::AdoptDevice { .. } => super::MessageKind::AdoptDevice,
            Self::ForgetDevice { .. } => super::MessageKind::ForgetDevice,
            Self::GetDeviceConfig { .. } => super::MessageKind::GetDeviceConfig,
            Self::SetSelection { .. } => super::MessageKind::SetSelection,
            Self::SetSettings { .. } => super::MessageKind::SetSettings,
            Self::SetSubscriptions { .. } => super::MessageKind::SetSubscriptions,
            Self::TriggerSync { .. } => super::MessageKind::TriggerSync,
            Self::BackfillRockbox { .. } => super::MessageKind::BackfillRockbox,
            Self::ReplaceLibrary { .. } => super::MessageKind::ReplaceLibrary,
            Self::GetHistory { .. } => super::MessageKind::GetHistory,
            Self::GetLibrary { .. } => super::MessageKind::GetLibrary,
            Self::ScanLibrary { .. } => super::MessageKind::ScanLibrary,
            Self::RetrySourceMount { .. } => super::MessageKind::RetrySourceMount,
            Self::PreviewSelection { .. } => super::MessageKind::PreviewSelection,
            Self::PreviewDevice { .. } => super::MessageKind::PreviewDevice,
            Self::ResolveTracks { .. } => super::MessageKind::ResolveTracks,
            Self::AddSelectionToDevice { .. } => super::MessageKind::AddSelectionToDevice,
            Self::ListPlaylists { .. } => super::MessageKind::ListPlaylists,
            Self::GetPlaylist { .. } => super::MessageKind::GetPlaylist,
            Self::SavePlaylist { .. } => super::MessageKind::SavePlaylist,
            Self::DeletePlaylist { .. } => super::MessageKind::DeletePlaylist,
            Self::AppendSelectionToPlaylist { .. } => super::MessageKind::AppendSelectionToPlaylist,
            Self::Shutdown { .. } => super::MessageKind::Shutdown,
            Self::ApplyReview { .. } => super::MessageKind::ApplyReview,
            Self::DryRunReview { .. } => super::MessageKind::DryRunReview,
            Self::QuitReview { .. } => super::MessageKind::QuitReview,
            Self::PromptDecision { .. } => super::MessageKind::PromptDecision,
            Self::FormDecision { .. } => super::MessageKind::FormDecision,
            Self::CancelSync { .. } => super::MessageKind::CancelSync,
            Self::PauseSync { .. } => super::MessageKind::PauseSync,
        }
    }

    pub(super) fn session_route(&self) -> Option<(&DeviceId, SessionId)> {
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
            } => Some((device_id, *session_id)),
            _ => None,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::AdoptDevice {
                device_id,
                selection_mutation_id,
                selection,
                settings_mutation_id,
                settings,
                subscriptions_mutation_id,
                subscriptions,
                ..
            } => {
                if selection_mutation_id == settings_mutation_id
                    || selection_mutation_id == subscriptions_mutation_id
                    || settings_mutation_id == subscriptions_mutation_id
                {
                    bail!("adoption mutation IDs must be unique");
                }
                PendingMutation::selection(
                    selection_mutation_id.clone(),
                    device_id.clone(),
                    selection.clone(),
                    0,
                )?;
                PendingMutation::settings(
                    settings_mutation_id.clone(),
                    device_id.clone(),
                    settings.clone(),
                    0,
                )?;
                PendingMutation::subscriptions(
                    subscriptions_mutation_id.clone(),
                    device_id.clone(),
                    subscriptions.clone(),
                    0,
                )?;
            }
            Self::SetSelection {
                device_id,
                mutation_id,
                selection,
                ..
            } => {
                PendingMutation::selection(
                    mutation_id.clone(),
                    device_id.clone(),
                    selection.clone(),
                    0,
                )?;
            }
            Self::SetSettings {
                device_id,
                mutation_id,
                settings,
                ..
            } => {
                PendingMutation::settings(
                    mutation_id.clone(),
                    device_id.clone(),
                    settings.clone(),
                    0,
                )?;
            }
            Self::SetSubscriptions {
                device_id,
                mutation_id,
                subscriptions,
                ..
            } => {
                PendingMutation::subscriptions(
                    mutation_id.clone(),
                    device_id.clone(),
                    subscriptions.clone(),
                    0,
                )?;
            }
            Self::GetHistory { limit, .. } if !(1..=50).contains(limit) => {
                bail!("history limit must be between 1 and 50")
            }
            Self::PreviewSelection { selection, .. } => {
                if selection.schema_version != 1 {
                    bail!("unsupported selection schema");
                }
                super::library::validate_selection_rules(&selection.rules)?;
            }
            Self::ResolveTracks { rules, .. } => {
                super::library::validate_selection_rules(rules)?;
            }
            Self::AddSelectionToDevice { rules, .. }
            | Self::AppendSelectionToPlaylist { rules, .. } => {
                if rules.is_empty() {
                    bail!("library mutation requires at least one selection rule");
                }
                super::library::validate_selection_rules(rules)?;
            }
            Self::SavePlaylist { playlist, .. } => playlist.validate()?,
            _ => {}
        }
        Ok(())
    }
}
