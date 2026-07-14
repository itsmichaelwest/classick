//! Daemon state machine: tracks whether a sync is currently in flight
//! and centralizes the "should this trigger be accepted?" policy. Per
//! spec §4: concurrent triggers during Syncing are dropped (not queued).

use crate::daemon::history::SyncTrigger;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    Idle,
    Syncing(SyncSession),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSession {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerOutcome {
    Accepted,
    DroppedAlreadySyncing,
}

pub struct StateMachine {
    state: DaemonState,
}

impl StateMachine {
    pub fn new() -> Self { Self { state: DaemonState::Idle } }

    pub fn state(&self) -> &DaemonState { &self.state }

    pub fn is_idle(&self) -> bool { matches!(self.state, DaemonState::Idle) }

    /// Try to accept a sync trigger. Returns `Accepted` if state was Idle
    /// (and transitions to Syncing); returns `DroppedAlreadySyncing` if
    /// state was Syncing (state unchanged).
    pub fn try_start_sync(&mut self, trigger: SyncTrigger) -> TriggerOutcome {
        self.try_start_sync_inner(trigger, None, None, SessionKind::Sync)
    }

    pub fn try_start_sync_for_device(
        &mut self,
        trigger: SyncTrigger,
        serial: String,
        drive: String,
    ) -> TriggerOutcome {
        self.try_start_sync_inner(trigger, Some(serial), Some(drive), SessionKind::Sync)
    }

    /// A library scan occupies the same guard as a sync — they never run
    /// concurrently (no SMB contention, no index-file races).
    pub fn try_start_scan(&mut self) -> TriggerOutcome {
        self.try_start_sync_inner(SyncTrigger::Manual, None, None, SessionKind::Scan)
    }

    fn try_start_sync_inner(
        &mut self,
        trigger: SyncTrigger,
        serial: Option<String>,
        drive: Option<String>,
        kind: SessionKind,
    ) -> TriggerOutcome {
        match &self.state {
            DaemonState::Idle => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                self.state = DaemonState::Syncing(SyncSession {
                    started_at_unix_secs: now,
                    trigger,
                    serial,
                    drive,
                    kind,
                });
                TriggerOutcome::Accepted
            }
            DaemonState::Syncing(_) => TriggerOutcome::DroppedAlreadySyncing,
        }
    }

    /// Called when the sync subprocess finishes (success or failure).
    /// Returns the SyncSession that was active.
    pub fn finish_sync(&mut self) -> Option<SyncSession> {
        match std::mem::replace(&mut self.state, DaemonState::Idle) {
            DaemonState::Syncing(s) => Some(s),
            DaemonState::Idle => None,
        }
    }
}

impl Default for StateMachine {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_idle() {
        let sm = StateMachine::new();
        assert!(sm.is_idle());
    }

    #[test]
    fn try_start_accepts_when_idle() {
        let mut sm = StateMachine::new();
        let result = sm.try_start_sync(SyncTrigger::PlugIn);
        assert_eq!(result, TriggerOutcome::Accepted);
        assert!(matches!(sm.state(), DaemonState::Syncing(_)));
    }

    #[test]
    fn try_start_drops_when_syncing() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::PlugIn);
        let result = sm.try_start_sync(SyncTrigger::Scheduled);
        assert_eq!(result, TriggerOutcome::DroppedAlreadySyncing);
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.trigger, SyncTrigger::PlugIn);
        } else {
            panic!("expected Syncing");
        }
    }

    #[test]
    fn finish_returns_session_and_resets_to_idle() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::Manual);
        let session = sm.finish_sync().expect("session present");
        assert_eq!(session.trigger, SyncTrigger::Manual);
        assert!(sm.is_idle());
    }

    #[test]
    fn finish_from_idle_returns_none() {
        let mut sm = StateMachine::new();
        assert!(sm.finish_sync().is_none());
        assert!(sm.is_idle());
    }

    #[test]
    fn session_carries_drive_and_serial() {
        let mut sm = StateMachine::new();
        sm.try_start_sync_for_device(SyncTrigger::PlugIn, "0xABC".to_string(), "G:\\".to_string());
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.serial.as_deref(), Some("0xABC"));
            assert_eq!(s.drive.as_deref(), Some("G:\\"));
        } else {
            panic!("expected Syncing");
        }
    }

    #[test]
    fn scan_session_carries_scan_kind_and_shares_the_guard() {
        let mut sm = StateMachine::new();
        assert_eq!(sm.try_start_scan(), TriggerOutcome::Accepted);
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.kind, SessionKind::Scan);
        } else { panic!("expected Syncing (shared guard)"); }
        // A sync while scanning is dropped — the guard is shared.
        assert_eq!(sm.try_start_sync(SyncTrigger::Manual), TriggerOutcome::DroppedAlreadySyncing);
        sm.finish_sync();
        assert!(sm.is_idle());
    }

    #[test]
    fn sync_sessions_default_to_sync_kind() {
        let mut sm = StateMachine::new();
        sm.try_start_sync(SyncTrigger::Manual);
        if let DaemonState::Syncing(s) = sm.state() {
            assert_eq!(s.kind, SessionKind::Sync);
        } else { panic!(); }
    }

    #[test]
    fn try_start_sync_without_device_still_works() {
        // Manual triggers without an attached device set serial/drive to None.
        let mut sm = StateMachine::new();
        let outcome = sm.try_start_sync(SyncTrigger::Manual);
        assert_eq!(outcome, TriggerOutcome::Accepted);
        if let DaemonState::Syncing(s) = sm.state() {
            assert!(s.serial.is_none());
            assert!(s.drive.is_none());
        }
    }
}
