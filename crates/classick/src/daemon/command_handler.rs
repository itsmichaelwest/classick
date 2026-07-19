//! Serial-target and active-session command admission.

use crate::daemon::device_registry::{canonical_serial_key, DeviceRegistry};
use crate::daemon::runtime_state::RuntimeState;
use crate::daemon::state::SessionKind;
use crate::ipc_daemon::DaemonEvent;
use crate::ipc_daemon::{DaemonCommand, SyncRejectReason};

pub(crate) fn command_failed(request_id: String, error: impl Into<String>) -> DaemonEvent {
    DaemonEvent::CommandFailed {
        acknowledged_request_id: request_id,
        error: error.into(),
    }
}

pub(crate) fn same_serial(left: &str, right: &str) -> bool {
    canonical_serial_key(left) == canonical_serial_key(right)
}

pub(crate) fn target_rejection(
    command: &DaemonCommand,
    registry: &DeviceRegistry,
    state: &RuntimeState,
) -> Option<SyncRejectReason> {
    let requested = command.target_serial()?;
    let Some(record) = registry.record(requested) else {
        return Some(SyncRejectReason::NotConfigured);
    };
    if !record.configured {
        return Some(SyncRejectReason::NotConfigured);
    }

    match command {
        DaemonCommand::CancelSync { .. }
        | DaemonCommand::Pause { .. }
        | DaemonCommand::DecidePrompt { .. } => {
            let active_serial = state
                .active_session()
                .filter(|session| session.kind == SessionKind::Sync)
                .and_then(|session| session.serial.as_deref());
            if active_serial.is_some_and(|active| same_serial(requested, active)) {
                None
            } else {
                Some(SyncRejectReason::AlreadySyncing)
            }
        }
        DaemonCommand::TriggerSync { .. }
        | DaemonCommand::BackfillRockbox { .. }
        | DaemonCommand::ReplaceLibrary { .. } => {
            if state.connected_device(requested).is_none() {
                Some(SyncRejectReason::NoIpod)
            } else if !state.is_idle() {
                Some(SyncRejectReason::AlreadySyncing)
            } else {
                None
            }
        }
        DaemonCommand::ForgetIpod { .. } => {
            if state.active_session().is_some_and(|session| {
                session
                    .serial
                    .as_deref()
                    .is_some_and(|active| same_serial(requested, active))
            }) {
                Some(SyncRejectReason::AlreadySyncing)
            } else {
                None
            }
        }
        DaemonCommand::PreviewSelection { .. }
        | DaemonCommand::GetDeviceConfig { .. }
        | DaemonCommand::SaveDeviceConfig { .. }
        | DaemonCommand::PreviewDevice { .. } => None,
        _ => return None,
    }
}
