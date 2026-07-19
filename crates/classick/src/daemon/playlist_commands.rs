use crate::atomic_file::AtomicFileWriter;
use crate::daemon::device_registry::DeviceRegistry;
use crate::device_config::Subscriptions;
use crate::playlist::{Playlist, PlaylistStore};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeletePlaylistOutcome {
    pub request_id: String,
    pub deleted: bool,
    pub changed_revisions: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MutationPhase {
    Prepared,
    Publishing,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistMutation {
    live_path: PathBuf,
    staged_original_path: PathBuf,
    original_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SubscriptionMutation {
    serial: String,
    live_path: PathBuf,
    staged_original_path: PathBuf,
    staged_target_path: PathBuf,
    original_hash: String,
    target_hash: String,
    original_revision: u64,
    target_revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct MutationJournal {
    version: u32,
    request_id: String,
    slug: String,
    phase: MutationPhase,
    playlist: PlaylistMutation,
    subscriptions: Vec<SubscriptionMutation>,
}

pub(crate) fn delete_and_scrub_subscriptions(
    store: &PlaylistStore,
    registry: &mut DeviceRegistry,
    state_root: &Path,
    slug: &str,
    request_id: &str,
) -> Result<DeletePlaylistOutcome> {
    validate_component("playlist slug", slug)?;
    validate_component("request id", request_id)?;

    let Some(playlist) = store
        .load(slug)
        .with_context(|| format!("load playlist {slug:?} before deletion"))?
    else {
        return Ok(DeletePlaylistOutcome {
            request_id: request_id.to_string(),
            deleted: false,
            changed_revisions: BTreeMap::new(),
        });
    };

    let playlist_live = playlist_path(state_root, &playlist);
    let stale_other_kind = match &playlist {
        Playlist::Manual(_) => state_root
            .join("playlists")
            .join(format!("{slug}.rules.json")),
        Playlist::Smart(_) => state_root.join("playlists").join(format!("{slug}.m3u8")),
    };
    if stale_other_kind.exists() {
        bail!("playlist {slug:?} exists as both manual and smart");
    }
    let playlist_bytes = std::fs::read(&playlist_live)
        .with_context(|| format!("read playlist {}", playlist_live.display()))?;
    let mutation_root = mutation_root(state_root);
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    let journal_path = mutation_root.join(format!("{request_id}.json"));
    if journal_path.exists() || stage_root.exists() {
        bail!("playlist mutation request id {request_id:?} already exists");
    }

    let playlist_mutation = PlaylistMutation {
        live_path: relative_to(state_root, &playlist_live)?,
        staged_original_path: relative_to(state_root, &stage_root.join("playlist.original"))?,
        original_hash: hash(&playlist_bytes),
    };
    let writer = AtomicFileWriter::new();
    let mut prepared = Vec::new();
    for record in registry.records() {
        let live = subscription_path(state_root, &record.serial);
        let bytes = match std::fs::read(&live) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read subscriptions {}", live.display()))
            }
        };
        let mut parsed: Subscriptions = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse subscriptions {}", live.display()))?;
        if !parsed.playlists.iter().any(|candidate| candidate == slug) {
            continue;
        }
        parsed.playlists.retain(|candidate| candidate != slug);
        let target = serde_json::to_vec_pretty(&parsed)
            .with_context(|| format!("encode subscriptions for {:?}", record.serial))?;
        let index = prepared.len();
        let original_stage = stage_root.join(format!("subscription-{index}.original"));
        let target_stage = stage_root.join(format!("subscription-{index}.target"));
        let target_revision = record
            .subscriptions_revision
            .checked_add(1)
            .ok_or_else(|| anyhow!("subscriptions revision overflow for {:?}", record.serial))?;
        prepared.push((
            SubscriptionMutation {
                serial: record.serial,
                live_path: relative_to(state_root, &live)?,
                staged_original_path: relative_to(state_root, &original_stage)?,
                staged_target_path: relative_to(state_root, &target_stage)?,
                original_hash: hash(&bytes),
                target_hash: hash(&target),
                original_revision: record.subscriptions_revision,
                target_revision,
            },
            bytes,
            target,
        ));
    }
    for (mutation, original, target) in &prepared {
        writer.write(&state_root.join(&mutation.staged_original_path), original)?;
        writer.write(&state_root.join(&mutation.staged_target_path), target)?;
    }

    let mut journal = MutationJournal {
        version: JOURNAL_VERSION,
        request_id: request_id.to_string(),
        slug: slug.to_string(),
        phase: MutationPhase::Prepared,
        playlist: playlist_mutation,
        subscriptions: prepared
            .into_iter()
            .map(|(mutation, _, _)| mutation)
            .collect(),
    };
    save_journal(&writer, &journal_path, &journal)?;
    journal.phase = MutationPhase::Publishing;
    save_journal(&writer, &journal_path, &journal)?;

    if let Err(error) = publish(&journal, registry, state_root) {
        if let Err(rollback_error) = restore(&journal, registry, state_root) {
            return Err(error).context(format!(
                "playlist deletion failed and rollback remains pending: {rollback_error:#}"
            ));
        }
        cleanup(&journal_path, &stage_root, &mutation_root)?;
        return Err(error);
    }

    let changed_revisions = journal
        .subscriptions
        .iter()
        .map(|mutation| (mutation.serial.clone(), mutation.target_revision))
        .collect();
    cleanup(&journal_path, &stage_root, &mutation_root)?;
    Ok(DeletePlaylistOutcome {
        request_id: request_id.to_string(),
        deleted: true,
        changed_revisions,
    })
}

pub(crate) fn recover_pending_playlist_mutations(
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    let root = mutation_root(state_root);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read playlist mutations {}", root.display()))
        }
    };
    let mut journals = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .with_context(|| format!("read entry in {}", root.display()))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    journals.sort();
    for journal_path in journals {
        let bytes = std::fs::read(&journal_path)
            .with_context(|| format!("read playlist mutation {}", journal_path.display()))?;
        let journal: MutationJournal = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode playlist mutation {}", journal_path.display()))?;
        validate_journal(&journal, &journal_path, registry, state_root)?;
        match journal.phase {
            MutationPhase::Prepared => restore(&journal, registry, state_root)?,
            MutationPhase::Publishing => publish(&journal, registry, state_root)?,
        }
        let stage_root = state_root.join(
            journal
                .playlist
                .staged_original_path
                .parent()
                .context("playlist staged path has no parent")?,
        );
        cleanup(&journal_path, &stage_root, &root)?;
    }
    Ok(())
}

