use super::*;
use crate::atomic_file::AtomicFileWriter;
use crate::ipod::playlist_ownership::{
    ManagedPlaylistEntry, ManagedPlaylistKind, RockboxProjectionRecord,
    MANAGED_PLAYLIST_OWNERSHIP_VERSION,
};
use crate::pending_session::{PendingRockboxOp, PendingSession};
use anyhow::{bail, Result};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn temp_mount(label: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "rockbox-publication-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn record(name: &str, bytes: &[u8]) -> RockboxProjectionRecord {
    RockboxProjectionRecord {
        relative_filename: name.to_string(),
        content_hash: blake3::hash(bytes).to_hex().to_string(),
    }
}

fn ownership(record: Option<RockboxProjectionRecord>) -> ManagedPlaylistOwnership {
    ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: "SERIAL".into(),
        playlists: BTreeMap::from([(
            "stable".into(),
            ManagedPlaylistEntry {
                apple_playlist_id: 41,
                expected_kind: ManagedPlaylistKind::Normal,
                rockbox: record,
            },
        )]),
    }
}

fn verified(bytes_path: &str) -> BTreeMap<String, VerifiedPlaylistMembership> {
    BTreeMap::from([(
        "stable".into(),
        VerifiedPlaylistMembership {
            slug: "stable".into(),
            apple_playlist_id: 41,
            ordered_dbids: vec![7],
            ordered_ipod_paths: vec![bytes_path.into()],
        },
    )])
}

#[derive(Default)]
struct RecordingIo {
    files: RefCell<HashMap<String, Vec<u8>>>,
    events: RefCell<Vec<String>>,
    fail_delete_once: Cell<bool>,
}

impl ProjectionIo for RecordingIo {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState> {
        if !authorized.contains(name) {
            bail!("unauthorized target")
        }
        Ok(if self.files.borrow().contains_key(name) {
            TargetState::RecordedFile
        } else {
            TargetState::Missing
        })
    }

    fn write_durable(
        &self,
        name: &str,
        bytes: &[u8],
        authorized: &HashSet<String>,
        replace_recorded: bool,
    ) -> Result<()> {
        if !authorized.contains(name) {
            bail!("unauthorized write")
        }
        self.events
            .borrow_mut()
            .push(format!("write:{name}:{replace_recorded}"));
        if !replace_recorded && self.files.borrow().contains_key(name) {
            bail!("no-replace destination already exists")
        }
        self.files
            .borrow_mut()
            .insert(name.to_string(), bytes.to_vec());
        Ok(())
    }

    fn remove_recorded(&self, name: &str, authorized: &HashSet<String>) -> Result<bool> {
        if !authorized.contains(name) {
            bail!("unauthorized delete")
        }
        self.events.borrow_mut().push(format!("delete:{name}"));
        if self.fail_delete_once.replace(false) {
            bail!("injected delete failure")
        }
        Ok(self.files.borrow_mut().remove(name).is_some())
    }

    fn content_matches(
        &self,
        name: &str,
        expected_hash: &str,
        authorized: &HashSet<String>,
    ) -> Result<bool> {
        if !authorized.contains(name) {
            bail!("unauthorized read")
        }
        Ok(self
            .files
            .borrow()
            .get(name)
            .is_some_and(|bytes| blake3::hash(bytes).to_hex().as_str() == expected_hash))
    }
}

fn prepared_rename(
    fail_delete_once: bool,
) -> (
    PendingSessionStore,
    PendingSession,
    DeviceOwnershipStore,
    RecordingIo,
    BTreeMap<String, VerifiedPlaylistMembership>,
) {
    let mount = temp_mount("rename");
    let store = PendingSessionStore::new(&mount);
    let bytes_path = "iPod_Control/Music/F00/A.m4a";
    let bytes = b"/iPod_Control/Music/F00/A.m4a\n";
    let old = record("Old--0123456789.m3u8", b"old\n");
    let new = record("New--0123456789.m3u8", bytes);
    let candidate = ownership(Some(new.clone()));
    let ownership_store = DeviceOwnershipStore::new(
        mount,
        "SERIAL".into(),
        temp_mount("host").join("managed.json"),
        AtomicFileWriter::new(),
    );
    ownership_store.publish_device(&candidate).unwrap();
    let mut journal = PendingSession::new(7, "SERIAL", Vec::new());
    journal.phase = PendingPhase::PlaylistOwnershipPublished;
    journal.candidate_playlist_ownership = Some(candidate);
    journal
        .desired_playlist_memberships
        .insert("stable".into(), vec![7]);
    journal.verified_playlist_memberships = verified(bytes_path).into_values().collect();
    journal.pending_rockbox_ops.insert(
        "stable".into(),
        PendingRockboxOp {
            previous: Some(old.clone()),
            desired: Some(new),
        },
    );
    journal.rockbox_projection_plan_version = Some(ROCKBOX_PROJECTION_PLAN_VERSION);
    store.save(&journal).unwrap();
    let io = RecordingIo::default();
    io.files
        .borrow_mut()
        .insert(old.relative_filename, b"old\n".to_vec());
    io.fail_delete_once.set(fail_delete_once);
    (store, journal, ownership_store, io, verified(bytes_path))
}

