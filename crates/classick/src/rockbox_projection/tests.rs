use super::*;
use crate::ipod::playlist_ownership::{
    ManagedPlaylistEntry, ManagedPlaylistKind, RockboxProjectionRecord,
    MANAGED_PLAYLIST_OWNERSHIP_VERSION,
};
use crate::rockbox_playlist::{candidate_filename, render_verified_paths};
use crate::rockbox_projection_fs::TargetState;
use anyhow::Result;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct MemoryIo {
    states: HashMap<String, TargetState>,
    content_matches: HashMap<String, bool>,
    probes: Cell<usize>,
    probed_authority: RefCell<Vec<HashSet<String>>>,
}

impl MemoryIo {
    fn foreign(name: &str) -> Self {
        Self {
            states: HashMap::from([(name.to_string(), TargetState::ForeignFile)]),
            ..Self::default()
        }
    }

    fn recorded(records: impl IntoIterator<Item = RockboxProjectionRecord>) -> Self {
        let records = records.into_iter().collect::<Vec<_>>();
        Self {
            states: records
                .iter()
                .cloned()
                .map(|record| (record.relative_filename, TargetState::RecordedFile))
                .collect(),
            content_matches: records
                .into_iter()
                .map(|record| (record.relative_filename, true))
                .collect(),
            ..Self::default()
        }
    }
}

impl ProjectionIo for MemoryIo {
    fn target_state(&self, name: &str, authorized: &HashSet<String>) -> Result<TargetState> {
        self.probes.set(self.probes.get() + 1);
        self.probed_authority.borrow_mut().push(authorized.clone());
        Ok(self
            .states
            .get(name)
            .copied()
            .unwrap_or(TargetState::Missing))
    }

    fn write_durable(
        &self,
        _name: &str,
        _bytes: &[u8],
        _authorized: &HashSet<String>,
        _replace_recorded: bool,
    ) -> Result<()> {
        panic!("pure planner must not write")
    }

    fn remove_recorded(
        &self,
        _name: &str,
        _expected_hash: &str,
        _authorized: &HashSet<String>,
    ) -> Result<bool> {
        panic!("pure planner must not delete")
    }

    fn content_matches(
        &self,
        name: &str,
        _expected_hash: &str,
        authorized: &HashSet<String>,
    ) -> Result<bool> {
        assert!(authorized.contains(name));
        Ok(self.content_matches.get(name).copied().unwrap_or(false))
    }
}

fn record(name: String, hash_byte: char) -> RockboxProjectionRecord {
    RockboxProjectionRecord {
        relative_filename: name,
        content_hash: hash_byte.to_string().repeat(64),
    }
}

fn ownership(
    serial: &str,
    entries: impl IntoIterator<Item = (String, u64)>,
) -> ManagedPlaylistOwnership {
    ManagedPlaylistOwnership {
        schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
        device_serial: serial.to_string(),
        playlists: entries
            .into_iter()
            .map(|(slug, id)| {
                (
                    slug,
                    ManagedPlaylistEntry {
                        apple_playlist_id: id,
                        expected_kind: ManagedPlaylistKind::Normal,
                        rockbox: None,
                    },
                )
            })
            .collect(),
    }
}

fn desired(name: &str, slug: &str, id: u64, paths: &[&str]) -> DesiredVerifiedPlaylist {
    DesiredVerifiedPlaylist {
        display_name: name.to_string(),
        membership: VerifiedPlaylistMembership {
            slug: slug.to_string(),
            apple_playlist_id: id,
            ordered_dbids: (1..=paths.len() as u64).collect(),
            ordered_ipod_paths: paths.iter().map(|path| (*path).to_string()).collect(),
        },
    }
}

#[test]
fn plans_same_order_and_hash_as_verified_membership() {
    let desired = vec![
        desired(
            "Smart",
            "smart",
            2,
            &[
                "iPod_Control/Music/F01/B.m4a",
                "iPod_Control/Music/F00/A.m4a",
            ],
        ),
        desired(
            "Manual",
            "manual",
            1,
            &[
                "iPod_Control/Music/F00/A.m4a",
                "iPod_Control/Music/F01/B.m4a",
            ],
        ),
    ];
    let settled = ownership("SERIAL", []);
    let candidate = ownership("SERIAL", [("smart".into(), 2), ("manual".into(), 1)]);
    let plan = plan_projection(
        "SERIAL",
        true,
        &desired,
        &settled,
        &candidate,
        &MemoryIo::default(),
    )
    .unwrap();

    assert_eq!(
        plan.operations
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["manual", "smart"]
    );
    let membership = &desired[1].membership;
    assert_eq!(
        render_verified_paths(membership).unwrap(),
        b"/iPod_Control/Music/F00/A.m4a\n/iPod_Control/Music/F01/B.m4a\n"
    );
    let operation = &plan.operations["manual"];
    assert_eq!(
        operation.desired.as_ref().unwrap().content_hash,
        blake3::hash(&render_verified_paths(membership).unwrap())
            .to_hex()
            .to_string()
    );
}