fn publish(
    journal: &MutationJournal,
    registry: &mut DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    for mutation in &journal.subscriptions {
        let live = state_root.join(&mutation.live_path);
        match file_hash(&live)? {
            Some(actual) if actual == mutation.target_hash => {}
            Some(actual) if actual == mutation.original_hash => {
                let target = state_root.join(&mutation.staged_target_path);
                require_hash(&target, &mutation.target_hash)?;
                replace(&target, &live).with_context(|| {
                    format!(
                        "publish subscriptions {} -> {}",
                        target.display(),
                        live.display()
                    )
                })?;
            }
            actual => bail!(
                "subscriptions {} have unexpected hash {:?}",
                live.display(),
                actual
            ),
        }
    }

    let playlist_live = state_root.join(&journal.playlist.live_path);
    let playlist_stage = state_root.join(&journal.playlist.staged_original_path);
    match file_hash(&playlist_live)? {
        Some(actual) if actual == journal.playlist.original_hash => {
            if playlist_stage.exists() {
                require_hash(&playlist_stage, &journal.playlist.original_hash)?;
                std::fs::remove_file(&playlist_live).with_context(|| {
                    format!("remove duplicate playlist {}", playlist_live.display())
                })?;
            } else {
                std::fs::rename(&playlist_live, &playlist_stage).with_context(|| {
                    format!(
                        "stage deleted playlist {} -> {}",
                        playlist_live.display(),
                        playlist_stage.display()
                    )
                })?;
            }
        }
        None => require_hash(&playlist_stage, &journal.playlist.original_hash)?,
        actual => bail!(
            "playlist {} has unexpected hash {:?}",
            playlist_live.display(),
            actual
        ),
    }

    for mutation in &journal.subscriptions {
        let current = registry
            .record(&mutation.serial)
            .with_context(|| {
                format!(
                    "missing device {:?} during playlist mutation",
                    mutation.serial
                )
            })?
            .subscriptions_revision;
        if current == mutation.target_revision {
            continue;
        }
        if current != mutation.original_revision {
            bail!(
                "device {:?} subscriptions revision is {}, expected {} or {}",
                mutation.serial,
                current,
                mutation.original_revision,
                mutation.target_revision
            );
        }
        registry.advance_config_revisions(&mutation.serial, false, false, true)?;
    }
    Ok(())
}