#[test]
fn zero_operation_plan_is_durable_and_not_legacy_ambiguous() {
    let mount = temp_mount("stage");
    let store = PendingSessionStore::new(&mount);
    let mut journal = PendingSession::new(1, "SERIAL", Vec::new());
    journal.phase = PendingPhase::DeviceManifestPublished;
    journal.candidate_playlist_ownership = Some(ownership(None));
    journal
        .desired_playlist_memberships
        .insert("stable".into(), vec![7]);
    journal.verified_playlist_memberships = verified("iPod_Control/Music/F00/A.m4a")
        .into_values()
        .collect();

    stage_playlist_projection(
        &store,
        &mut journal,
        ProjectionPlan {
            candidate_ownership: ownership(None),
            operations: BTreeMap::new(),
        },
    )
    .unwrap();

    let loaded = store.load(1).unwrap();
    assert_eq!(loaded.phase, PendingPhase::RockboxProjectionsPrepared);
    assert_eq!(
        loaded.rockbox_projection_plan_version,
        Some(ROCKBOX_PROJECTION_PLAN_VERSION)
    );
    assert!(loaded.pending_rockbox_ops.is_empty());
}

#[test]
fn rename_writes_new_before_deleting_old() {
    let (store, mut journal, ownership, io, verified) = prepared_rename(false);

    publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();

    assert_eq!(
        io.events.borrow().as_slice(),
        [
            "write:New--0123456789.m3u8:false",
            "delete:Old--0123456789.m3u8"
        ]
    );
    assert_eq!(journal.phase, PendingPhase::RockboxProjectionsPublished);
}

#[test]
fn rename_destination_that_appears_after_planning_is_not_replaced() {
    let (store, mut journal, ownership, io, verified) = prepared_rename(false);
    let desired_name = journal.pending_rockbox_ops["stable"]
        .desired
        .as_ref()
        .unwrap()
        .relative_filename
        .clone();
    io.files
        .borrow_mut()
        .insert(desired_name.clone(), b"raced foreign".to_vec());

    assert!(
        publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).is_err()
    );

    assert_eq!(io.files.borrow()[&desired_name], b"raced foreign");
    assert!(io.files.borrow().contains_key("Old--0123456789.m3u8"));
    assert_eq!(journal.phase, PendingPhase::PlaylistOwnershipPublished);
    assert!(store.load(journal.session_id).is_ok());
}

#[test]
fn failed_old_delete_retries_without_rewriting_verified_new_file() {
    let (store, mut journal, ownership, io, verified) = prepared_rename(true);

    assert!(
        publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).is_err()
    );
    assert_eq!(journal.phase, PendingPhase::PlaylistOwnershipPublished);
    publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();

    assert_eq!(
        io.events
            .borrow()
            .iter()
            .filter(|event| event.starts_with("write:"))
            .count(),
        1
    );
    assert_eq!(
        io.events
            .borrow()
            .iter()
            .filter(|event| event.starts_with("delete:"))
            .count(),
        2
    );
}

#[test]
fn same_name_missing_target_uses_no_replace() {
    let (store, mut journal, ownership, io, verified) = prepared_rename(false);
    let desired = journal.pending_rockbox_ops["stable"]
        .desired
        .clone()
        .unwrap();
    journal
        .pending_rockbox_ops
        .get_mut("stable")
        .unwrap()
        .previous = Some(desired);
    io.files.borrow_mut().clear();
    store.save(&journal).unwrap();

    publish_playlist_finalization(&store, &mut journal, &ownership, &io, &verified).unwrap();

    assert_eq!(
        io.events.borrow().as_slice(),
        ["write:New--0123456789.m3u8:false"]
    );
}

#[test]
fn disabled_projection_executes_recorded_delete_before_completion() {
    let (store, mut journal, ownership_store, io, _verified) = prepared_rename(false);
    let old = journal.pending_rockbox_ops["stable"]
        .previous
        .clone()
        .unwrap();
    let candidate = ownership(None);
    ownership_store.publish_device(&candidate).unwrap();
    journal.candidate_playlist_ownership = Some(candidate);
    journal.pending_rockbox_ops.insert(
        "stable".into(),
        PendingRockboxOp {
            previous: Some(old),
            desired: None,
        },
    );
    store.save(&journal).unwrap();

    publish_playlist_finalization(
        &store,
        &mut journal,
        &ownership_store,
        &io,
        &BTreeMap::new(),
    )
    .unwrap();

    assert_eq!(
        io.events.borrow().as_slice(),
        ["delete:Old--0123456789.m3u8"]
    );
    assert_eq!(journal.phase, PendingPhase::RockboxProjectionsPublished);
}

#[test]
fn legacy_prepared_journal_without_plan_marker_fails_closed() {
    let mut journal = PendingSession::new(9, "SERIAL", Vec::new());
    journal.phase = PendingPhase::RockboxProjectionsPrepared;
    journal.candidate_playlist_ownership = Some(ownership(None));
    journal
        .desired_playlist_memberships
        .insert("stable".into(), vec![7]);
    journal.verified_playlist_memberships = verified("iPod_Control/Music/F00/A.m4a")
        .into_values()
        .collect();

    assert!(journal.validate().is_err());
}
