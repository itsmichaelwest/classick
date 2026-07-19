//! Transactional host-side playlist deletion across every remembered device.

#[path = "playlist_deletion_integration/support.rs"]
mod support;

use classick::config_file::{self, PersistedConfig};
use classick::device_config::Subscriptions;
use serde_json::json;
use support::{
    load_subscriptions, save_playlist, save_subscriptions, subscriptions_revision, test_root,
    write_mutation_journal, write_registry, JournalSubscription, Sandbox,
};

#[tokio::test]
async fn deletion_scrubs_a_and_b_preserves_unrelated_order_and_leaves_c_unchanged() {
    let sandbox = Sandbox::start(&[("RAW-A", 4), ("RAW-B", 8), ("RAW-C", 12)]).await;
    save_playlist(&sandbox.root, "gym");
    save_subscriptions(&sandbox.root, "RAW-A", &["before", "gym", "after"]);
    save_subscriptions(&sandbox.root, "RAW-B", &["gym", "other", "gym"]);
    let c_path = save_subscriptions(&sandbox.root, "RAW-C", &["other", "before"]);
    let c_before = std::fs::read(&c_path).unwrap();
    let mut client = sandbox.connect().await;

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":"delete-gym"}))
        .await;

    let mut changed_serials = Vec::new();
    loop {
        let event = client.next().await;
        match event["type"].as_str() {
            Some("device_config_update") => changed_serials.push(event["serial"].clone()),
            Some("playlists_update") if event["acknowledged_request_id"] == "delete-gym" => {
                assert_eq!(event["playlists"], json!([]));
                break;
            }
            _ => {}
        }
    }
    changed_serials.sort_by_key(|serial| serial.as_str().unwrap().to_string());

    assert_eq!(changed_serials, vec![json!("RAW-A"), json!("RAW-B")]);
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-A").playlists,
        ["before", "after"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-B").playlists,
        ["other"]
    );
    assert_eq!(std::fs::read(c_path).unwrap(), c_before);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 5);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-B"), 9);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-C"), 12);
    assert!(!sandbox.root.join("playlists/gym.m3u8").exists());
    assert!(!sandbox.root.join("devices/playlist-mutations").exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn missing_playlist_is_an_acknowledged_no_op() {
    let sandbox = Sandbox::start(&[("RAW-A", 2)]).await;
    let subscriptions_path = save_subscriptions(&sandbox.root, "RAW-A", &["ghost", "other"]);
    let before = std::fs::read(&subscriptions_path).unwrap();
    let mut client = sandbox.connect().await;

    client
        .send(json!({"type":"delete_playlist","slug":"ghost","request_id":"delete-missing"}))
        .await;

    let update = client.next_type("playlists_update").await;
    assert_eq!(update["acknowledged_request_id"], "delete-missing");
    assert_eq!(std::fs::read(subscriptions_path).unwrap(), before);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 2);
    client.assert_no_success_broadcast().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn registry_publish_failure_rolls_back_and_emits_no_success_broadcast() {
    let sandbox = Sandbox::start(&[("RAW-A", 3)]).await;
    save_playlist(&sandbox.root, "gym");
    let subscriptions_path = save_subscriptions(&sandbox.root, "RAW-A", &["gym", "other"]);
    let playlist_path = sandbox.root.join("playlists/gym.m3u8");
    let playlist_before = std::fs::read(&playlist_path).unwrap();
    let subscriptions_before = std::fs::read(&subscriptions_path).unwrap();
    let mut client = sandbox.connect().await;
    std::fs::remove_file(&sandbox.registry_path).unwrap();
    std::fs::create_dir(&sandbox.registry_path).unwrap();

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":"delete-fails"}))
        .await;

    client.assert_no_success_broadcast().await;
    assert_eq!(std::fs::read(playlist_path).unwrap(), playlist_before);
    assert_eq!(
        std::fs::read(subscriptions_path).unwrap(),
        subscriptions_before
    );
    assert!(!sandbox.root.join("devices/playlist-mutations").exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn live_playlist_mutation_journal_blocks_dependent_mutations() {
    let sandbox = Sandbox::start(&[("RAW-A", 3)]).await;
    save_playlist(&sandbox.root, "gym");
    let playlist_path = sandbox.root.join("playlists/gym.m3u8");
    let playlist_before = std::fs::read(&playlist_path).unwrap();
    let mut client = sandbox.connect().await;
    let mutation_root = sandbox.root.join("devices/playlist-mutations");
    std::fs::create_dir_all(&mutation_root).unwrap();
    std::fs::write(mutation_root.join("unresolved.json"), b"{}").unwrap();

    client
        .send(json!({
            "type":"delete_playlist",
            "slug":"gym",
            "request_id":"delete-while-recovery-pending"
        }))
        .await;

    let event = loop {
        let event = client.next().await;
        if matches!(
            event["type"].as_str(),
            Some("command_failed" | "playlists_update")
        ) {
            break event;
        }
    };
    assert_eq!(event["type"], "command_failed");
    assert_eq!(
        event["acknowledged_request_id"],
        "delete-while-recovery-pending"
    );
    assert_eq!(
        event["error"],
        "playlist mutation recovery is pending; restart Classick"
    );
    assert_eq!(std::fs::read(playlist_path).unwrap(), playlist_before);
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_rolls_forward_publishing_journal_and_next_preview_is_clean() {
    let root = test_root("recover-publishing");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("RAW-A", 6), ("RAW-B", 9)]);
    save_playlist(&root, "gym");
    save_playlist(&root, "keep-a");
    save_playlist(&root, "keep-b");
    let a_path = save_subscriptions(&root, "RAW-A", &["gym", "keep-a"]);
    let b_path = save_subscriptions(&root, "RAW-B", &["keep-b", "gym"]);
    let a_original = std::fs::read(&a_path).unwrap();
    let b_original = std::fs::read(&b_path).unwrap();
    let a_target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep-a".into()],
    })
    .unwrap();
    let b_target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep-b".into()],
    })
    .unwrap();
    let request_id = "recover-delete";
    let mutation_root = root.join("devices/playlist-mutations");
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    std::fs::create_dir_all(&stage_root).unwrap();
    let playlist_path = root.join("playlists/gym.m3u8");
    let playlist_original = std::fs::read(&playlist_path).unwrap();
    let playlist_stage = stage_root.join("playlist.original");
    std::fs::rename(&playlist_path, &playlist_stage).unwrap();
    let a_original_stage = stage_root.join("subscription-0.original");
    let a_target_stage = stage_root.join("subscription-0.target");
    let b_original_stage = stage_root.join("subscription-1.original");
    let b_target_stage = stage_root.join("subscription-1.target");
    std::fs::write(&a_original_stage, &a_original).unwrap();
    std::fs::write(&a_target_stage, &a_target).unwrap();
    std::fs::write(&b_original_stage, &b_original).unwrap();
    std::fs::write(&b_target_stage, &b_target).unwrap();
    std::fs::rename(&a_target_stage, &a_path).unwrap();
    write_mutation_journal(
        &root,
        request_id,
        "publishing",
        &playlist_path,
        &playlist_stage,
        &playlist_original,
        &[
            JournalSubscription {
                serial: "RAW-A",
                live: &a_path,
                original_stage: &a_original_stage,
                target_stage: &a_target_stage,
                original: &a_original,
                target: &a_target,
                original_revision: 6,
            },
            JournalSubscription {
                serial: "RAW-B",
                live: &b_path,
                original_stage: &b_original_stage,
                target_stage: &b_target_stage,
                original: &b_original,
                target: &b_target,
                original_revision: 9,
            },
        ],
    );

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let mut client = sandbox.connect().await;
    client
        .send(
            json!({"type":"preview_device","serial":"RAW-A","request_id":"preview-after-recovery"}),
        )
        .await;
    let preview = client.next_type("device_preview").await;

    assert_eq!(preview["acknowledged_request_id"], "preview-after-recovery");
    assert!(preview.get("unresolved_subscriptions").is_none());
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-A").playlists,
        ["keep-a"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "RAW-B").playlists,
        ["keep-b"]
    );
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 7);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-B"), 10);
    assert!(!playlist_path.exists());
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_restores_prepared_journal_without_mutating_live_state() {
    let root = test_root("recover-prepared");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("RAW-A", 11)]);
    save_playlist(&root, "gym");
    let subscriptions_path = save_subscriptions(&root, "RAW-A", &["gym", "keep"]);
    let original = std::fs::read(&subscriptions_path).unwrap();
    let target = serde_json::to_vec_pretty(&Subscriptions {
        version: 1,
        playlists: vec!["keep".into()],
    })
    .unwrap();
    let request_id = "prepared-delete";
    let mutation_root = root.join("devices/playlist-mutations");
    let stage_root = mutation_root.join(format!("{request_id}.staged"));
    std::fs::create_dir_all(&stage_root).unwrap();
    let playlist_path = root.join("playlists/gym.m3u8");
    let playlist_original = std::fs::read(&playlist_path).unwrap();
    let playlist_stage = stage_root.join("playlist.original");
    let original_stage = stage_root.join("subscription-0.original");
    let target_stage = stage_root.join("subscription-0.target");
    std::fs::write(&original_stage, &original).unwrap();
    std::fs::write(&target_stage, &target).unwrap();
    write_mutation_journal(
        &root,
        request_id,
        "prepared",
        &playlist_path,
        &playlist_stage,
        &playlist_original,
        &[JournalSubscription {
            serial: "RAW-A",
            live: &subscriptions_path,
            original_stage: &original_stage,
            target_stage: &target_stage,
            original: &original,
            target: &target,
            original_revision: 11,
        }],
    );

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let _client = sandbox.connect().await;

    assert_eq!(std::fs::read(&subscriptions_path).unwrap(), original);
    assert_eq!(std::fs::read(&playlist_path).unwrap(), playlist_original);
    assert_eq!(subscriptions_revision(&sandbox.registry_path, "RAW-A"), 11);
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}
