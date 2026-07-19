//! Recoverable publication of per-device config files and their registry revisions.

use crate::atomic_file::AtomicFileWriter;
use crate::daemon::device_registry::DeviceRegistry;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigComponentKind {
    Selection,
    Settings,
    Subscriptions,
}

#[derive(Debug)]
pub(crate) struct ConfigComponentUpdate {
    pub(crate) kind: ConfigComponentKind,
    pub(crate) live_path: PathBuf,
    pub(crate) target_contents: Vec<u8>,
    pub(crate) failure_message: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct CommitOutcome {
    pub(crate) selection_changed: bool,
    pub(crate) settings_changed: bool,
    pub(crate) subscriptions_changed: bool,
    pub(crate) component_failure: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Revisions {
    selection: u64,
    settings: u64,
    subscriptions: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalComponent {
    kind: ConfigComponentKind,
    live_path: PathBuf,
    original_contents: Option<Vec<u8>>,
    target_contents: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MutationJournal {
    version: u32,
    request_id: String,
    serial: String,
    original_revisions: Revisions,
    components: Vec<JournalComponent>,
}

pub(crate) fn commit(
    registry: &mut DeviceRegistry,
    state_root: &Path,
    serial: &str,
    request_id: &str,
    updates: Vec<ConfigComponentUpdate>,
) -> Result<CommitOutcome> {
    if updates.is_empty() {
        return Ok(CommitOutcome::default());
    }
    let original_revisions = revisions(registry, serial)?;
    let root = journal_root(state_root);
    let journal_path = journal_path(&root, request_id);
    if journal_path.exists() {
        bail!("device config mutation request already exists");
    }

    let mut seen = BTreeSet::new();
    let mut journal = MutationJournal {
        version: JOURNAL_VERSION,
        request_id: request_id.to_string(),
        serial: serial.to_string(),
        original_revisions,
        components: updates
            .iter()
            .map(|update| {
                if !seen.insert(update.kind) {
                    bail!("duplicate device config component");
                }
                let live_path = relative_to(state_root, &update.live_path)?;
                validate_live_path(state_root, serial, update.kind, &live_path)?;
                let original_contents = match std::fs::read(&update.live_path) {
                    Ok(contents) => Some(contents),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("read device config {}", update.live_path.display())
                        })
                    }
                };
                Ok(JournalComponent {
                    kind: update.kind,
                    live_path,
                    original_contents,
                    target_contents: update.target_contents.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let writer = AtomicFileWriter::new();
    save_journal(&writer, &journal_path, &journal)?;

    let mut succeeded = BTreeSet::new();
    let mut component_failure = None;
    for update in &updates {
        if let Err(error) = publish_component(update) {
            tracing::error!(
                serial,
                component = ?update.kind,
                "daemon: failed to publish device config component: {error:#}"
            );
            component_failure.get_or_insert(update.failure_message);
        } else {
            succeeded.insert(update.kind);
        }
    }

    if succeeded.len() != journal.components.len() {
        journal
            .components
            .retain(|component| succeeded.contains(&component.kind));
        if journal.components.is_empty() {
            remove_journal(&journal_path, &root)?;
            return Ok(CommitOutcome {
                component_failure,
                ..CommitOutcome::default()
            });
        }
        if let Err(error) = save_journal(&writer, &journal_path, &journal) {
            restore_originals(&writer, state_root, &journal)
                .context("restore device config after journal narrowing failed")?;
            remove_journal(&journal_path, &root)?;
            return Err(error).context("narrow device config journal to persisted components");
        }
    }

    let changed = changed_components(&journal);
    if let Err(error) = registry.advance_config_revisions(
        serial,
        changed.selection_changed,
        changed.settings_changed,
        changed.subscriptions_changed,
    ) {
        if let Err(rollback_error) = restore_originals(&writer, state_root, &journal) {
            return Err(error).context(format!(
                "advance device config revisions failed and rollback remains pending: {rollback_error:#}"
            ));
        }
        remove_journal(&journal_path, &root)?;
        return Err(error).context("advance device config revisions");
    }

    remove_journal(&journal_path, &root)?;
    Ok(CommitOutcome {
        component_failure,
        ..changed
    })
}

fn publish_component(update: &ConfigComponentUpdate) -> Result<()> {
    match update.kind {
        ConfigComponentKind::Selection => {
            let value = serde_json::from_slice(&update.target_contents)
                .context("decode staged device selection")?;
            crate::selection::save_atomic(&update.live_path, &value)
        }
        ConfigComponentKind::Settings => {
            let value = serde_json::from_slice(&update.target_contents)
                .context("decode staged device settings")?;
            crate::device_config::DeviceSettings::save_atomic(&update.live_path, &value)
        }
        ConfigComponentKind::Subscriptions => {
            let value = serde_json::from_slice(&update.target_contents)
                .context("decode staged device subscriptions")?;
            crate::device_config::Subscriptions::save_atomic(&update.live_path, &value)
        }
    }
}

/// Recovery is revision-directed: original revisions restore exact originals;
/// target revisions require exact target bytes. Mixed revisions are unsafe.
pub(crate) fn recover_pending(registry: &DeviceRegistry, state_root: &Path) -> Result<()> {
    let root = journal_root(state_root);
    for journal_path in journal_paths(&root)? {
        let bytes = std::fs::read(&journal_path)
            .with_context(|| format!("read device config journal {}", journal_path.display()))?;
        let journal: MutationJournal = serde_json::from_slice(&bytes)
            .with_context(|| format!("decode device config journal {}", journal_path.display()))?;
        validate_journal(&journal, &journal_path, registry, state_root)?;
        let current = revisions(registry, &journal.serial)?;
        let target = target_revisions(&journal)?;
        if current.selection == journal.original_revisions.selection
            && current.settings == journal.original_revisions.settings
            && current.subscriptions == journal.original_revisions.subscriptions
        {
            require_original_or_target(state_root, &journal)?;
            restore_originals(&AtomicFileWriter::new(), state_root, &journal)?;
        } else if current.selection == target.selection
            && current.settings == target.settings
            && current.subscriptions == target.subscriptions
        {
            require_targets(state_root, &journal)?;
        } else {
            bail!(
                "device config journal {:?} has mixed or unexpected revisions",
                journal.request_id
            );
        }
        remove_journal(&journal_path, &root)?;
    }
    Ok(())
}

pub(crate) fn has_pending(state_root: &Path) -> Result<bool> {
    Ok(!journal_paths(&journal_root(state_root))?.is_empty())
}

fn changed_components(journal: &MutationJournal) -> CommitOutcome {
    CommitOutcome {
        selection_changed: journal
            .components
            .iter()
            .any(|component| component.kind == ConfigComponentKind::Selection),
        settings_changed: journal
            .components
            .iter()
            .any(|component| component.kind == ConfigComponentKind::Settings),
        subscriptions_changed: journal
            .components
            .iter()
            .any(|component| component.kind == ConfigComponentKind::Subscriptions),
        component_failure: None,
    }
}

fn revisions(registry: &DeviceRegistry, serial: &str) -> Result<Revisions> {
    let record = registry
        .record(serial)
        .with_context(|| format!("missing device {serial:?} for config mutation"))?;
    Ok(Revisions {
        selection: record.selection_revision,
        settings: record.settings_revision,
        subscriptions: record.subscriptions_revision,
    })
}

fn target_revisions(journal: &MutationJournal) -> Result<Revisions> {
    let changed = changed_components(journal);
    Ok(Revisions {
        selection: advance(
            journal.original_revisions.selection,
            changed.selection_changed,
        )?,
        settings: advance(
            journal.original_revisions.settings,
            changed.settings_changed,
        )?,
        subscriptions: advance(
            journal.original_revisions.subscriptions,
            changed.subscriptions_changed,
        )?,
    })
}

fn advance(value: u64, changed: bool) -> Result<u64> {
    if changed {
        value
            .checked_add(1)
            .context("device config revision overflow")
    } else {
        Ok(value)
    }
}

fn restore_originals(
    writer: &AtomicFileWriter,
    state_root: &Path,
    journal: &MutationJournal,
) -> Result<()> {
    for component in journal.components.iter().rev() {
        let live = state_root.join(&component.live_path);
        match &component.original_contents {
            Some(contents) => writer
                .write(&live, contents)
                .with_context(|| format!("restore device config {}", live.display()))?,
            None => match std::fs::remove_file(&live) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("remove device config {}", live.display()))
                }
            },
        }
    }
    Ok(())
}

fn require_original_or_target(state_root: &Path, journal: &MutationJournal) -> Result<()> {
    for component in &journal.components {
        let live = state_root.join(&component.live_path);
        let actual = match std::fs::read(&live) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("read device config {}", live.display()))
            }
        };
        let matches_original = actual.as_deref() == component.original_contents.as_deref();
        let matches_target = actual.as_deref() == Some(component.target_contents.as_slice());
        if !matches_original && !matches_target {
            bail!(
                "device config {} differs from journal original and target",
                live.display()
            );
        }
    }
    Ok(())
}

