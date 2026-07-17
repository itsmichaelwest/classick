//! Classick-managed iTunesDB playlist reconcile, plus the host-store <->
//! iPod mirror that backs up playlists onto the device and lets a fresh
//! install adopt them back (spec §3).
//!
//! ## Managed-identity model
//!
//! Classick "owns" exactly the set of on-device playlists recorded in
//! `devices/<serial>/managed_playlists.json` after the last successful
//! `reconcile` — identified by **itdb playlist id**, NEVER by name alone.
//! This is what keeps a user's own iTunes-authored (or Rockbox, or
//! hand-made) playlist safe even if it happens to share a name with one
//! Classick would otherwise create: since its id was never recorded, both
//! the create/update side (`ipod::db::ensure_managed_playlist`) and the
//! removal side treat it as untouchable. A name collision with a NEWLY
//! desired playlist name results in two same-named playlists on the
//! device — Classick's new one, and the foreign one, left exactly as it
//! was — rather than either side adopting the foreign playlist in place.

use crate::ipod::db::{ensure_managed_playlist, remove_playlist_by_id, remove_playlist_by_name, OwnedDb};
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Outcome of one `reconcile` call. All-zero on a run where the desired set
/// exactly matched what was already on the device.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    pub created: usize,
    pub updated: usize,
    pub removed: usize,
}

/// One entry in `managed_playlists.json`: a display name plus the itdb
/// playlist id that was assigned to it as of the last successful
/// reconcile, if known. `id` is the actual source of managed identity (see
/// the module doc comment); `name` is carried alongside for display and as
/// the join key `reconcile_at` uses to find a desired entry's previous id.
///
/// Deserializes from either the current `{"name": ..., "id": ...}` shape
/// or a bare JSON string (the pre-migration format, when this file only
/// ever recorded names) — a legacy string entry becomes `{name, id: None}`,
/// which `ensure_managed_playlist` treats as "no recorded id" (see its doc
/// comment for the consequence: legacy entries never get their on-device
/// playlist adopted by name, only ever superseded by a fresh one).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedPlaylistEntry {
    pub name: String,
    pub id: Option<u64>,
}

impl<'de> Deserialize<'de> for ManagedPlaylistEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(String),
            Full { name: String, id: Option<u64> },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Legacy(name) => ManagedPlaylistEntry { name, id: None },
            Repr::Full { name, id } => ManagedPlaylistEntry { name, id },
        })
    }
}

/// Persisted record of the playlists Classick manages on one device — see
/// the module doc comment for why the entries' `id` field, and not
/// playlist-name heuristics, is the source of truth for what `reconcile`
/// is allowed to touch or remove. Written by every `reconcile` call (even
/// a no-op run), so the next run's decisions are always based on fresh
/// truth rather than a stale file. Entries are stored sorted by name for a
/// deterministic, diff-friendly file on disk.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedPlaylists {
    pub names: Vec<ManagedPlaylistEntry>,
}

