//! Serial-target and active-session command admission.

use crate::daemon::device_registry::canonical_serial_key;
use crate::daemon::runtime_state::RuntimeState;
use crate::daemon::state::SessionKind;
use crate::ipc_daemon::{DaemonCommand, SyncRejectReason};
use crate::ipod::device::DetectedIpod;

pub(crate) fn same_serial(left: &str, right: &str) -> bool {
    canonical_serial_key(left) == canonical_serial_key(right)
}

pub(crate) fn singleton_target_rejection(
    command: &DaemonCommand,
    connected: &Option<DetectedIpod>,
    configured_serial: Option<&str>,
    state: &RuntimeState,
) -> Option<SyncRejectReason> {
    let requested = command.target_serial()?;
    let connected_serial = connected.as_ref().map(|device| device.serial.as_str());

    if configured_serial
        .zip(connected_serial)
        .is_some_and(|(configured, connected)| !same_serial(configured, connected))
    {
        return Some(SyncRejectReason::NotConfigured);
    }

    let (expected, missing_or_mismatch) = match command {
        DaemonCommand::CancelSync { .. }
        | DaemonCommand::Pause { .. }
        | DaemonCommand::DecidePrompt { .. } => {
            let active_serial = state
                .active_session()
                .filter(|session| session.kind == SessionKind::Sync)
                .and_then(|session| session.serial.as_deref());
            (active_serial, SyncRejectReason::AlreadySyncing)
        }
        DaemonCommand::TriggerSync { .. }
        | DaemonCommand::BackfillRockbox { .. }
        | DaemonCommand::ReplaceLibrary { .. } => {
            if configured_serial.is_none() {
                return Some(SyncRejectReason::NotConfigured);
            }
            (connected_serial, SyncRejectReason::NoIpod)
        }
        DaemonCommand::ForgetIpod { .. }
        | DaemonCommand::PreviewSelection { .. }
        | DaemonCommand::GetDeviceConfig { .. }
        | DaemonCommand::SaveDeviceConfig { .. }
        | DaemonCommand::PreviewDevice { .. } => {
            (configured_serial, SyncRejectReason::NotConfigured)
        }
        _ => return None,
    };

    match expected {
        Some(actual) if same_serial(requested, actual) => None,
        _ => Some(missing_or_mismatch),
    }
}
