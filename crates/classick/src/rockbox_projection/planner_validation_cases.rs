use super::*;

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