fn restore(journal: &MutationJournal, registry: &DeviceRegistry, state_root: &Path) -> Result<()> {
    for mutation in &journal.subscriptions {
        let current_revision = registry
            .record(&mutation.serial)
            .with_context(|| format!("missing device {:?} during rollback", mutation.serial))?
            .subscriptions_revision;
        if current_revision != mutation.original_revision {
            bail!(
                "cannot restore device {:?} at subscriptions revision {} (expected {})",
                mutation.serial,
                current_revision,
                mutation.original_revision
            );
        }
        let live = state_root.join(&mutation.live_path);
        match file_hash(&live)? {
            Some(actual) if actual == mutation.original_hash => {}
            Some(actual) if actual == mutation.target_hash => {
                let original = state_root.join(&mutation.staged_original_path);
                require_hash(&original, &mutation.original_hash)?;
                replace(&original, &live).with_context(|| {
                    format!(
                        "restore subscriptions {} -> {}",
                        original.display(),
                        live.display()
                    )
                })?;
            }
            actual => bail!(
                "cannot restore subscriptions {} with hash {:?}",
                live.display(),
                actual
            ),
        }
    }
    let live = state_root.join(&journal.playlist.live_path);
    let staged = state_root.join(&journal.playlist.staged_original_path);
    match file_hash(&live)? {
        Some(actual) if actual == journal.playlist.original_hash => {}
        None => {
            require_hash(&staged, &journal.playlist.original_hash)?;
            std::fs::rename(&staged, &live).with_context(|| {
                format!(
                    "restore playlist {} -> {}",
                    staged.display(),
                    live.display()
                )
            })?;
        }
        actual => bail!(
            "cannot restore playlist {} with hash {:?}",
            live.display(),
            actual
        ),
    }
    Ok(())
}

fn validate_journal(
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

fn playlist_path(state_root: &Path, playlist: &Playlist) -> PathBuf {
    match playlist {
        Playlist::Manual(manual) => state_root
            .join("playlists")
            .join(format!("{}.m3u8", manual.slug)),
        Playlist::Smart(smart) => state_root
            .join("playlists")
            .join(format!("{}.rules.json", smart.slug)),
    }
}

fn subscription_path(state_root: &Path, serial: &str) -> PathBuf {
    state_root
        .join("devices")
        .join(crate::device_state::sanitize_serial(serial))
        .join("subscriptions.json")
}

fn mutation_root(state_root: &Path) -> PathBuf {
    state_root.join("devices").join("playlist-mutations")
}

fn save_journal(writer: &AtomicFileWriter, path: &Path, journal: &MutationJournal) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(journal).context("encode playlist mutation journal")?;
    writer
        .write(path, &bytes)
        .context("write playlist mutation journal")
}

fn cleanup(journal: &Path, stage: &Path, root: &Path) -> Result<()> {
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

fn relative_to(root: &Path, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))
}

fn validate_component(label: &str, value: &str) -> Result<()> {
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

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn file_hash(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(hash(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn require_hash(path: &Path, expected: &str) -> Result<()> {
    match file_hash(path)? {
        Some(actual) if actual == expected => Ok(()),
        actual => bail!("{} has unexpected hash {:?}", path.display(), actual),
    }
}

#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
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
