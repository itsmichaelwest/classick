use crate::source_location::{SourceIdentity, SourceLocation};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, Mutex};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountInteraction {
    SuppressUi,
    AllowUi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSource {
    pub root: PathBuf,
    pub remounted: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub enum SourceUnavailable {
    AuthRequired,
    MountFailed(String),
    MissingSubpath(PathBuf),
}

impl fmt::Debug for SourceUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AuthRequired => "AuthRequired",
            Self::MountFailed(_) => "MountFailed(<redacted>)",
            Self::MissingSubpath(_) => "MissingSubpath(<redacted>)",
        })
    }
}

impl fmt::Display for SourceUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthRequired => formatter.write_str("source authentication required"),
            Self::MountFailed(_) => formatter.write_str("source mount failed"),
            Self::MissingSubpath(_) => {
                formatter.write_str("mounted source is missing its configured subpath")
            }
        }
    }
}

impl std::error::Error for SourceUnavailable {}

pub trait SourceMountBackend: Send + Sync + 'static {
    fn mount<'a>(
        &'a self,
        location: &'a SourceLocation,
        interaction: MountInteraction,
    ) -> BoxFuture<'a, Result<PathBuf, SourceUnavailable>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SourceKey {
    Smb {
        host: String,
        share: String,
        subpath: Option<String>,
    },
    Local(String),
}

impl From<&SourceIdentity> for SourceKey {
    fn from(identity: &SourceIdentity) -> Self {
        match identity {
            SourceIdentity::Smb {
                host,
                share,
                subpath,
            } => Self::Smb {
                host: host.to_ascii_lowercase(),
                share: share.to_ascii_lowercase(),
                subpath: subpath.as_ref().map(|path| path.as_str().to_owned()),
            },
            SourceIdentity::Local { library_id } => Self::Local(library_id.clone()),
        }
    }
}

type AvailabilityResult = Result<ResolvedSource, SourceUnavailable>;

struct InFlight {
    id: u64,
    result: watch::Receiver<Option<AvailabilityResult>>,
}

struct ServiceInner {
    backend: Arc<dyn SourceMountBackend>,
    in_flight: Mutex<HashMap<SourceKey, InFlight>>,
    next_id: AtomicU64,
}

#[derive(Clone)]
pub struct SourceAvailabilityService {
    inner: Arc<ServiceInner>,
}

impl SourceAvailabilityService {
    pub fn new(backend: Arc<dyn SourceMountBackend>) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                backend,
                in_flight: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    pub fn platform_default() -> Self {
        #[cfg(target_os = "macos")]
        let backend: Arc<dyn SourceMountBackend> =
            Arc::new(crate::daemon::macos_netfs::MacosNetFsBackend);
        #[cfg(not(target_os = "macos"))]
        let backend: Arc<dyn SourceMountBackend> = Arc::new(EstablishedSessionBackend);
        Self::new(backend)
    }

    pub async fn ensure_source_available(
        &self,
        location: &SourceLocation,
        interaction: MountInteraction,
    ) -> AvailabilityResult {
        if location.resolved_path.exists() {
            return Ok(ResolvedSource {
                root: location.resolved_path.clone(),
                remounted: false,
            });
        }

        if matches!(location.identity, SourceIdentity::Local { .. }) {
            return Err(SourceUnavailable::MissingSubpath(
                location.resolved_path.clone(),
            ));
        }

        let key = SourceKey::from(&location.identity);
        let mut receiver = {
            let mut in_flight = self.inner.in_flight.lock().await;
            if let Some(attempt) = in_flight.get(&key) {
                attempt.result.clone()
            } else {
                let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
                let (sender, receiver) = watch::channel(None);
                in_flight.insert(
                    key.clone(),
                    InFlight {
                        id,
                        result: receiver.clone(),
                    },
                );
                self.spawn_mount(location.clone(), interaction, key, id, sender);
                receiver
            }
        };

        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err(SourceUnavailable::MountFailed(
                    "source mount operation ended unexpectedly".into(),
                ));
            }
        }
    }

    fn spawn_mount(
        &self,
        location: SourceLocation,
        interaction: MountInteraction,
        key: SourceKey,
        id: u64,
        sender: watch::Sender<Option<AvailabilityResult>>,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let result = inner
                .backend
                .mount(&location, interaction)
                .await
                .and_then(|mountpoint| resolve_mounted_source(&location, mountpoint))
                .map_err(redact_backend_error);
            let mut in_flight = inner.in_flight.lock().await;
            if in_flight.get(&key).is_some_and(|attempt| attempt.id == id) {
                in_flight.remove(&key);
            }
            drop(in_flight);
            let _ = sender.send(Some(result));
        });
    }
}

