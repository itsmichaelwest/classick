//! Immutable facts carried by each admitted daemon session.

use crate::daemon::history::SyncTrigger;
use crate::ipc_device::SessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSession {
    pub id: SessionId,
    pub started_at_unix_secs: u64,
    pub trigger: SyncTrigger,
    pub serial: Option<String>,
    pub drive: Option<String>,
    pub kind: SessionKind,
}

/// Whether the occupied `Syncing` state is a real sync or a library scan.
/// Both share the single guard so they never run concurrently (no SMB
/// contention, no index-file races); the label distinguishes them for the
/// UI's `status_update.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Sync,
    Scan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_shape_carries_authoritative_identity_and_context() {
        let session = SyncSession {
            id: 7,
            started_at_unix_secs: 11,
            trigger: SyncTrigger::PlugIn,
            serial: Some("0xABC".to_string()),
            drive: Some("G:\\".to_string()),
            kind: SessionKind::Sync,
        };

        assert_eq!(session.id, 7);
        assert_eq!(session.serial.as_deref(), Some("0xABC"));
        assert_eq!(session.drive.as_deref(), Some("G:\\"));
    }
}