#[test]
fn foreign_collision_uses_next_attempt_without_claiming_it() {
    let desired = vec![desired("Road Trip", "road-trip", 1, &[])];
    let first = candidate_filename("Road Trip", "road-trip", 0);
    let settled = ownership("SERIAL", []);
    let candidate = ownership("SERIAL", [("road-trip".into(), 1)]);
    let io = MemoryIo::foreign(&first);

    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();

    assert_eq!(
        plan.operations["road-trip"]
            .desired
            .as_ref()
            .unwrap()
            .relative_filename,
        candidate_filename("Road Trip", "road-trip", 1)
    );
    assert!(io
        .probed_authority
        .borrow()
        .iter()
        .all(|set| !set.contains(&first)));
}

#[test]
fn another_slugs_recorded_name_is_a_collision_even_when_io_calls_it_recorded() {
    let desired = vec![desired("Mix", "new", 2, &[])];
    let first = candidate_filename("Mix", "new", 0);
    let mut settled = ownership("SERIAL", [("old".into(), 1)]);
    settled.playlists.get_mut("old").unwrap().rockbox = Some(record(first.clone(), 'a'));
    let candidate = ownership("SERIAL", [("new".into(), 2)]);
    let io = MemoryIo::recorded([record(first.clone(), 'a')]);

    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();

    assert_eq!(
        plan.operations["new"]
            .desired
            .as_ref()
            .unwrap()
            .relative_filename,
        candidate_filename("Mix", "new", 1)
    );
    assert_eq!(
        plan.operations["old"]
            .previous
            .as_ref()
            .unwrap()
            .relative_filename,
        first
    );
}

#[test]
fn unchanged_name_and_hash_reuses_same_slug_and_can_be_zero_op() {
    let desired = vec![desired("Mix", "mix", 1, &[])];
    let filename = candidate_filename("Mix", "mix", 0);
    let exact = record(filename, 'a');
    let mut settled = ownership("SERIAL", [("mix".into(), 1)]);
    settled.playlists.get_mut("mix").unwrap().rockbox = Some(exact.clone());
    let mut candidate = settled.clone();
    candidate.playlists.get_mut("mix").unwrap().rockbox = None;
    let exact_hash = blake3::hash(b"").to_hex().to_string();
    settled
        .playlists
        .get_mut("mix")
        .unwrap()
        .rockbox
        .as_mut()
        .unwrap()
        .content_hash = exact_hash.clone();
    let io = MemoryIo::recorded([settled.playlists["mix"].rockbox.clone().unwrap()]);

    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();

    assert!(plan.operations.is_empty());
    assert_eq!(
        plan.candidate_ownership.playlists["mix"]
            .rockbox
            .as_ref()
            .unwrap()
            .content_hash,
        exact_hash
    );
}

#[test]
fn unchanged_record_with_modified_bytes_stages_repair_without_writing() {
    let desired = vec![desired("Mix", "mix", 1, &[])];
    let filename = candidate_filename("Mix", "mix", 0);
    let exact_hash = blake3::hash(b"").to_hex().to_string();
    let exact = RockboxProjectionRecord {
        relative_filename: filename.clone(),
        content_hash: exact_hash,
    };
    let mut settled = ownership("SERIAL", [("mix".into(), 1)]);
    settled.playlists.get_mut("mix").unwrap().rockbox = Some(exact.clone());
    let mut candidate = settled.clone();
    candidate.playlists.get_mut("mix").unwrap().rockbox = None;
    let mut io = MemoryIo::recorded([exact.clone()]);
    io.content_matches.insert(filename, false);

    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();

    let operation = &plan.operations["mix"];
    assert_eq!(operation.previous, Some(exact));
    assert_eq!(
        operation.desired.as_ref().unwrap().relative_filename,
        candidate_filename("Mix", "mix", 1)
    );
}

#[test]
fn rename_update_unsubscribe_and_toggle_off_have_exact_pairs() {
    let desired = vec![
        desired("New Name", "rename", 1, &[]),
        desired("Update", "update", 2, &["iPod_Control/Music/F00/A.m4a"]),
    ];
    let mut settled = ownership(
        "SERIAL",
        [
            ("rename".into(), 1),
            ("update".into(), 2),
            ("gone".into(), 3),
        ],
    );
    let rename_old = record(candidate_filename("Old Name", "rename", 0), 'a');
    let update_old = record(candidate_filename("Update", "update", 0), 'b');
    let gone_old = record(candidate_filename("Gone", "gone", 0), 'c');
    settled.playlists.get_mut("rename").unwrap().rockbox = Some(rename_old.clone());
    settled.playlists.get_mut("update").unwrap().rockbox = Some(update_old.clone());
    settled.playlists.get_mut("gone").unwrap().rockbox = Some(gone_old.clone());
    let candidate = ownership("SERIAL", [("rename".into(), 1), ("update".into(), 2)]);
    let mut io = MemoryIo::recorded([rename_old.clone(), update_old.clone(), gone_old.clone()]);
    io.content_matches
        .insert(update_old.relative_filename.clone(), false);

    let plan = plan_projection("SERIAL", true, &desired, &settled, &candidate, &io).unwrap();
    assert_eq!(plan.operations["rename"].previous, Some(rename_old.clone()));
    assert_ne!(
        plan.operations["rename"]
            .desired
            .as_ref()
            .unwrap()
            .relative_filename,
        rename_old.relative_filename
    );
    assert_eq!(plan.operations["update"].previous, Some(update_old.clone()));
    assert_ne!(
        plan.operations["update"]
            .desired
            .as_ref()
            .unwrap()
            .relative_filename,
        update_old.relative_filename
    );
    assert_eq!(
        plan.operations["gone"],
        PendingRockboxOp {
            previous: Some(gone_old.clone()),
            desired: None
        }
    );

    let off = plan_projection(
        "SERIAL",
        false,
        &[],
        &settled,
        &ownership("SERIAL", [("rename".into(), 1), ("update".into(), 2)]),
        &io,
    )
    .unwrap();
    assert_eq!(off.operations.len(), 3);
    assert!(off
        .operations
        .values()
        .all(|op| op.previous.is_some() && op.desired.is_none()));
    assert!(off
        .candidate_ownership
        .playlists
        .values()
        .all(|entry| entry.rockbox.is_none()));
}