#[cfg(not(target_os = "macos"))]
struct EstablishedSessionBackend;

#[cfg(not(target_os = "macos"))]
impl SourceMountBackend for EstablishedSessionBackend {
    fn mount<'a>(
        &'a self,
        _location: &'a SourceLocation,
        _interaction: MountInteraction,
    ) -> BoxFuture<'a, Result<PathBuf, SourceUnavailable>> {
        Box::pin(async {
            Err(SourceUnavailable::MountFailed(
                "source path is unavailable through the current OS session".into(),
            ))
        })
    }
}

fn resolve_mounted_source(location: &SourceLocation, mountpoint: PathBuf) -> AvailabilityResult {
    let root = match &location.identity {
        SourceIdentity::Smb {
            subpath: Some(subpath),
            ..
        } => subpath.resolve(&mountpoint),
        SourceIdentity::Smb { subpath: None, .. } => mountpoint,
        SourceIdentity::Local { .. } => location.resolved_path.clone(),
    };

    if !root.exists() {
        return Err(SourceUnavailable::MissingSubpath(root));
    }

    Ok(ResolvedSource {
        root,
        remounted: true,
    })
}

fn redact_backend_error(error: SourceUnavailable) -> SourceUnavailable {
    match error {
        SourceUnavailable::MountFailed(_) => {
            SourceUnavailable::MountFailed("source mount failed".into())
        }
        other => other,
    }
}

#[cfg(test)]
#[path = "source_availability_test_support.rs"]
mod test_support;

#[cfg(test)]
mod tests {
    use super::test_support::{local_source, smb_source, wait_for_calls, FakeBackend, TestDir};
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[tokio::test]
    async fn existing_local_source_is_an_immediate_no_op() {
        let root = TestDir::new("local");
        let backend = FakeBackend::responding_with(vec![]);
        let service = SourceAvailabilityService::new(Arc::new(backend.clone()));

        let resolved = service
            .ensure_source_available(
                &local_source(root.path().to_owned()),
                MountInteraction::SuppressUi,
            )
            .await
            .unwrap();

        assert_eq!(resolved.root, root.path());
        assert!(!resolved.remounted);
        assert_eq!(backend.call_count(), 0);
    }

