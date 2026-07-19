use super::platform;
#[cfg(unix)]
use crate::rockbox_playlist::validate_recorded_filename;
use anyhow::{bail, Context, Result};

pub(super) fn recorded_entry_state(
    directory: &platform::ManagedDirectory,
    name: &str,
) -> Result<platform::EntryKind> {
    let kind = directory.entry_kind(name)?;
    if kind == platform::EntryKind::Regular && !directory.has_exact_entry(name)? {
        return Ok(platform::EntryKind::Other);
    }
    Ok(kind)
}

pub(super) fn entry_content_matches(
    directory: &platform::ManagedDirectory,
    name: &str,
    expected_hash: &str,
) -> Result<bool> {
    let bytes = directory.read(name)?;
    Ok(blake3::hash(&bytes).to_hex().as_str() == expected_hash)
}

#[cfg(unix)]
pub(super) fn deletion_quarantine_name(name: &str, expected_hash: &str) -> Result<String> {
    validate_recorded_filename(name)?;
    validate_recorded_hash(expected_hash)?;
    let digest = blake3::hash(format!("{name}\0{expected_hash}").as_bytes());
    Ok(format!(".classick-delete-{}.tmp", digest.to_hex().as_str()))
}

pub(super) fn validate_recorded_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("Rockbox projection content hash is not lowercase BLAKE3 hex");
    }
    Ok(())
}

pub(super) fn sync_after_delete(directory: &platform::ManagedDirectory, name: &str) -> Result<()> {
    directory
        .sync()
        .context("sync managed projection directory")?;
    directory
        .ensure_path_identity()
        .with_context(|| format!("revalidate managed root after deleting projection {name:?}"))
}
