use crate::atomic_file::AtomicFileWriter;
use crate::device_state;
use crate::manifest::{self, Manifest};
use crate::portable_path::PortablePath;
use crate::source_location::{SourceIdentity, SourceLocation};
use anyhow::{Context, Result};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestOrigin {
    DeviceV2,
    HostV2,
    HostV1,
    LegacyV1,
    Missing,
}

#[derive(Debug)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub origin: ManifestOrigin,
    pub needs_device_publish: bool,
    pub source_identity: Option<SourceIdentity>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestStoreError {
    InvalidDevice { path: PathBuf, reason: String },
}

impl fmt::Display for ManifestStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDevice { path, reason } => write!(
                f,
                "connected device manifest at {} is invalid: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManifestStoreError {}

#[derive(Debug)]
pub struct ManifestPublishOutcome {
    pub device_validated: bool,
    pub host_cache_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManifestStore {
    mount: PathBuf,
    serial: String,
    host_cache: PathBuf,
    legacy_flat: PathBuf,
    atomic_writer: AtomicFileWriter,
}

impl ManifestStore {
    pub fn new(
        mount: PathBuf,
        serial: String,
        host_cache: PathBuf,
        legacy_flat: PathBuf,
        atomic_writer: AtomicFileWriter,
    ) -> Self {
        Self {
            mount,
            serial,
            host_cache,
            legacy_flat,
            atomic_writer,
        }
    }

    pub fn load(&self, source: &SourceLocation) -> Result<LoadedManifest> {
        let device_path = device_state::portable_manifest_path(&self.mount);
        if device_path.exists() {
            return self.load_device(&device_path, source);
        }
        if self.host_cache.exists() {
            return self.load_host(&self.host_cache, ManifestOrigin::HostV1, source);
        }
        if self.legacy_flat.exists() {
            return self.load_host(&self.legacy_flat, ManifestOrigin::LegacyV1, source);
        }
        Ok(self.missing())
    }

    /// Load connected display state without adopting an unscoped legacy
    /// manifest. Sync migration uses [`Self::load`]; serial-targeted daemon
    /// reads must not let a second device inherit the first device's flat v1
    /// facts.
    pub fn load_device_or_host_cache(&self, source: &SourceLocation) -> Result<LoadedManifest> {
        let device_path = device_state::portable_manifest_path(&self.mount);
        if device_path.exists() {
            return self.load_device(&device_path, source);
        }
        self.load_host_cache(source)
    }

    /// Load display-only state when the device is disconnected. This never
    /// consults the last known mount path: a volume can remain mounted after
    /// device removal or be reused by a different attachment, while the host
    /// cache is explicitly serial-keyed.
    pub fn load_host_cache(&self, source: &SourceLocation) -> Result<LoadedManifest> {
        if self.host_cache.exists() {
            return self.load_host(&self.host_cache, ManifestOrigin::HostV1, source);
        }
        Ok(self.missing())
    }

    fn missing(&self) -> LoadedManifest {
        let mut manifest = Manifest::empty();
        manifest.version = 2;
        manifest.ipod_serial = Some(self.serial.clone());
        LoadedManifest {
            manifest,
            origin: ManifestOrigin::Missing,
            needs_device_publish: true,
            source_identity: None,
        }
    }

    pub fn publish(
        &self,
        manifest: &Manifest,
        source: &SourceLocation,
    ) -> Result<ManifestPublishOutcome> {
        let bytes = manifest.encode_v2(source, &self.serial)?;
        let device_path = device_state::portable_manifest_path(&self.mount);
        self.atomic_writer
            .write(&device_path, &bytes)
            .context("publish authoritative device manifest")?;
        self.validate_v2_file(&device_path, source)
            .context("validate authoritative device manifest")?;

        let host_cache_warning = self
            .refresh_host_cache(&bytes, source)
            .err()
            .map(|error| format!("{error:#}"));
        Ok(ManifestPublishOutcome {
            device_validated: true,
            host_cache_warning,
        })
    }

    pub fn publish_runtime(&self, manifest: &Manifest) -> Result<ManifestPublishOutcome> {
        let root = manifest
            .last_source_root
            .clone()
            .context("candidate manifest has no resolved source root")?;
        let source = SourceLocation::discover(root).context("resolve candidate source identity")?;
        self.publish(manifest, &source)
    }

    pub fn reconcile_from_live_db(
        &self,
        source: &SourceLocation,
        rebuild: impl FnOnce() -> Result<Manifest>,
    ) -> Result<LoadedManifest> {
        let mut rebuilt = rebuild().context("rebuild manifest from live iTunesDB")?;
        rebuilt.version = 2;
        rebuilt.ipod_serial = Some(self.serial.clone());
        self.publish(&rebuilt, source)?;
        self.load(source)
    }

    fn load_device(&self, path: &Path, source: &SourceLocation) -> Result<LoadedManifest> {
        let bytes = std::fs::read(path).map_err(|error| ManifestStoreError::InvalidDevice {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        let decoded =
            manifest::decode_v2_document(&bytes, &source.resolved_path).map_err(|error| {
                ManifestStoreError::InvalidDevice {
                    path: path.to_path_buf(),
                    reason: format!("{error:#}"),
                }
            })?;
        self.require_serial(&decoded.manifest).map_err(|reason| {
            ManifestStoreError::InvalidDevice {
                path: path.to_path_buf(),
                reason,
            }
        })?;
        Ok(LoadedManifest {
            manifest: decoded.manifest,
            origin: ManifestOrigin::DeviceV2,
            needs_device_publish: false,
            source_identity: decoded.source_identity,
        })
    }

    fn load_host(
        &self,
        path: &Path,
        v1_origin: ManifestOrigin,
        source: &SourceLocation,
    ) -> Result<LoadedManifest> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read host manifest {}", path.display()))?;
        let version = manifest_version(&bytes)
            .with_context(|| format!("inspect host manifest {}", path.display()))?;
        if version == 2 {
            let decoded = manifest::decode_v2_document(&bytes, &source.resolved_path)
                .with_context(|| format!("decode host manifest v2 {}", path.display()))?;
            self.require_serial(&decoded.manifest)
                .map_err(anyhow::Error::msg)?;
            return Ok(LoadedManifest {
                manifest: decoded.manifest,
                origin: ManifestOrigin::HostV2,
                needs_device_publish: true,
                source_identity: decoded.source_identity,
            });
        }
        if version != 1 {
            anyhow::bail!("unsupported host manifest version {version}");
        }
        let legacy: Manifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode host manifest v1 {}", path.display()))?;
        self.require_serial_if_present(&legacy)
            .map_err(anyhow::Error::msg)?;
        Ok(LoadedManifest {
            manifest: migrate_v1(legacy, source, &self.serial),
            origin: v1_origin,
            needs_device_publish: true,
            source_identity: None,
        })
    }

    fn refresh_host_cache(&self, bytes: &[u8], source: &SourceLocation) -> Result<()> {
        if self.host_cache.exists() {
            let existing = std::fs::read(&self.host_cache).with_context(|| {
                format!("read existing host cache {}", self.host_cache.display())
            })?;
            if manifest_version(&existing).ok() == Some(1) {
                let retained = device_state::retained_v1_manifest_path(&self.host_cache);
                if !retained.exists() {
                    self.atomic_writer
                        .write(&retained, &existing)
                        .context("retain per-device manifest v1 migration input")?;
                    let retained_manifest: Manifest = serde_json::from_slice(
                        &std::fs::read(&retained).context("read retained manifest v1")?,
                    )
                    .context("validate retained manifest v1")?;
                    if retained_manifest.version != 1 {
                        anyhow::bail!("retained manifest is not v1");
                    }
                }
            }
        }
        self.atomic_writer
            .write(&self.host_cache, bytes)
            .context("refresh host manifest cache")?;
        self.validate_v2_file(&self.host_cache, source)
            .context("validate host manifest cache")
    }

    fn validate_v2_file(&self, path: &Path, source: &SourceLocation) -> Result<()> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read published manifest {}", path.display()))?;
        let decoded = manifest::decode_v2_document(&bytes, &source.resolved_path)
            .with_context(|| format!("decode published manifest {}", path.display()))?;
        self.require_serial(&decoded.manifest)
            .map_err(anyhow::Error::msg)
    }

    fn require_serial(&self, manifest: &Manifest) -> std::result::Result<(), String> {
        let actual = manifest
            .ipod_serial
            .as_deref()
            .ok_or_else(|| "manifest has no iPod serial".to_string())?;
        if device_state::sanitize_serial(actual) != device_state::sanitize_serial(&self.serial) {
            return Err(format!(
                "manifest serial {actual:?} does not match connected device {:?}",
                self.serial
            ));
        }
        Ok(())
    }

    fn require_serial_if_present(&self, manifest: &Manifest) -> std::result::Result<(), String> {
        match manifest.ipod_serial.as_deref() {
            Some(_) => self.require_serial(manifest),
            None => Ok(()),
        }
    }
}

fn manifest_version(bytes: &[u8]) -> Result<u32> {
    #[derive(serde::Deserialize)]
    struct Header {
        version: u32,
    }
    Ok(serde_json::from_slice::<Header>(bytes)
        .context("decode manifest version")?
        .version)
}

fn migrate_v1(mut manifest: Manifest, source: &SourceLocation, serial: &str) -> Manifest {
    let recorded_root = manifest.last_source_root.clone();
    for entry in &mut manifest.tracks {
        if !entry.source_known {
            entry.source_path = PathBuf::new();
            continue;
        }
        let relative = recorded_root
            .as_deref()
            .and_then(|root| PortablePath::from_absolute(root, &entry.source_path).ok());
        match relative {
            Some(relative) => entry.source_path = relative.resolve(&source.resolved_path),
            None => {
                entry.source_known = false;
                entry.source_path = PathBuf::new();
            }
        }
    }
    manifest.version = 2;
    manifest.ipod_serial = Some(serial.to_string());
    manifest.last_source_root = Some(source.resolved_path.clone());
    manifest
}

#[cfg(test)]
mod tests {
    use super::{ManifestOrigin, ManifestStore, ManifestStoreError};
    use crate::atomic_file::AtomicFileWriter;
    use crate::manifest::{Manifest, ManifestEntry};
    use crate::portable_path::PortablePath;
    use crate::source_location::{SourceIdentity, SourceLocation};
    use std::path::{Path, PathBuf};

    const V1: &[u8] = include_bytes!("../tests/fixtures/manifest-v1-windows.json");
    const V2: &[u8] = include_bytes!("../tests/fixtures/manifest-v2-portable.json");

    struct Sandbox {
        mount: PathBuf,
        host: PathBuf,
        legacy: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/test-tmp")
                .join(format!(
                    "manifest-store-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self {
                mount: root.join("iPod"),
                host: root.join("devices/SERIAL-1/manifest.json"),
                legacy: root.join("manifest.json"),
            }
        }

        fn store(&self) -> ManifestStore {
            ManifestStore::new(
                self.mount.clone(),
                "SERIAL-1".into(),
                self.host.clone(),
                self.legacy.clone(),
                AtomicFileWriter::new(),
            )
        }

        fn device_manifest(&self) -> PathBuf {
            self.mount.join("iPod_Control/classick/manifest.json")
        }

        fn write(path: &Path, bytes: &[u8]) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    fn source(root: impl Into<PathBuf>) -> SourceLocation {
        SourceLocation {
            resolved_path: root.into(),
            identity: SourceIdentity::Smb {
                host: "jupiter".into(),
                share: "data".into(),
                subpath: Some(PortablePath::parse("media/music").unwrap()),
            },
        }
    }

    fn rebuilt_manifest() -> Manifest {
        Manifest {
            version: 1,
            ipod_serial: Some("SERIAL-1".into()),
            last_source_root: None,
            tracks: vec![ManifestEntry {
                source_path: PathBuf::new(),
                source_mtime: 0,
                source_size: 0,
                source_fingerprint: String::new(),
                ipod_dbid: 999,
                ipod_relpath: "iPod_Control/Music/F00/ZZZZ.m4a".into(),
                source_known: false,
                audio_fingerprint: String::new(),
                encoder: "unknown".into(),
                encoder_version: String::new(),
                source_format: "flac".into(),
                transcode_profile: None,
            }],
        }
    }

    fn v2_with_count(source: &SourceLocation, count: usize) -> Vec<u8> {
        let mut manifest = rebuilt_manifest();
        manifest.tracks = (0..count)
            .map(|index| ManifestEntry {
                source_path: source.resolved_path.join(format!("{index}.flac")),
                source_mtime: 0,
                source_size: 1,
                source_fingerprint: format!("fp-{index}"),
                ipod_dbid: index as u64 + 1,
                ipod_relpath: format!("iPod_Control/Music/F00/{index}.m4a"),
                source_known: true,
                audio_fingerprint: String::new(),
                encoder: "unknown".into(),
                encoder_version: String::new(),
                source_format: "flac".into(),
                transcode_profile: None,
            })
            .collect();
        manifest.encode_v2(source, "SERIAL-1").unwrap()
    }

    #[test]
    fn valid_device_v2_wins_over_stale_host_cache() {
        let s = Sandbox::new();
        Sandbox::write(&s.device_manifest(), V2);
        Sandbox::write(&s.host, br#"{"version":1,"tracks":[]}"#);

        let loaded = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::DeviceV2);
        assert_eq!(loaded.manifest.tracks.len(), 1);
        assert!(!loaded.needs_device_publish);
    }

    #[test]
    fn disconnected_load_uses_host_cache_even_if_the_last_mount_still_exists() {
        let sandbox = Sandbox::new();
        let store = sandbox.store();
        let source = source("/Volumes/data/media/music");
        Sandbox::write(&sandbox.device_manifest(), &v2_with_count(&source, 7));
        Sandbox::write(&sandbox.host, &v2_with_count(&source, 3));

        let loaded = store.load_host_cache(&source).unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::HostV2);
        assert_eq!(loaded.manifest.tracks.len(), 3);
    }

    #[test]
    fn missing_device_uses_host_v2_then_requests_device_publish() {
        let s = Sandbox::new();
        Sandbox::write(&s.host, V2);

        let loaded = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::HostV2);
        assert!(loaded.needs_device_publish);
    }

    #[test]
    fn host_v1_is_relativized_and_outside_root_becomes_source_unknown() {
        let s = Sandbox::new();
        Sandbox::write(&s.host, V1);

        let loaded = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::HostV1);
        assert_eq!(
            loaded.manifest.tracks[0].source_path,
            PathBuf::from("/Volumes/data/media/music/Birdy/Beautiful Lies/01 - Growing Pains.flac")
        );
        assert!(loaded.manifest.tracks[0].source_known);
        assert!(!loaded.manifest.tracks[1].source_known);
        assert!(loaded.manifest.tracks[1].source_path.as_os_str().is_empty());
        assert_eq!(std::fs::read(&s.host).unwrap(), V1);
    }

    #[test]
    fn legacy_v1_is_used_only_after_host_cache_and_is_retained() {
        let s = Sandbox::new();
        Sandbox::write(&s.legacy, V1);

        let loaded = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::LegacyV1);
        assert!(loaded.needs_device_publish);
        assert_eq!(std::fs::read(&s.legacy).unwrap(), V1);
    }

    #[test]
    fn absent_authorities_request_a_live_database_rebuild() {
        let s = Sandbox::new();

        let loaded = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap();

        assert_eq!(loaded.origin, ManifestOrigin::Missing);
        assert!(loaded.manifest.tracks.is_empty());
        assert!(loaded.needs_device_publish);
    }

    #[test]
    fn invalid_device_fails_closed_without_consuming_valid_host_cache() {
        let s = Sandbox::new();
        Sandbox::write(&s.device_manifest(), b"not json");
        Sandbox::write(&s.host, V2);

        let error = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<ManifestStoreError>(),
            Some(ManifestStoreError::InvalidDevice { .. })
        ));
    }

    #[test]
    fn device_serial_mismatch_is_invalid_device_authority() {
        let s = Sandbox::new();
        let wrong = String::from_utf8(V2.to_vec())
            .unwrap()
            .replace("SERIAL-1", "SERIAL-2");
        Sandbox::write(&s.device_manifest(), wrong.as_bytes());

        let error = s
            .store()
            .load(&source("/Volumes/data/media/music"))
            .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<ManifestStoreError>(),
            Some(ManifestStoreError::InvalidDevice { .. })
        ));
    }

    #[test]
    fn reconciliation_explicitly_rebuilds_from_live_db_before_replacing_invalid_device() {
        let s = Sandbox::new();
        Sandbox::write(&s.device_manifest(), b"not json");
        let mut rebuilt = false;

        let loaded = s
            .store()
            .reconcile_from_live_db(&source("/Volumes/data/media/music"), || {
                rebuilt = true;
                Ok(rebuilt_manifest())
            })
            .unwrap();

        assert!(rebuilt);
        assert_eq!(loaded.origin, ManifestOrigin::DeviceV2);
        assert_eq!(loaded.manifest.tracks[0].ipod_dbid, 999);
    }

    #[test]
    fn publish_validates_device_before_refreshing_host_cache() {
        let s = Sandbox::new();
        let source = source("/Volumes/data/media/music");
        let manifest = Manifest::decode_v2(V2, &source.resolved_path).unwrap();

        let outcome = s.store().publish(&manifest, &source).unwrap();

        assert!(outcome.device_validated);
        assert!(outcome.host_cache_warning.is_none());
        assert_eq!(
            s.store().load(&source).unwrap().origin,
            ManifestOrigin::DeviceV2
        );
    }

    #[test]
    fn device_publication_failure_does_not_advance_host_cache() {
        let s = Sandbox::new();
        std::fs::create_dir_all(s.mount.join("iPod_Control")).unwrap();
        std::fs::write(s.mount.join("iPod_Control/classick"), b"not a directory").unwrap();
        let source = source("/Volumes/data/media/music");
        let manifest = Manifest::decode_v2(V2, &source.resolved_path).unwrap();

        assert!(s.store().publish(&manifest, &source).is_err());
        assert!(!s.host.exists());
    }

    #[test]
    fn host_cache_failure_is_warning_only_after_valid_device_publication() {
        let s = Sandbox::new();
        std::fs::create_dir_all(s.host.parent().unwrap().parent().unwrap()).unwrap();
        std::fs::write(s.host.parent().unwrap(), b"not a directory").unwrap();
        let source = source("/Volumes/data/media/music");
        let manifest = Manifest::decode_v2(V2, &source.resolved_path).unwrap();

        let outcome = s.store().publish(&manifest, &source).unwrap();

        assert!(outcome.device_validated);
        assert!(outcome.host_cache_warning.is_some());
        assert!(s.device_manifest().exists());
    }
}
