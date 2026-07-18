use crate::daemon::history::SyncTrigger;
use crate::daemon::session_admission::{AdmissionRejection, SessionAdmission};
use crate::daemon::state::SyncSession;
use crate::ipc_device::SessionId;
use std::collections::BTreeMap;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct SessionControls {
    cancel: Option<oneshot::Sender<()>>,
    pause: Option<oneshot::Sender<()>>,
    prompt: mpsc::UnboundedSender<(u64, i32)>,
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
}

impl RuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            admission: SessionAdmission::single(),
            controls: BTreeMap::new(),
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

    pub(crate) fn active_session(&self) -> Option<&SyncSession> {
        self.admission.active_session()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.admission.is_idle()
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
}