fn require_targets(state_root: &Path, journal: &MutationJournal) -> Result<()> {
    for component in &journal.components {
        let live = state_root.join(&component.live_path);
        let actual = std::fs::read(&live)
            .with_context(|| format!("read committed device config {}", live.display()))?;
        if actual != component.target_contents {
            bail!(
                "committed device config {} differs from journal",
                live.display()
            );
        }
    }
    Ok(())
}

fn validate_journal(
    journal: &MutationJournal,
    path: &Path,
    registry: &DeviceRegistry,
    state_root: &Path,
) -> Result<()> {
    if journal.version != JOURNAL_VERSION || journal.request_id.is_empty() {
        bail!("invalid device config journal header");
    }
    if journal_path(&journal_root(state_root), &journal.request_id) != path {
        bail!("device config journal filename does not match request id");
    }
    let record = registry
        .record(&journal.serial)
        .context("unknown device in config journal")?;
    if record.serial != journal.serial {
        bail!("device config journal serial is not canonical");
    }
    let mut seen = BTreeSet::new();
    for component in &journal.components {
        if !seen.insert(component.kind) {
            bail!("duplicate component in device config journal");
        }
        validate_live_path(
            state_root,
            &journal.serial,
            component.kind,
            &component.live_path,
        )?;
    }
    target_revisions(journal)?;
    Ok(())
}