#[test]
fn candidate_shape_is_preserved_and_only_rockbox_changes() {
    let desired = vec![desired("B", "b", 22, &[]), desired("A", "a", 11, &[])];
    let settled = ownership("SERIAL", []);
    let candidate = ownership("SERIAL", [("b".into(), 22), ("a".into(), 11)]);

    let plan = plan_projection(
        "SERIAL",
        true,
        &desired,
        &settled,
        &candidate,
        &MemoryIo::default(),
    )
    .unwrap();

    assert_eq!(
        plan.candidate_ownership.schema_version,
        candidate.schema_version
    );
    assert_eq!(
        plan.candidate_ownership.device_serial,
        candidate.device_serial
    );
    assert_eq!(
        plan.candidate_ownership
            .playlists
            .keys()
            .collect::<Vec<_>>(),
        candidate.playlists.keys().collect::<Vec<_>>()
    );
    for slug in candidate.playlists.keys() {
        assert_eq!(
            plan.candidate_ownership.playlists[slug].apple_playlist_id,
            candidate.playlists[slug].apple_playlist_id
        );
        assert_eq!(
            plan.candidate_ownership.playlists[slug].expected_kind,
            candidate.playlists[slug].expected_kind
        );
    }
}

#[test]
fn serial_schema_apple_id_missing_candidate_and_duplicate_slug_fail_closed() {
    let base_desired = desired("Mix", "mix", 1, &[]);
    for mutate in 0..5 {
        let settled = ownership(if mutate == 0 { "OTHER" } else { "SERIAL" }, []);
        let mut candidate = ownership("SERIAL", [("mix".into(), 1)]);
        let mut desired = vec![base_desired.clone()];
        match mutate {
            1 => candidate.schema_version += 1,
            2 => {
                candidate
                    .playlists
                    .get_mut("mix")
                    .unwrap()
                    .apple_playlist_id = 2
            }
            3 => {
                candidate.playlists.remove("mix");
            }
            4 => desired.push(base_desired.clone()),
            _ => {}
        }
        assert!(plan_projection(
            "SERIAL",
            true,
            &desired,
            &settled,
            &candidate,
            &MemoryIo::default()
        )
        .is_err());
    }
}

#[test]
fn malformed_records_and_paths_fail_before_collision_probes() {
    let mut settled = ownership("SERIAL", [("mix".into(), 1)]);
    settled.playlists.get_mut("mix").unwrap().rockbox = Some(RockboxProjectionRecord {
        relative_filename: "../bad.m3u8".into(),
        content_hash: "A".repeat(64),
    });
    let candidate = ownership("SERIAL", [("mix".into(), 1)]);
    let io = MemoryIo::default();
    assert!(plan_projection(
        "SERIAL",
        true,
        &[desired("Mix", "mix", 1, &[])],
        &settled,
        &candidate,
        &io
    )
    .is_err());
    assert_eq!(io.probes.get(), 0);

    let settled = ownership("SERIAL", []);
    let io = MemoryIo::default();
    assert!(plan_projection(
        "SERIAL",
        true,
        &[desired("Mix", "mix", 1, &["/Users/me/a.flac"])],
        &settled,
        &candidate,
        &io
    )
    .is_err());
    assert_eq!(io.probes.get(), 0);
}

#[test]
fn all_256_foreign_attempts_fail_without_a_write() {
    let desired = vec![desired("Mix", "mix", 1, &[])];
    let states = (0..256)
        .map(|index| {
            (
                candidate_filename("Mix", "mix", index),
                TargetState::ForeignFile,
            )
        })
        .collect();
    let io = MemoryIo {
        states,
        ..MemoryIo::default()
    };
    let result = plan_projection(
        "SERIAL",
        true,
        &desired,
        &ownership("SERIAL", []),
        &ownership("SERIAL", [("mix".into(), 1)]),
        &io,
    );
    assert!(result.is_err());
    assert_eq!(io.probes.get(), 256);
}
