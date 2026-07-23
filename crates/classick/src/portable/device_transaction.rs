use super::profile::PortableProfile;
use crate::atomic_file::AtomicFileWriter;
use crate::device::DeviceId;
use crate::device_coordination::DeviceMutationSession;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const TRANSACTION_SCHEMA_VERSION: u32 = 2;
const TRANSACTION_DIRECTORY: &str = "iPod_Control/classick/pending/portable-config";
const JOURNAL_FILE: &str = "journal.json";
const PROFILE_BACKUP: &str = "profile.original";
const SYSINFO_BACKUP: &str = "sysinfo-extended.original";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    device_id: DeviceId,
    generation_before: crate::device_coordination::DeviceGeneration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_generation: Option<crate::device_coordination::DeviceGeneration>,
    profile_existed: bool,
    sysinfo_existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_backup_blake3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sysinfo_backup_blake3: Option<String>,
}

pub(super) fn recover(session: &DeviceMutationSession) -> Result<bool> {
    let root = transaction_root(session.mount());
    let journal_path = root.join(JOURNAL_FILE);
    let bytes = match std::fs::read(&journal_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read portable transaction {}", journal_path.display()));
        }
    };
    let journal: Journal =
        serde_json::from_slice(&bytes).context("parse pending portable transaction")?;
    if journal.schema_version != TRANSACTION_SCHEMA_VERSION
        || journal.device_id != *session.device_id()
    {
        anyhow::bail!("pending portable transaction does not match this device");
    }

    let live = session.capture_current_generation()?;
    if live == journal.generation_before {
        remove_transaction(&root)?;
        session.adopt_verified_generation(live)?;
        return Ok(true);
    }
    if journal.published_generation.as_ref() != Some(&live) {
        anyhow::bail!(
            "external_generation_changed: pending portable transaction does not own the live device generation"
        );
    }

    let profile_original = validated_backup(
        &root.join(PROFILE_BACKUP),
        journal.profile_existed,
        journal.profile_backup_blake3.as_deref(),
    )?;
    let sysinfo_original = validated_backup(
        &root.join(SYSINFO_BACKUP),
        journal.sysinfo_existed,
        journal.sysinfo_backup_blake3.as_deref(),
    )?;
    restore(&profile_path(session.mount()), profile_original.as_deref())?;
    restore(&sysinfo_path(session.mount()), sysinfo_original.as_deref())?;
    let restored = session.capture_current_generation()?;
    if restored != journal.generation_before {
        anyhow::bail!(
            "external_generation_changed: portable transaction rollback did not restore its recorded predecessor"
        );
    }
    remove_transaction(&root)?;
    session.adopt_verified_generation(restored)?;
    Ok(true)
}

pub(super) fn publish(
    session: &DeviceMutationSession,
    profile: &PortableProfile,
    sysinfo: Option<&[u8]>,
) -> Result<()> {
    profile.validate()?;
    if profile.device_id != *session.device_id() {
        anyhow::bail!("portable transaction profile belongs to another device");
    }
    recover(session)?;
    session.verify_expected_generation()?;
    let generation_before = session.capture_current_generation()?;

    let root = transaction_root(session.mount());
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create portable transaction {}", root.display()))?;
    let profile_path = profile_path(session.mount());
    let sysinfo_path = sysinfo_path(session.mount());
    let profile_original = read_optional(&profile_path)?;
    let sysinfo_original = read_optional(&sysinfo_path)?;
    write_backup(&root.join(PROFILE_BACKUP), profile_original.as_deref())?;
    write_backup(&root.join(SYSINFO_BACKUP), sysinfo_original.as_deref())?;
    let mut journal = Journal {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        device_id: session.device_id().clone(),
        generation_before,
        published_generation: None,
        profile_existed: profile_original.is_some(),
        sysinfo_existed: sysinfo_original.is_some(),
        profile_backup_blake3: profile_original.as_deref().map(content_hash),
        sysinfo_backup_blake3: sysinfo_original.as_deref().map(content_hash),
    };
    save_journal(&root, &journal)?;

    let result = (|| -> Result<()> {
        AtomicFileWriter::new().write(&profile_path, profile.to_json_pretty()?.as_bytes())?;
        record_published_generation(session, &root, &mut journal)?;
        if let Some(bytes) = sysinfo {
            AtomicFileWriter::new().write(&sysinfo_path, bytes)?;
            record_published_generation(session, &root, &mut journal)?;
        }
        let published = super::device_store::read_profile(session.mount())?;
        if published != super::device_store::OwnedDeviceProfile::Valid(profile.clone()) {
            anyhow::bail!("portable profile readback differs");
        }
        if let Some(expected) = sysinfo {
            if std::fs::read(&sysinfo_path)? != expected {
                anyhow::bail!("generated SysInfoExtended readback differs");
            }
        }
        Ok(())
    })();

    if let Err(error) = result {
        let live = session.capture_current_generation()?;
        if live != journal.generation_before && journal.published_generation.as_ref() != Some(&live)
        {
            return Err(error).context(
                "publish portable device transaction; external_generation_changed: refusing rollback over an unknown live generation",
            );
        }
        restore(&profile_path, profile_original.as_deref()).context("rollback portable profile")?;
        restore(&sysinfo_path, sysinfo_original.as_deref()).context("rollback SysInfoExtended")?;
        let restored = session.capture_current_generation()?;
        if restored != journal.generation_before {
            return Err(error).context(
                "publish portable device transaction; external_generation_changed: rollback did not restore the recorded predecessor",
            );
        }
        remove_transaction(&root)?;
        session.adopt_verified_generation(restored)?;
        return Err(error).context("publish portable device transaction");
    }

    remove_transaction(&root)?;
    session.accept_verified_generation()?;
    Ok(())
}

