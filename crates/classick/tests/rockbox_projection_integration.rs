#[path = "rockbox_projection_integration/support.rs"]
mod support;

use classick::rockbox_playlist::candidate_filename;
use std::sync::atomic::Ordering;
use support::{playlist, rendered_lines, FailurePoint, Harness};

#[test]
fn manual_and_smart_projection_match_verified_apple_order_byte_for_byte() {
    let mut h = Harness::new();
    let result = h
        .sync(
            true,
            vec![
                playlist("manual", "Manual", &[0, 1]),
                playlist("smart", "Smart", &[1, 0]),
            ],
        )
        .unwrap();
    assert!(result.completed);
    for membership in &result.verified {
        let record = result.ownership.playlists[&membership.slug]
            .rockbox
            .as_ref()
            .unwrap();
        assert_eq!(
            rendered_lines(&h.read_projection(record)),
            support::normalized_paths(membership)
        );
    }
}

#[test]
fn empty_playlist_publishes_zero_bytes_and_valid_ownership_hash() {
    let mut h = Harness::new();
    let result = h.sync(true, vec![playlist("empty", "Empty", &[])]).unwrap();
    let record = result.ownership.playlists["empty"]
        .rockbox
        .as_ref()
        .unwrap();
    let bytes = h.read_projection(record);
    assert!(bytes.is_empty());
    assert_eq!(
        record.content_hash,
        blake3::hash(&bytes).to_hex().to_string()
    );
}

#[test]
fn same_display_name_and_foreign_filename_collisions_never_overwrite() {
    let mut h = Harness::new();
    let collision = candidate_filename("Mix", "mix-a", 0);
    h.write_foreign(&collision, b"foreign\n");
    let before = h.foreign_hash(&collision);
    let result = h
        .sync(
            true,
            vec![
                playlist("mix-a", "Mix", &[0]),
                playlist("mix-b", "Mix", &[1]),
            ],
        )
        .unwrap();
    let a = result.ownership.playlists["mix-a"]
        .rockbox
        .as_ref()
        .unwrap();
    let b = result.ownership.playlists["mix-b"]
        .rockbox
        .as_ref()
        .unwrap();
    assert_ne!(a.relative_filename, b.relative_filename);
    assert_ne!(a.relative_filename, collision);
    assert_eq!(h.foreign_hash(&collision), before);
}

#[test]
fn collision_choice_is_stable_across_recovery() {
    let mut h = Harness::new();
    let collision = candidate_filename("Mix", "mix", 0);
    h.write_foreign(&collision, b"foreign");
    h.fail_once(FailurePoint::BeforeOwnershipPublish);
    assert!(h.sync(true, vec![playlist("mix", "Mix", &[0])]).is_err());
    let chosen = h.journal().unwrap().pending_rockbox_ops["mix"]
        .desired
        .as_ref()
        .unwrap()
        .relative_filename
        .clone();
    let recovered = h.recover().unwrap();
    assert_eq!(
        recovered.ownership.playlists["mix"]
            .rockbox
            .as_ref()
            .unwrap()
            .relative_filename,
        chosen
    );
}

#[test]
fn rename_publishes_new_before_removing_old_and_settles_new_record() {
    let mut h = Harness::new();
    let first = h
        .sync(true, vec![playlist("stable", "Old Name", &[0])])
        .unwrap();
    let old = first.ownership.playlists["stable"].rockbox.clone().unwrap();
    h.fail_once(FailurePoint::ProjectionDelete);
    assert!(h
        .sync(true, vec![playlist("stable", "New Name", &[0])])
        .is_err());
    let pending = h.journal().unwrap();
    let new = pending.pending_rockbox_ops["stable"]
        .desired
        .clone()
        .unwrap();
    assert!(h.projection_exists(&old));
    assert!(h.projection_exists(&new));
    let recovered = h.recover().unwrap();
    assert!(!h.projection_exists(&old));
    assert_eq!(recovered.ownership.playlists["stable"].rockbox, Some(new));
}

