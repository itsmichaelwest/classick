use crate::atomic_file::AtomicFileWriter;
use crate::daemon::device_registry::DeviceRegistry;
use crate::playlist::Playlist;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(super) const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum MutationPhase {
    Prepared,
    Publishing,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PlaylistMutation {
    pub(super) live_path: PathBuf,
    pub(super) staged_original_path: PathBuf,
    pub(super) original_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SubscriptionMutation {
    pub(super) serial: String,
    pub(super) live_path: PathBuf,
    pub(super) staged_original_path: PathBuf,
    pub(super) staged_target_path: PathBuf,
    pub(super) original_hash: String,
    pub(super) target_hash: String,
    pub(super) original_revision: u64,
    pub(super) target_revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct MutationJournal {
    pub(super) version: u32,
    pub(super) request_id: String,
    pub(super) slug: String,
    pub(super) phase: MutationPhase,
    pub(super) playlist: PlaylistMutation,
    pub(super) subscriptions: Vec<SubscriptionMutation>,
}

pub(super) fn validate_journal(
    journal: &MutationJournal,
    journal_path: &Path,
    registry: &DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    if journal.version != JOURNAL_VERSION {
        bail!("unsupported playlist mutation version {}", journal.version);
    }
    validate_component("request id", &journal.request_id)?;
    validate_component("playlist slug", &journal.slug)?;
    let file_request_id = journal_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("playlist mutation filename is not UTF-8")?;
    if file_request_id != journal.request_id {
        bail!("playlist mutation filename does not match request id");
    }
    let expected_stage = PathBuf::from("devices")
        .join("playlist-mutations")
        .join(format!("{}.staged", journal.request_id));
    validate_relative(&journal.playlist.live_path)?;
    validate_staged(&journal.playlist.staged_original_path, &expected_stage)?;
    if journal.playlist.live_path.parent() != Some(Path::new("playlists")) {
        bail!("playlist live path is outside playlists directory");
    }
    let expected_manual = format!("{}.m3u8", journal.slug);
    let expected_smart = format!("{}.rules.json", journal.slug);
    let playlist_name = journal
        .playlist
        .live_path
        .file_name()
        .and_then(|name| name.to_str());
    if playlist_name != Some(&expected_manual) && playlist_name != Some(&expected_smart) {
        bail!("playlist live path does not match journal slug");
    }
    let mut serials = BTreeSet::new();
    for mutation in &journal.subscriptions {
        if !serials.insert(crate::daemon::device_registry::canonical_serial_key(
            &mutation.serial,
        )) {
            bail!(
                "duplicate device {:?} in playlist mutation",
                mutation.serial
            );
        }
        validate_relative(&mutation.live_path)?;
        validate_staged(&mutation.staged_original_path, &expected_stage)?;
        validate_staged(&mutation.staged_target_path, &expected_stage)?;
        let record = registry.record(&mutation.serial).with_context(|| {
            format!("unknown device {:?} in playlist mutation", mutation.serial)
        })?;
        let expected_live =
            relative_to(state_root, &subscription_path(state_root, &record.serial))?;
        if mutation.live_path != expected_live {
            bail!(
                "subscription path does not match device {:?}",
                mutation.serial
            );
        }
        if mutation.target_revision
            != mutation
                .original_revision
                .checked_add(1)
                .context("revision overflow")?
        {
            bail!("playlist mutation target revision is not the next revision");
        }
    }
    Ok(())
}

pub(super) fn playlist_path(state_root: &Path, playlist: &Playlist) -> PathBuf {
    match playlist {
        Playlist::Manual(manual) => state_root
            .join("playlists")
            .join(format!("{}.m3u8", manual.slug)),
        Playlist::Smart(smart) => state_root
            .join("playlists")
            .join(format!("{}.rules.json", smart.slug)),
    }
}

pub(super) fn subscription_path(state_root: &Path, serial: &str) -> PathBuf {
    state_root
        .join("devices")
        .join(crate::device_state::sanitize_serial(serial))
        .join("subscriptions.json")
}

pub(super) fn mutation_root(state_root: &Path) -> PathBuf {
    state_root.join("devices").join("playlist-mutations")
}

pub(super) fn save_journal(
    writer: &AtomicFileWriter,
    path: &Path,
    journal: &MutationJournal,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(journal).context("encode playlist mutation journal")?;
    writer
        .write(path, &bytes)
        .context("write playlist mutation journal")
}

pub(super) fn cleanup(journal: &Path, stage: &Path, root: &Path) -> Result<()> {
    remove_file_if_exists(journal)?;
    if let Err(error) = std::fs::remove_dir_all(stage) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                "playlist mutation committed but staging cleanup failed at {}: {error}",
                stage.display()
            );
        }
    }
    if let Err(error) = std::fs::remove_dir(root) {
        if !matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
        ) {
            tracing::warn!(
                "playlist mutation directory cleanup failed at {}: {error}",
                root.display()
            );
        }
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

pub(super) fn relative_to(root: &Path, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))
}

pub(super) fn validate_component(label: &str, value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        bail!("{label} must be one non-empty path component");
    }
    Ok(())
}

fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe playlist mutation path {}", path.display());
    }
    Ok(())
}

fn validate_staged(path: &Path, expected_root: &Path) -> Result<()> {
    validate_relative(path)?;
    let relative = path
        .strip_prefix(expected_root)
        .with_context(|| format!("staged path {} is outside journal staging", path.display()))?;
    if relative.components().count() != 1 {
        bail!("staged path {} is not a direct child", path.display());
    }
    Ok(())
}

pub(super) fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub(super) fn file_hash(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(hash(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub(super) fn require_hash(path: &Path, expected: &str) -> Result<()> {
    match file_hash(path)? {
        Some(actual) if actual == expected => Ok(()),
        actual => bail!("{} has unexpected hash {:?}", path.display(), actual),
    }
}

#[cfg(not(windows))]
pub(super) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
pub(super) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