fn save_journal(root: &Path, journal: &Journal) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(journal)?;
    bytes.push(b'\n');
    AtomicFileWriter::new().write(&root.join(JOURNAL_FILE), &bytes)
}

fn record_published_generation(
    session: &DeviceMutationSession,
    root: &Path,
    journal: &mut Journal,
) -> Result<()> {
    journal.published_generation = Some(session.capture_current_generation()?);
    save_journal(root, journal)
}

fn write_backup(path: &Path, original: Option<&[u8]>) -> Result<()> {
    AtomicFileWriter::new().write(path, original.unwrap_or_default())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read authority {}", path.display())),
    }
}

fn validated_backup(
    backup: &Path,
    existed: bool,
    expected_blake3: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    if existed {
        let metadata = std::fs::symlink_metadata(backup)
            .with_context(|| format!("inspect portable rollback {}", backup.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            anyhow::bail!(
                "portable rollback backup is not a regular file: {}",
                backup.display()
            );
        }
        let bytes = std::fs::read(backup)
            .with_context(|| format!("read portable rollback {}", backup.display()))?;
        let expected_blake3 =
            expected_blake3.context("portable rollback backup has no recorded content hash")?;
        if content_hash(&bytes) != expected_blake3 {
            anyhow::bail!(
                "portable rollback backup content differs: {}",
                backup.display()
            );
        }
        Ok(Some(bytes))
    } else {
        Ok(None)
    }
}

fn restore(target: &Path, original: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = original {
        AtomicFileWriter::new().write(target, bytes)
    } else {
        match std::fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error)
                .with_context(|| format!("remove newly-created authority {}", target.display())),
        }
    }
}

fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn remove_transaction(root: &Path) -> Result<()> {
    match std::fs::remove_dir_all(root) {
        Ok(()) => {
            if let Some(parent) = root.parent() {
                match std::fs::remove_dir(parent) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                        ) => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "remove empty portable pending directory {}",
                                parent.display()
                            )
                        });
                    }
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("remove portable transaction {}", root.display()))
        }
    }
}

fn transaction_root(mount: &Path) -> PathBuf {
    mount.join(TRANSACTION_DIRECTORY)
}

fn profile_path(mount: &Path) -> PathBuf {
    mount.join("iPod_Control/classick/profile.json")
}