impl ManagedPlaylists {
    /// Load the record at `path`. A missing file (fresh device, never
    /// reconciled) or a file that fails to parse (corrupt/foreign content)
    /// both fall back to an empty record rather than erroring — the safe
    /// default is "we don't believe we manage anything yet", which makes
    /// the caller's removal pass a no-op instead of accidentally treating
    /// unrelated on-device playlists as ours.
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!(
                    "managed_playlists.json at {} failed to parse ({e}); treating as empty",
                    path.display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Write the record atomically (tmp file + rename), same pattern as
    /// `manifest::save_atomic`. Failure here is surfaced to the caller
    /// (unlike the mirror/adopt paths below): if we can't durably record
    /// what we manage, the next run's removal decision would be made on
    /// stale information, which is a correctness problem worth failing the
    /// sync over rather than silently risking either "stuck" leftover
    /// playlists or (worse) a future false-positive removal.
    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, json.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

/// Reconcile the DB's Classick-managed playlists against `desired` using
/// the real per-device state directory (`devices/<serial>/managed_playlists.json`).
/// See [`reconcile_in`] for the test/override variant and the full
/// algorithm description.
pub fn reconcile(db: &OwnedDb, desired: &[(String, Vec<u64>)], serial: &str) -> Result<ReconcileStats> {
    let record_path = crate::device_state::managed_playlists_path(serial)?;
    reconcile_at(db, desired, &record_path)
}

/// Test/override variant of [`reconcile`]: uses `root/devices/<serial>/managed_playlists.json`.
pub fn reconcile_in(
    db: &OwnedDb,
    desired: &[(String, Vec<u64>)],
    root: &Path,
    serial: &str,
) -> Result<ReconcileStats> {
    let record_path = crate::device_state::managed_playlists_path_in(root, serial)?;
    reconcile_at(db, desired, &record_path)
}

/// Core reconcile: `desired` is `(playlist name, member dbids)`, in the
/// order the caller wants playlists ensured (apply_loop passes subscription
/// order). Playlists recorded in the PREVIOUS managed record but absent
/// from `desired` (by name) are removed by their recorded id; everything in
/// `desired` is ensured by recorded id (see `ipod::db::ensure_managed_playlist`
/// for what "ensured by id" means and why — reuse-by-id only, never
/// adopt-by-name). If `desired` contains the same name twice, the later
/// entry wins (last `ensure_managed_playlist` call overwrites the earlier
/// one's membership) — apply_loop's join never produces duplicate names in
/// practice (one playlist store entry per subscribed slug), but this
/// function doesn't defend against a caller that does.
///
/// The previous record's id for a desired name is looked up by matching
/// `name` against the previous record's entries — this is also how a
/// rename is expressed: if the CALLER already knows a desired slot's
/// previous display name changed, it's expected to have updated the
/// on-disk record's `name` field for that id before calling (or, in
/// practice, a future slug-keyed caller passes the recorded id directly
/// rather than through this record). Within one `reconcile_at` call,
/// `ensure_managed_playlist` itself performs the actual rename against the
/// live DB once it resolves the recorded id.
///
/// # Failure paths
/// - An individual `ensure_managed_playlist` call fails (e.g. malformed
///   name): logged via `tracing::warn!` and skipped — that one playlist is
///   left out of the new managed record (so it's retried as "created" next
///   run rather than silently forgotten), but the rest of `desired` and
///   the removal pass still proceed. One bad playlist must not block every
///   other playlist's reconcile.
/// - An individual removal call fails (MPL guard, or a malformed legacy
///   name): logged via `tracing::warn!`; that entry is kept in the new
///   managed record so the NEXT run retries the removal, rather than
///   silently dropping bookkeeping for a playlist that's still actually
///   present on the device.
/// - Writing the new `managed_playlists.json` record fails: `Err` is
///   returned (see `ManagedPlaylists::save`'s doc comment for why this one
///   failure mode isn't swallowed).
fn reconcile_at(
    db: &OwnedDb,
    desired: &[(String, Vec<u64>)],
    record_path: &Path,
) -> Result<ReconcileStats> {
    let previous = ManagedPlaylists::load(record_path);
    let previous_id_by_name: HashMap<&str, Option<u64>> =
        previous.names.iter().map(|e| (e.name.as_str(), e.id)).collect();

    let mut stats = ReconcileStats::default();
    let mut managed: Vec<ManagedPlaylistEntry> = Vec::with_capacity(desired.len());

    for (name, dbids) in desired {
        let recorded_id = previous_id_by_name.get(name.as_str()).copied().flatten();
        match ensure_managed_playlist(db, name, dbids, recorded_id) {
            Ok(new_id) => {
                // A recorded id that still resolves means we reused (and
                // possibly renamed/rewrote) the existing playlist; anything
                // else means `ensure_managed_playlist` created a fresh one
                // (no recorded id, a stale id, or an id that turned out to
                // be the MPL).
                if recorded_id == Some(new_id) {
                    stats.updated += 1;
                } else {
                    stats.created += 1;
                }
                managed.push(ManagedPlaylistEntry { name: name.clone(), id: Some(new_id) });
            }
            Err(e) => {
                tracing::warn!("device-playlists: failed to ensure \"{name}\": {e:#}");
            }
        }
    }

    let desired_names: HashSet<&str> = desired.iter().map(|(n, _)| n.as_str()).collect();
    for entry in &previous.names {
        if desired_names.contains(entry.name.as_str()) {
            continue; // still desired; already ensured above
        }
        let removal = match entry.id {
            Some(id) => remove_playlist_by_id(db, id),
            None => remove_playlist_by_name(db, &entry.name),
        };
        match removal {
            Ok(true) => {
                stats.removed += 1;
            }
            Ok(false) => {
                // Already gone (user deleted it, or a previous partial run
                // already removed it) — drop it from the managed record;
                // nothing left to retry.
            }
            Err(e) => {
                tracing::warn!("device-playlists: failed to remove \"{}\": {e:#}", entry.name);
                // Keep it in the managed record so the next run gets
                // another chance at removing it — see doc comment above.
                managed.push(entry.clone());
            }
        }
    }

    managed.sort_by(|a, b| a.name.cmp(&b.name));
    managed.dedup_by(|a, b| a.name == b.name);
    ManagedPlaylists { names: managed }.save(record_path)?;

    Ok(stats)
}

/// Copy every file directly under `playlists_root` (the host `PlaylistStore`
/// root — one `<slug>.m3u8`/`<slug>.rules.json` per playlist) plus
/// `subscriptions_path` (this device's `subscriptions.json`) to
/// `<mount>/iPod_Control/classick/playlists/`.
///
/// Call this AFTER a successful `db.write()` — mirroring before the write
/// lands would let the device's mirror claim playlists whose track adds
/// never actually made it to disk.
///
/// Entirely best-effort: any failure (missing/unreadable source dir,
/// permission error, iPod unplugged mid-copy) is logged via
/// `tracing::warn!` and swallowed. The mirror is a backup convenience, not
/// something worth failing an otherwise-successful sync over.
pub fn mirror_to_ipod(mount: &Path, playlists_root: &Path, subscriptions_path: &Path) {
    let dest_dir = crate::ipod::layout::playlists_mirror_dir(mount);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        tracing::warn!("playlist mirror: failed to create {}: {e}", dest_dir.display());
        return;
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(playlists_root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect(),
        Err(e) => {
            // NotFound is expected when nothing has ever been saved to the
            // store yet; anything else is worth a warning, but neither
            // blocks the sync.
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "playlist mirror: failed to read {}: {e}",
                    playlists_root.display()
                );
            }
            Vec::new()
        }
    };
    entries.sort();

