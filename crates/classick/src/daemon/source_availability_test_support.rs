use super::{BoxFuture, MountInteraction, SourceMountBackend, SourceUnavailable};
use crate::portable_path::PortablePath;
use crate::source_location::{SourceIdentity, SourceLocation};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct TestDir(PathBuf);

impl TestDir {
    pub(super) fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "classick-source-availability-{}-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed),
            label
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
pub(super) struct FakeBackend {
    calls: Arc<AtomicUsize>,
    pub(super) interactions: Arc<Mutex<Vec<MountInteraction>>>,
    pub(super) responses: Arc<Mutex<VecDeque<Result<PathBuf, SourceUnavailable>>>>,
    pub(super) gate: Option<Arc<Semaphore>>,
}

impl FakeBackend {
    pub(super) fn responding_with(responses: Vec<Result<PathBuf, SourceUnavailable>>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            interactions: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses.into())),
            gate: None,
        }
    }

    pub(super) fn gated(response: Result<PathBuf, SourceUnavailable>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            interactions: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(VecDeque::from([response]))),
            gate: Some(Arc::new(Semaphore::new(0))),
        }
    }

    pub(super) fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SourceMountBackend for FakeBackend {
    fn mount<'a>(
        &'a self,
        _location: &'a SourceLocation,
        interaction: MountInteraction,
    ) -> BoxFuture<'a, Result<PathBuf, SourceUnavailable>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.interactions.lock().unwrap().push(interaction);
            if let Some(gate) = &self.gate {
                gate.acquire().await.unwrap().forget();
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("fake backend response")
        })
    }
}

pub(super) fn local_source(root: PathBuf) -> SourceLocation {
    SourceLocation {
        resolved_path: root,
        identity: SourceIdentity::Local {
            library_id: "local-library".into(),
        },
    }
}

pub(super) fn smb_source(root: PathBuf, subpath: &str) -> SourceLocation {
    SourceLocation {
        resolved_path: root,
        identity: SourceIdentity::Smb {
            host: "JUPITER".into(),
            share: "Data".into(),
            subpath: Some(PortablePath::parse(subpath).unwrap()),
        },
    }
}

pub(super) async fn wait_for_calls(backend: &FakeBackend, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while backend.call_count() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("backend call count timed out");
}