#[test]
fn unsubscribe_removes_apple_and_exact_rockbox_but_preserves_foreign() {
    let mut h = Harness::new();
    h.write_foreign("Handmade.m3u8", b"/foreign/path.m4a\n");
    let foreign_before = h.foreign_hash("Handmade.m3u8");
    let first = h.sync(true, vec![playlist("gone", "Gone", &[0])]).unwrap();
    let old = first.ownership.playlists["gone"].rockbox.clone().unwrap();
    let second = h.sync(true, vec![]).unwrap();
    assert!(second.verified.iter().all(|p| p.slug != "gone"));
    assert!(!second.ownership.playlists.contains_key("gone"));
    assert!(!h.projection_exists(&old));
    assert_eq!(h.foreign_hash("Handmade.m3u8"), foreign_before);
}

#[test]
fn toggle_off_waits_for_ownership_checkpoint_then_removes_recorded_only() {
    let mut h = Harness::new();
    h.write_foreign("Handmade.m3u8", b"foreign");
    let before = h.foreign_hash("Handmade.m3u8");
    let first = h.sync(true, vec![playlist("keep", "Keep", &[0])]).unwrap();
    let record = first.ownership.playlists["keep"].rockbox.clone().unwrap();
    h.fail_once(FailurePoint::BeforeOwnershipPublish);
    assert!(h.sync(false, vec![playlist("keep", "Keep", &[0])]).is_err());
    assert!(h.projection_exists(&record));
    let recovered = h.recover().unwrap();
    assert!(recovered.completed);
    assert!(!h.projection_exists(&record));
    assert_eq!(h.foreign_hash("Handmade.m3u8"), before);
}

#[test]
fn failed_delete_recovery_retries_exact_old_path_without_new_apple_id() {
    let mut h = Harness::new();
    let first = h.sync(true, vec![playlist("stable", "Old", &[0])]).unwrap();
    let apple_id = first.ownership.playlists["stable"].apple_playlist_id;
    let old = first.ownership.playlists["stable"].rockbox.clone().unwrap();
    h.fail_once(FailurePoint::ProjectionDelete);
    assert!(h.sync(true, vec![playlist("stable", "New", &[0])]).is_err());
    assert!(h.journal().unwrap().pending_rockbox_ops["stable"]
        .previous
        .is_some());
    let apple_writes_before = h.apple_write_count.load(Ordering::SeqCst);
    let recovered = h.recover().unwrap();
    assert_eq!(
        recovered.ownership.playlists["stable"].apple_playlist_id,
        apple_id
    );
    assert_eq!(
        h.apple_write_count.load(Ordering::SeqCst),
        apple_writes_before
    );
    assert!(!h.projection_exists(&old));
    assert!(h.journal().is_none());
}

#[test]
fn corrupt_record_fails_closed_without_foreign_mutation() {
    for bad in ["../x.m3u8", "/x.m3u8", "a/b.m3u8", "a\\b.m3u8"] {
        let mut h = Harness::new();
        h.write_foreign("Handmade.m3u8", b"foreign");
        let before = h.foreign_hash("Handmade.m3u8");
        let invalid = serde_json::json!({
            "schema_version": 1, "device_serial": h.serial,
            "playlists": { "bad": { "apple_playlist_id": 7, "expected_kind": "normal",
                "rockbox": { "relative_filename": bad, "content_hash": "bad" }}}
        });
        h.write_raw_device_ownership(&serde_json::to_vec(&invalid).unwrap());
        assert!(h.sync(false, vec![]).is_err(), "accepted {bad:?}");
        assert_eq!(h.foreign_hash("Handmade.m3u8"), before);
    }
}

