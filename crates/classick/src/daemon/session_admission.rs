use crate::daemon::history::SyncTrigger;
use crate::daemon::state::SessionKind;
pub use crate::daemon::state::SyncSession;
use crate::ipc_daemon::DaemonEvent;
pub use crate::ipc_device::SessionId;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventContext {
    pub session_id: SessionId,
    pub serial: Option<String>,
}

impl EventContext {
    pub(crate) fn wrap(&self, line: String) -> DaemonEvent {
        DaemonEvent::SyncEvent {
            line,
            serial: self.serial.clone(),
            session_id: self.session_id,
        }
    }
}

impl From<&SyncSession> for EventContext {
    fn from(session: &SyncSession) -> Self {
        Self {
            session_id: session.id,
            serial: session.serial.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRejection {
    AtCapacity { active_session_id: SessionId },
}

pub struct SessionAdmission {
    capacity: usize,
    sessions: BTreeMap<SessionId, SyncSession>,
}

impl SessionAdmission {
    pub fn single() -> Self {
        Self {
            capacity: 1,
            sessions: BTreeMap::new(),
        }
    }

    pub fn try_admit_device(
        &mut self,
        serial: &str,
        drive: &Path,
    ) -> Result<SyncSession, AdmissionRejection> {
        self.try_admit_device_with_trigger(SyncTrigger::Manual, serial, drive)
    }

    pub(crate) fn try_admit_device_with_trigger(
        &mut self,
        trigger: SyncTrigger,
        serial: &str,
        drive: &Path,
    ) -> Result<SyncSession, AdmissionRejection> {
        self.try_admit(
            trigger,
            Some(serial.to_string()),
            Some(drive.to_string_lossy().into_owned()),
            SessionKind::Sync,
        )
    }

    pub(crate) fn try_admit_scan(&mut self) -> Result<SyncSession, AdmissionRejection> {
        self.try_admit(SyncTrigger::Manual, None, None, SessionKind::Scan)
    }

    fn try_admit(
        &mut self,
        trigger: SyncTrigger,
        serial: Option<String>,
        drive: Option<String>,
        kind: SessionKind,
    ) -> Result<SyncSession, AdmissionRejection> {
        if self.sessions.len() >= self.capacity {
            let active_session_id = self
                .sessions
                .keys()
                .next()
                .copied()
                .expect("occupied admission has an active session");
            return Err(AdmissionRejection::AtCapacity { active_session_id });
        }

        let id = NEXT_SESSION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("daemon session id space exhausted");
        let started_at_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let session = SyncSession {
            id,
            started_at_unix_secs,
            trigger,
            serial,
            drive,
            kind,
        };
        let previous = self.sessions.insert(id, session.clone());
        debug_assert!(previous.is_none());
        Ok(session)
    }

    pub fn finish(&mut self, id: SessionId) -> bool {
        self.sessions.remove(&id).is_some()
    }

    pub(crate) fn session(&self, id: SessionId) -> Option<&SyncSession> {
        self.sessions.get(&id)
    }

    pub(crate) fn active_session(&self) -> Option<&SyncSession> {
        self.sessions.values().next()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.sessions.is_empty()
    }
}

impl Default for SessionAdmission {
    fn default() -> Self {
        Self::single()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::history::SyncTrigger;
    use crate::daemon::state::SessionKind;
    use crate::ipc_daemon::DaemonEvent;
    use std::path::Path;

    #[test]
    fn starts_idle_and_finishing_an_unknown_id_is_a_no_op() {
        let mut admission = SessionAdmission::single();

        assert!(admission.is_idle());
        assert!(!admission.finish(u64::MAX));
        assert!(admission.is_idle());
    }

    #[test]
    fn device_session_preserves_trigger_drive_and_kind() {
        let mut admission = SessionAdmission::single();
        let session = admission
            .try_admit_device_with_trigger(SyncTrigger::PlugIn, "RAW-A", Path::new("/Volumes/A"))
            .expect("device admitted");

        assert_eq!(session.trigger, SyncTrigger::PlugIn);
        assert_eq!(session.drive.as_deref(), Some("/Volumes/A"));
        assert_eq!(session.kind, SessionKind::Sync);
    }

    #[test]
    fn finishing_a_scan_releases_capacity_for_a_device() {
        let mut admission = SessionAdmission::single();
        let scan = admission.try_admit_scan().expect("scan admitted");

        assert!(admission.finish(scan.id));
        assert!(admission
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .is_ok());
    }

    #[test]
    fn separate_admission_policies_share_the_process_session_id_epoch() {
        let mut first = SessionAdmission::single();
        let mut second = SessionAdmission::single();

        let a = first
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");
        let b = second
            .try_admit_device("RAW-B", Path::new("/Volumes/B"))
            .expect("B admitted");

        assert_ne!(a.id, b.id);
    }

    #[test]
    fn capacity_one_rejects_a_second_device_session() {
        let mut admission = SessionAdmission::single();
        let a = admission
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");

        assert_eq!(
            admission.try_admit_device("RAW-B", Path::new("/Volumes/B")),
            Err(AdmissionRejection::AtCapacity {
                active_session_id: a.id,
            })
        );
        assert_eq!(admission.active_session(), Some(&a));
    }

    #[test]
    fn finishing_a_releases_capacity_for_b() {
        let mut admission = SessionAdmission::single();
        let a = admission
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");

        assert!(admission.finish(a.id));
        let b = admission
            .try_admit_device("RAW-B", Path::new("/Volumes/B"))
            .expect("B admitted after A finished");

        assert_ne!(a.id, 0);
        assert_ne!(b.id, 0);
        assert_ne!(a.id, b.id);
        assert_eq!(b.serial.as_deref(), Some("RAW-B"));
    }

    #[test]
    fn stale_a_completion_cannot_finish_b() {
        let mut admission = SessionAdmission::single();
        let a = admission
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");
        assert!(admission.finish(a.id));
        let b = admission
            .try_admit_device("RAW-B", Path::new("/Volumes/B"))
            .expect("B admitted");

        assert!(!admission.finish(a.id));
        assert_eq!(admission.active_session(), Some(&b));
    }

    #[test]
    fn admitted_a_context_attributes_progress_to_a_and_its_session() {
        let mut admission = SessionAdmission::single();
        let a = admission
            .try_admit_device("RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");
        let context = EventContext::from(&a);

        assert_eq!(context.session_id, a.id);
        assert_eq!(context.serial.as_deref(), Some("RAW-A"));
        assert!(matches!(
            context.wrap("{\"type\":\"track_done\"}".to_string()),
            DaemonEvent::SyncEvent {
                serial: Some(serial),
                session_id,
                ..
            } if serial == "RAW-A" && session_id == a.id
        ));
    }

    #[test]
    fn serial_less_scan_occupies_the_same_capacity() {
        let mut admission = SessionAdmission::single();
        let scan = admission.try_admit_scan().expect("scan admitted");

        assert_eq!(scan.serial, None);
        assert_eq!(scan.drive, None);
        assert_eq!(scan.kind, SessionKind::Scan);
        assert_eq!(scan.trigger, SyncTrigger::Manual);
        assert!(matches!(
            EventContext::from(&scan).wrap("{\"type\":\"log\"}".to_string()),
            DaemonEvent::SyncEvent {
                serial: None,
                session_id,
                ..
            } if session_id == scan.id
        ));
        assert!(matches!(
            admission.try_admit_device("RAW-A", Path::new("/Volumes/A")),
            Err(AdmissionRejection::AtCapacity { active_session_id })
                if active_session_id == scan.id
        ));
    }
}
