use crate::daemon::device_registry::canonical_serial_key;
use crate::daemon::history::SyncTrigger;
use crate::daemon::session_admission::{AdmissionRejection, SessionAdmission};
use crate::daemon::state::SyncSession;
use crate::ipc_device::SessionId;
use crate::ipod::device::DetectedIpod;
use std::collections::BTreeMap;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct SessionControls {
    cancel: Option<oneshot::Sender<()>>,
    pause: Option<oneshot::Sender<()>>,
    prompt: mpsc::UnboundedSender<(u64, i32)>,
}

pub(crate) struct DetachedTerminalIntent {
    pub(crate) entry: crate::daemon::history::HistoryEntry,
    pub(crate) persisted: bool,
}

impl SessionControls {
    pub(crate) fn new(
        cancel: oneshot::Sender<()>,
        pause: oneshot::Sender<()>,
        prompt: mpsc::UnboundedSender<(u64, i32)>,
    ) -> Self {
        Self {
            cancel: Some(cancel),
            pause: Some(pause),
            prompt,
        }
    }
}

pub(crate) struct RuntimeState {
    admission: SessionAdmission,
    controls: BTreeMap<SessionId, SessionControls>,
    connected: BTreeMap<String, DetectedIpod>,
    connection_generations: BTreeMap<String, u64>,
    next_connection_generation: u64,
    detached_terminal_intents: BTreeMap<SessionId, DetachedTerminalIntent>,
    retained_terminal_attempts: BTreeMap<String, crate::daemon::history::HistoryEntry>,
    terminal_persistence_errors: BTreeMap<String, String>,
}