#[test]
fn corrupt_serial_and_hash_fail_closed() {
    for (serial, hash) in [("OTHER", "a".repeat(64)), ("SERIAL", "bad".into())] {
        let mut h = Harness::new();
        let invalid = serde_json::json!({
            "schema_version": 1, "device_serial": serial,
            "playlists": { "bad": { "apple_playlist_id": 7, "expected_kind": "normal",
                "rockbox": { "relative_filename": "Bad--0123456789.m3u8", "content_hash": hash }}}
        });
        h.write_raw_device_ownership(&serde_json::to_vec(&invalid).unwrap());
        assert!(h.sync(false, vec![]).is_err());
    }
}

#[test]
fn unplug_at_write_rename_and_delete_recovers_idempotently() {
    for point in [
        FailurePoint::ProjectionWrite,
        FailurePoint::ProjectionRename,
        FailurePoint::ProjectionDelete,
    ] {
        let mut h = Harness::new();
        let first = h.sync(true, vec![playlist("stable", "Old", &[0])]).unwrap();
        let apple_id = first.ownership.playlists["stable"].apple_playlist_id;
        h.fail_once(point);
        assert!(h.sync(true, vec![playlist("stable", "New", &[0])]).is_err());
        assert!(h.journal().is_some());
        let apple_writes_before = h.apple_write_count.load(Ordering::SeqCst);
        let recovered = h.recover().unwrap();
        let record = recovered.ownership.playlists["stable"]
            .rockbox
            .as_ref()
            .unwrap();
        assert_eq!(
            recovered.ownership.playlists["stable"].apple_playlist_id,
            apple_id
        );
        assert_eq!(
            h.apple_write_count.load(Ordering::SeqCst),
            apple_writes_before
        );
        assert_eq!(
            record.content_hash,
            blake3::hash(&h.read_projection(record))
                .to_hex()
                .to_string()
        );
        assert!(h.journal().is_none());
    }
}

#[test]
fn failure_injection_is_mount_scoped_and_production_default_remains_usable() {
    let mut injected = Harness::new();
    injected.fail_once(FailurePoint::ProjectionWrite);
    let mut default = Harness::new();
    assert!(
        default
            .sync(true, vec![playlist("mix", "Mix", &[0])])
            .unwrap()
            .completed
    );
    assert!(injected
        .sync(true, vec![playlist("mix", "Mix", &[0])])
        .is_err());
}

#[test]
fn identical_second_run_does_not_rewrite_projection_or_serialize_host_paths() {
    let mut h = Harness::new();
    let desired = vec![playlist("mix", "Mix", &[0, 1])];
    let first = h.sync(true, desired.clone()).unwrap();
    let record = first.ownership.playlists["mix"].rockbox.clone().unwrap();
    let before = std::fs::metadata(h.projection_path(&record))
        .unwrap()
        .modified()
        .unwrap();
    h.fail_once(FailurePoint::ProjectionWrite);
    let second = h.sync(true, desired).unwrap();
    assert_eq!(
        std::fs::metadata(h.projection_path(&record))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    let bytes = h.read_projection(&record);
    assert!(!String::from_utf8_lossy(&bytes).contains(&h.root.display().to_string()));
    assert_eq!(first.verified, second.verified);
}

#[cfg(unix)]
#[test]
fn symlink_swap_after_staging_cannot_escape_managed_root() {
    let mut h = Harness::new();
    h.fail_once(FailurePoint::BeforeOwnershipPublish);
    assert!(h.sync(true, vec![playlist("mix", "Mix", &[0])]).is_err());
    let outside = h.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    h.replace_managed_root_with_symlink(&outside);
    assert!(h.recover().is_err());
    assert!(h.journal().is_some());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn managed_root_swap_between_validation_and_mutation_retains_journal() {
    let mut h = Harness::new();
    let outside = h.root.join("outside-race");
    std::fs::create_dir_all(&outside).unwrap();
    h.swap_managed_root_before_projection_mutation(&outside);

    assert!(h.sync(true, vec![playlist("mix", "Mix", &[0])]).is_err());

    assert!(h.journal().is_some());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}