    #[tokio::test]
    async fn concurrent_requests_for_the_same_logical_source_share_one_mount() {
        let mounted = TestDir::new("same-source-mounted");
        std::fs::create_dir_all(mounted.path().join("media/music")).unwrap();
        let backend = FakeBackend::gated(Ok(mounted.path().to_owned()));
        let service = Arc::new(SourceAvailabilityService::new(Arc::new(backend.clone())));
        let a = smb_source(PathBuf::from("/missing/a"), "media/music");
        let b = smb_source(PathBuf::from("/different/stale/path"), "media/music");

        let first = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .ensure_source_available(&a, MountInteraction::SuppressUi)
                    .await
            }
        });
        let second = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .ensure_source_available(&b, MountInteraction::SuppressUi)
                    .await
            }
        });
        wait_for_calls(&backend, 1).await;
        backend.gate.as_ref().unwrap().add_permits(1);

        assert_eq!(
            first.await.unwrap().unwrap(),
            second.await.unwrap().unwrap()
        );
        assert_eq!(backend.call_count(), 1);
    }

    #[tokio::test]
    async fn distinct_logical_sources_mount_independently() {
        let mounted = TestDir::new("distinct-mounted");
        std::fs::create_dir_all(mounted.path().join("media/music")).unwrap();
        std::fs::create_dir_all(mounted.path().join("media/videos")).unwrap();
        let backend = FakeBackend::gated(Ok(mounted.path().to_owned()));
        backend
            .responses
            .lock()
            .unwrap()
            .push_back(Ok(mounted.path().to_owned()));
        let service = Arc::new(SourceAvailabilityService::new(Arc::new(backend.clone())));

        let music = tokio::spawn({
            let service = service.clone();
            let source = smb_source(PathBuf::from("/missing/music"), "media/music");
            async move {
                service
                    .ensure_source_available(&source, MountInteraction::SuppressUi)
                    .await
            }
        });
        let videos = tokio::spawn({
            let service = service.clone();
            let source = smb_source(PathBuf::from("/missing/videos"), "media/videos");
            async move {
                service
                    .ensure_source_available(&source, MountInteraction::SuppressUi)
                    .await
            }
        });
        wait_for_calls(&backend, 2).await;
        backend.gate.as_ref().unwrap().add_permits(2);

        assert!(music.await.unwrap().is_ok());
        assert!(videos.await.unwrap().is_ok());
        assert_eq!(backend.call_count(), 2);
    }

    #[tokio::test]
    async fn returned_mountpoint_is_authoritative_before_appending_subpath() {
        let volumes = TestDir::new("alternate-volume");
        let returned_mount = volumes.path().join("Volumes/data-1");
        let expected_root = returned_mount.join("media/music");
        std::fs::create_dir_all(&expected_root).unwrap();
        let backend = FakeBackend::responding_with(vec![Ok(returned_mount)]);
        let service = SourceAvailabilityService::new(Arc::new(backend));

        let resolved = service
            .ensure_source_available(
                &smb_source(volumes.path().join("stale/data/media/music"), "media/music"),
                MountInteraction::SuppressUi,
            )
            .await
            .unwrap();

        assert_eq!(resolved.root, expected_root);
        assert!(resolved.remounted);
    }

    #[tokio::test]
    async fn auth_required_can_be_retried_explicitly_with_ui() {
        let mounted = TestDir::new("auth-retry");
        std::fs::create_dir_all(mounted.path().join("media/music")).unwrap();
        let backend = FakeBackend::responding_with(vec![
            Err(SourceUnavailable::AuthRequired),
            Ok(mounted.path().to_owned()),
        ]);
        let service = SourceAvailabilityService::new(Arc::new(backend.clone()));
        let source = smb_source(PathBuf::from("/missing/auth"), "media/music");

        assert_eq!(
            service
                .ensure_source_available(&source, MountInteraction::SuppressUi)
                .await,
            Err(SourceUnavailable::AuthRequired)
        );
        assert!(service
            .ensure_source_available(&source, MountInteraction::AllowUi)
            .await
            .is_ok());
        assert_eq!(
            *backend.interactions.lock().unwrap(),
            vec![MountInteraction::SuppressUi, MountInteraction::AllowUi]
        );
    }

    #[tokio::test]
    async fn missing_portable_subpath_is_typed() {
        let mounted = TestDir::new("missing-subpath");
        let backend = FakeBackend::responding_with(vec![Ok(mounted.path().to_owned())]);
        let service = SourceAvailabilityService::new(Arc::new(backend));
        let expected = mounted.path().join("media/music");

        assert_eq!(
            service
                .ensure_source_available(
                    &smb_source(PathBuf::from("/missing/subpath"), "media/music"),
                    MountInteraction::SuppressUi,
                )
                .await,
            Err(SourceUnavailable::MissingSubpath(expected))
        );
    }

    #[tokio::test]
    async fn backend_diagnostics_are_redacted_before_exposure() {
        let backend = FakeBackend::responding_with(vec![Err(SourceUnavailable::MountFailed(
            "smb://alice:hunter2@jupiter/data?password=hunter2".into(),
        ))]);
        let service = SourceAvailabilityService::new(Arc::new(backend));
        let source = smb_source(PathBuf::from("/missing/secret"), "media/music");

        let diagnostic = service
            .ensure_source_available(&source, MountInteraction::SuppressUi)
            .await
            .unwrap_err()
            .to_string();

        assert!(!diagnostic.contains("alice"));
        assert!(!diagnostic.contains("hunter2"));
        assert!(!diagnostic.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn typed_errors_do_not_debug_log_credential_material() {
        let errors = [
            SourceUnavailable::MountFailed(
                "smb://alice:hunter2@jupiter/data?password=hunter2".into(),
            ),
            SourceUnavailable::MissingSubpath(PathBuf::from(
                "/Volumes/alice-password-hunter2/music",
            )),
        ];

        for error in errors {
            for diagnostic in [error.to_string(), format!("{error:?}")] {
                assert!(!diagnostic.contains("alice"));
                assert!(!diagnostic.contains("hunter2"));
                assert!(!diagnostic.to_ascii_lowercase().contains("password"));
            }
        }
    }
}