impl RuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            admission: SessionAdmission::single(),
            controls: BTreeMap::new(),
            connected: BTreeMap::new(),
            connection_generations: BTreeMap::new(),
            next_connection_generation: 0,
            detached_terminal_intents: BTreeMap::new(),
            retained_terminal_attempts: BTreeMap::new(),
            terminal_persistence_errors: BTreeMap::new(),
        }
    }

    pub(crate) fn try_admit_device(
        &mut self,
        trigger: SyncTrigger,
        serial: &str,
        drive: &Path,
    ) -> Result<SyncSession, AdmissionRejection> {
        self.admission
            .try_admit_device_with_trigger(trigger, serial, drive)
    }

    pub(crate) fn try_admit_scan(&mut self) -> Result<SyncSession, AdmissionRejection> {
        self.admission.try_admit_scan()
    }

    pub(crate) fn install_controls(&mut self, id: SessionId, controls: SessionControls) {
        assert!(
            self.admission.session(id).is_some(),
            "cannot install controls for an unknown session {id}"
        );
        assert!(
            self.controls.insert(id, controls).is_none(),
            "controls already installed for session {id}"
        );
    }

    pub(crate) fn finish(&mut self, id: SessionId) -> Option<SyncSession> {
        let session = self.admission.session(id)?.clone();
        if !self.admission.finish(id) {
            return None;
        }
        self.controls.remove(&id);
        Some(session)
    }

    pub(crate) fn signal_cancel(&mut self, id: SessionId) -> bool {
        if self.admission.session(id).is_none() {
            return false;
        }
        if let Some(cancel) = self.take_cancel(id) {
            let _ = cancel.send(());
        }
        true
    }

    pub(crate) fn record_detached_terminal_intent(
        &mut self,
        id: SessionId,
        entry: crate::daemon::history::HistoryEntry,
        persisted: bool,
    ) {
        assert!(
            self.admission.session(id).is_some(),
            "cannot detach an unknown session {id}"
        );
        let previous = self
            .detached_terminal_intents
            .insert(id, DetachedTerminalIntent { entry, persisted });
        assert!(previous.is_none(), "session {id} was already detached");
    }

    pub(crate) fn take_detached_terminal_intent(
        &mut self,
        id: SessionId,
    ) -> Option<DetachedTerminalIntent> {
        self.detached_terminal_intents.remove(&id)
    }

    pub(crate) fn active_session(&self) -> Option<&SyncSession> {
        self.admission.active_session()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.admission.is_idle()
    }

    pub(crate) fn connect(&mut self, device: DetectedIpod) -> Option<DetectedIpod> {
        self.next_connection_generation = self
            .next_connection_generation
            .checked_add(1)
            .expect("device connection generation space exhausted");
        let key = canonical_serial_key(&device.serial);
        self.connection_generations
            .insert(key.clone(), self.next_connection_generation);
        self.connected.insert(key, device)
    }

    pub(crate) fn disconnect(&mut self, serial: &str) -> Option<DetectedIpod> {
        let key = canonical_serial_key(serial);
        self.connection_generations.remove(&key);
        self.connected.remove(&key)
    }

    pub(crate) fn connected_device(&self, serial: &str) -> Option<&DetectedIpod> {
        self.connected.get(&canonical_serial_key(serial))
    }

    pub(crate) fn connected_device_mut(&mut self, serial: &str) -> Option<&mut DetectedIpod> {
        self.connected.get_mut(&canonical_serial_key(serial))
    }

    pub(crate) fn connected_devices(&self) -> impl Iterator<Item = &DetectedIpod> {
        self.connected.values()
    }

    pub(crate) fn connection_generation(&self, serial: &str) -> Option<u64> {
        self.connection_generations
            .get(&canonical_serial_key(serial))
            .copied()
    }

    pub(crate) fn retain_terminal_attempt(
        &mut self,
        entry: crate::daemon::history::HistoryEntry,
        persistence_error: String,
    ) {
        let key = canonical_serial_key(&entry.serial);
        self.retained_terminal_attempts.insert(key.clone(), entry);
        self.terminal_persistence_errors
            .insert(key, persistence_error);
    }

    pub(crate) fn clear_retained_terminal_attempt(&mut self, serial: &str) {
        let key = canonical_serial_key(serial);
        self.retained_terminal_attempts.remove(&key);
        self.terminal_persistence_errors.remove(&key);
    }

    pub(crate) fn retained_terminal_attempt(
        &self,
        serial: &str,
    ) -> Option<&crate::daemon::history::HistoryEntry> {
        self.retained_terminal_attempts
            .get(&canonical_serial_key(serial))
    }

    pub(crate) fn terminal_persistence_error(&self, serial: &str) -> Option<&str> {
        self.terminal_persistence_errors
            .get(&canonical_serial_key(serial))
            .map(String::as_str)
    }

    pub(crate) fn take_cancel(&mut self, id: SessionId) -> Option<oneshot::Sender<()>> {
        self.controls.get_mut(&id)?.cancel.take()
    }

    pub(crate) fn take_pause(&mut self, id: SessionId) -> Option<oneshot::Sender<()>> {
        self.controls.get_mut(&id)?.pause.take()
    }

    pub(crate) fn prompt_sender(&self, id: SessionId) -> Option<mpsc::UnboundedSender<(u64, i32)>> {
        self.controls
            .get(&id)
            .map(|controls| controls.prompt.clone())
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::history::SyncTrigger;
    use std::path::Path;
    use tokio::sync::{mpsc, oneshot};

    fn controls() -> (SessionControls, oneshot::Receiver<()>) {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (pause_tx, _pause_rx) = oneshot::channel();
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        (
            SessionControls::new(cancel_tx, pause_tx, prompt_tx),
            cancel_rx,
        )
    }

    #[test]
    fn stale_session_id_cannot_take_active_b_control_or_finish_b() {
        let mut state = RuntimeState::new();
        let a = state
            .try_admit_device(SyncTrigger::Manual, "RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");
        let (a_controls, _a_cancel_rx) = controls();
        state.install_controls(a.id, a_controls);
        assert_eq!(state.finish(a.id), Some(a.clone()));

        let b = state
            .try_admit_device(SyncTrigger::Manual, "RAW-B", Path::new("/Volumes/B"))
            .expect("B admitted");
        let (b_controls, mut b_cancel_rx) = controls();
        state.install_controls(b.id, b_controls);

        assert!(state.take_cancel(a.id).is_none());
        assert_eq!(state.finish(a.id), None);
        assert_eq!(state.active_session(), Some(&b));
        assert!(b_cancel_rx.try_recv().is_err());

        state
            .take_cancel(b.id)
            .expect("B cancel remains keyed to B")
            .send(())
            .expect("B receiver alive");
        assert_eq!(b_cancel_rx.try_recv(), Ok(()));
    }

    #[test]
    fn cancelling_keeps_the_session_admitted_until_matching_completion() {
        let mut state = RuntimeState::new();
        let session = state
            .try_admit_device(SyncTrigger::Manual, "RAW-A", Path::new("/Volumes/A"))
            .expect("A admitted");
        let (controls, mut cancel_rx) = controls();
        state.install_controls(session.id, controls);

        assert!(state.signal_cancel(session.id));
        assert_eq!(cancel_rx.try_recv(), Ok(()));
        assert_eq!(state.active_session(), Some(&session));
        assert_eq!(
            state.try_admit_device(SyncTrigger::Manual, "RAW-B", Path::new("/Volumes/B")),
            Err(AdmissionRejection::AtCapacity {
                active_session_id: session.id,
            })
        );

        assert_eq!(state.finish(session.id), Some(session));
        assert!(state.is_idle());
    }
}