fn validate_live_path(
    state_root: &Path,
    serial: &str,
    kind: ConfigComponentKind,
    relative: &Path,
) -> Result<()> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("unsafe device config path {}", relative.display());
    }
    let expected = match kind {
        ConfigComponentKind::Selection => {
            crate::device_state::device_selection_path_in(state_root, serial)?
        }
        ConfigComponentKind::Settings => {
            crate::device_state::device_settings_path_in(state_root, serial)?
        }
        ConfigComponentKind::Subscriptions => {
            crate::device_state::device_subscriptions_path_in(state_root, serial)?
        }
    };
    if state_root.join(relative) != expected {
        bail!("device config path does not match component and serial");
    }
    Ok(())
}

fn relative_to(root: &Path, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))
}

fn journal_root(state_root: &Path) -> PathBuf {
    state_root.join("devices").join("config-mutations")
}

fn journal_path(root: &Path, request_id: &str) -> PathBuf {
    root.join(format!(
        "{}.json",
        blake3::hash(request_id.as_bytes()).to_hex()
    ))
}

fn journal_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read config mutations {}", root.display()))
        }
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn save_journal(writer: &AtomicFileWriter, path: &Path, journal: &MutationJournal) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(journal).context("encode device config journal")?;
    writer
        .write(path, &bytes)
        .context("write device config journal")
}

fn remove_journal(path: &Path, root: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("remove journal {}", path.display()))
        }
    }
    if let Err(error) = std::fs::remove_dir(root) {
        if !matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
        ) {
            tracing::warn!(
                "device config journal cleanup failed at {}: {error}",
                root.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "device_config_transaction_tests.rs"]
mod tests;