    let mut copied = 0usize;
    for src in &entries {
        let Some(file_name) = src.file_name() else { continue };
        let dst = dest_dir.join(file_name);
        match std::fs::copy(src, &dst) {
            Ok(_) => copied += 1,
            Err(e) => tracing::warn!(
                "playlist mirror: failed to copy {} -> {}: {e}",
                src.display(),
                dst.display()
            ),
        }
    }

    if subscriptions_path.exists() {
        let dst = dest_dir.join("subscriptions.json");
        if let Err(e) = std::fs::copy(subscriptions_path, &dst) {
            tracing::warn!(
                "playlist mirror: failed to copy {} -> {}: {e}",
                subscriptions_path.display(),
                dst.display()
            );
        }
    }

    tracing::debug!("playlist mirror: copied {copied} playlist file(s) to {}", dest_dir.display());
}

/// Adopt playlists from a previously-mirrored device, once, at session
/// start — the disaster-recovery / new-machine path: a fresh Classick
/// install has an empty host playlist store, but the connected iPod
/// already carries a mirror from a prior machine or a prior install. Since
/// the host side has nothing to lose, the device's mirror is trusted as-is.
///
/// Returns the number of playlist files adopted (0 in every "nothing to do"
/// case: host store already has content, mirror dir is missing/empty, or
/// every copy failed). Never overwrites a NON-empty host store, even if the
/// device mirror has playlists the host doesn't — that isn't "empty host",
/// so this never guesses at a merge; only a truly-empty store adopts.
///
/// Entirely best-effort: any I/O failure (unreadable mirror dir, permission
/// error, disk full on the host) is logged via `tracing::warn!` and
/// swallowed per-file — one bad copy doesn't block the rest, and adoption
/// failing outright never blocks the sync that's about to run.
pub fn adopt_from_ipod(mount: &Path, playlists_root: &Path, subscriptions_path: &Path) -> usize {
    let host_has_content = match std::fs::read_dir(playlists_root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).any(|e| e.path().is_file()),
        Err(_) => false, // store dir doesn't exist yet == empty
    };
    if host_has_content {
        return 0;
    }

    let mirror_dir = crate::ipod::layout::playlists_mirror_dir(mount);
    let mut mirror_files: Vec<PathBuf> = match std::fs::read_dir(&mirror_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect(),
        Err(_) => return 0, // no mirror on this device == nothing to adopt
    };
    if mirror_files.is_empty() {
        return 0;
    }
    mirror_files.sort();

    if let Err(e) = std::fs::create_dir_all(playlists_root) {
        tracing::warn!("playlist adopt: failed to create {}: {e}", playlists_root.display());
        return 0;
    }

    let mut adopted_playlists = 0usize;
    for src in &mirror_files {
        let Some(file_name) = src.file_name() else { continue };
        if file_name == "subscriptions.json" {
            if let Some(parent) = subscriptions_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("playlist adopt: failed to create {}: {e}", parent.display());
                    continue;
                }
            }
            if let Err(e) = std::fs::copy(src, subscriptions_path) {
                tracing::warn!(
                    "playlist adopt: failed to copy {} -> {}: {e}",
                    src.display(),
                    subscriptions_path.display()
                );
            }
            continue;
        }
        let dst = playlists_root.join(file_name);
        match std::fs::copy(src, &dst) {
            Ok(_) => adopted_playlists += 1,
            Err(e) => tracing::warn!(
                "playlist adopt: failed to copy {} -> {}: {e}",
                src.display(),
                dst.display()
            ),
        }
    }

    if adopted_playlists > 0 {
        tracing::warn!("adopted {adopted_playlists} playlists from device mirror");
    }
    adopted_playlists
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temp dir under `target/` so leftover dirs don't
    /// pollute the system temp and are easy to clean. Per-test unique via
    /// an AtomicU32 counter (PID alone collides under parallel test
    /// execution — see LEARNINGS.md).
    fn tempdir_under_target(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("device-playlists-{label}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn managed_playlists_missing_file_loads_as_empty() {
        let dir = tempdir_under_target("missing");
        let path = dir.join("managed_playlists.json");
        let loaded = ManagedPlaylists::load(&path);
        assert_eq!(loaded, ManagedPlaylists::default());
    }

    #[test]
    fn managed_playlists_corrupt_file_loads_as_empty() {
        let dir = tempdir_under_target("corrupt");
        let path = dir.join("managed_playlists.json");
        std::fs::write(&path, b"not json at all").unwrap();
        let loaded = ManagedPlaylists::load(&path);
        assert_eq!(loaded, ManagedPlaylists::default());
    }

    #[test]
    fn managed_playlists_round_trips_through_save_and_load() {
        let dir = tempdir_under_target("roundtrip");
        let path = dir.join("nested").join("managed_playlists.json");
        let record = ManagedPlaylists {
            names: vec![
                ManagedPlaylistEntry { name: "Gym".to_string(), id: Some(111) },
                ManagedPlaylistEntry { name: "Road Trip".to_string(), id: Some(222) },
            ],
        };
        record.save(&path).expect("save should create parent dirs and succeed");
        let loaded = ManagedPlaylists::load(&path);
        assert_eq!(loaded, record);
    }

    #[test]
    fn managed_playlists_legacy_name_only_entries_deserialize_with_unknown_id() {
        let dir = tempdir_under_target("legacy");
        let path = dir.join("managed_playlists.json");
        std::fs::write(&path, br#"{"names":["Gym","Road Trip"]}"#).unwrap();
        let loaded = ManagedPlaylists::load(&path);
        assert_eq!(
            loaded,
            ManagedPlaylists {
                names: vec![
                    ManagedPlaylistEntry { name: "Gym".to_string(), id: None },
                    ManagedPlaylistEntry { name: "Road Trip".to_string(), id: None },
                ],
            }
        );
    }

    #[test]
    fn managed_playlists_save_is_atomic_no_tmp_left_behind() {
        let dir = tempdir_under_target("atomic");
        let path = dir.join("managed_playlists.json");
        ManagedPlaylists { names: vec![ManagedPlaylistEntry { name: "A".to_string(), id: Some(1) }] }
            .save(&path)
            .unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
    }
}