fn sysinfo_path(mount: &Path) -> PathBuf {
    mount.join("iPod_Control/Device/SysInfoExtended")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn mount() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mount = std::env::temp_dir().join(format!(
            "classick-portable-transaction-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&mount);
        std::fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
        std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
        std::fs::write(mount.join("iPod_Control/iTunes/iTunesDB"), b"db").unwrap();
        mount
    }

    #[test]
    fn recovery_restores_exact_originals_after_a_partial_publication() {
        let mount = mount();
        let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
        std::fs::write(sysinfo_path(&mount), b"foreign-sysinfo").unwrap();
        let session = DeviceMutationSession::acquire(&mount, device_id.clone()).unwrap();
        let generation_before = session.capture_current_generation().unwrap();
        let root = transaction_root(&mount);
        std::fs::create_dir_all(&root).unwrap();
        write_backup(&root.join(PROFILE_BACKUP), None).unwrap();
        write_backup(&root.join(SYSINFO_BACKUP), Some(b"foreign-sysinfo")).unwrap();
        let mut journal = Journal {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            device_id,
            generation_before,
            published_generation: None,
            profile_existed: false,
            sysinfo_existed: true,
            profile_backup_blake3: None,
            sysinfo_backup_blake3: Some(content_hash(b"foreign-sysinfo")),
        };
        AtomicFileWriter::new()
            .write(
                &root.join(JOURNAL_FILE),
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
        std::fs::write(profile_path(&mount), b"partial-profile").unwrap();
        std::fs::write(sysinfo_path(&mount), b"partial-sysinfo").unwrap();
        journal.published_generation = Some(session.capture_current_generation().unwrap());
        save_journal(&root, &journal).unwrap();

        assert!(recover(&session).unwrap());

        assert!(!profile_path(&mount).exists());
        assert_eq!(
            std::fs::read(sysinfo_path(&mount)).unwrap(),
            b"foreign-sysinfo"
        );
        assert!(!root.exists());
        session.verify_expected_generation().unwrap();
    }

    #[test]
    fn recovery_preserves_an_unknown_external_generation() {
        let mount = mount();
        let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
        let initial_session = DeviceMutationSession::acquire(&mount, device_id.clone()).unwrap();
        let generation_before = initial_session.capture_current_generation().unwrap();
        drop(initial_session);
        let root = transaction_root(&mount);
        std::fs::create_dir_all(&root).unwrap();
        write_backup(&root.join(PROFILE_BACKUP), None).unwrap();
        write_backup(&root.join(SYSINFO_BACKUP), Some(b"foreign-sysinfo")).unwrap();
        let journal = Journal {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            device_id: device_id.clone(),
            generation_before,
            published_generation: None,
            profile_existed: false,
            sysinfo_existed: true,
            profile_backup_blake3: None,
            sysinfo_backup_blake3: Some(content_hash(b"foreign-sysinfo")),
        };
        AtomicFileWriter::new()
            .write(
                &root.join(JOURNAL_FILE),
                &serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
        std::fs::write(profile_path(&mount), b"external-profile").unwrap();
        std::fs::write(sysinfo_path(&mount), b"external-sysinfo").unwrap();
        let session = DeviceMutationSession::acquire(&mount, device_id).unwrap();

        let error = recover(&session).unwrap_err();

        assert!(format!("{error:#}").contains("external_generation_changed"));
        assert_eq!(
            std::fs::read(profile_path(&mount)).unwrap(),
            b"external-profile"
        );
        assert_eq!(
            std::fs::read(sysinfo_path(&mount)).unwrap(),
            b"external-sysinfo"
        );
        assert!(root.exists());
    }

    #[test]
    fn recovery_refuses_a_tampered_backup_before_overwriting_live_authorities() {
        let mount = mount();
        let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
        std::fs::write(sysinfo_path(&mount), b"foreign-sysinfo").unwrap();
        let session = DeviceMutationSession::acquire(&mount, device_id.clone()).unwrap();
        let generation_before = session.capture_current_generation().unwrap();
        let root = transaction_root(&mount);
        std::fs::create_dir_all(&root).unwrap();
        write_backup(&root.join(PROFILE_BACKUP), None).unwrap();
        write_backup(&root.join(SYSINFO_BACKUP), Some(b"tampered-backup")).unwrap();
        let mut journal = Journal {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            device_id,
            generation_before,
            published_generation: None,
            profile_existed: false,
            sysinfo_existed: true,
            profile_backup_blake3: None,
            sysinfo_backup_blake3: Some(content_hash(b"foreign-sysinfo")),
        };
        std::fs::write(profile_path(&mount), b"partial-profile").unwrap();
        std::fs::write(sysinfo_path(&mount), b"partial-sysinfo").unwrap();
        journal.published_generation = Some(session.capture_current_generation().unwrap());
        save_journal(&root, &journal).unwrap();

        let error = recover(&session).unwrap_err();

        assert!(format!("{error:#}").contains("backup content differs"));
        assert_eq!(
            std::fs::read(profile_path(&mount)).unwrap(),
            b"partial-profile"
        );
        assert_eq!(
            std::fs::read(sysinfo_path(&mount)).unwrap(),
            b"partial-sysinfo"
        );
        assert!(root.exists());
    }
}
