//! Transactional host-side playlist deletion across every remembered device.

#[path = "playlist_deletion_integration/support.rs"]
mod support;

use classick::config_file::{self, PersistedConfig};
use classick::device::DeviceId;
use classick::device_config::Subscriptions;
use classick::portable::host_cache::HostCache;
use classick::portable::outbox::{PendingDeviceOutbox, PendingMutation, OUTBOX_SCHEMA_VERSION};
use classick::portable::profile::{
    MutationId, PlaylistSlug, SelectionMode, SelectionValue, SettingsValue, SubscriptionsValue,
    TranscodeProfile,
};
use classick::portable::state_store::PortableStateStore;
use serde_json::json;
use support::{
    load_subscriptions, save_playlist, save_subscriptions, subscriptions_revision, test_root,
    write_mutation_journal, write_registry, JournalSubscription, Sandbox,
};

const DELETE_GYM: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8200";
const DELETE_MISSING: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8201";
const DELETE_FAILS: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8202";
const DELETE_PENDING: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8203";
const PREVIEW_AFTER_RECOVERY: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8204";
const PREVIEW_AFTER_RECONCILE: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8205";
const PREVIEW_CORRUPT_PLAYLIST: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8206";

#[tokio::test]
async fn deletion_scrubs_a_and_b_preserves_unrelated_order_and_leaves_c_unchanged() {
    let sandbox = Sandbox::start(&[
        ("000A27002138B0A8", 4),
        ("000A27002138B0B9", 8),
        ("000A27002138B0CA", 12),
    ])
    .await;
    let mut client = sandbox.connect().await;
    save_playlist(&sandbox.root, "gym");
    save_subscriptions(
        &sandbox.root,
        "000A27002138B0A8",
        &["before", "gym", "after"],
    );
    save_subscriptions(&sandbox.root, "000A27002138B0B9", &["gym", "other", "gym"]);
    let c_path = save_subscriptions(&sandbox.root, "000A27002138B0CA", &["other", "before"]);
    let c_before = std::fs::read(&c_path).unwrap();

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":DELETE_GYM}))
        .await;

    let mut changed_serials = Vec::new();
    loop {
        let event = client.next().await;
        match event["type"].as_str() {
            Some("device_config") => changed_serials.push(event["device_id"].clone()),
            Some("playlists") if event["request_id"] == DELETE_GYM => {
                assert_eq!(event["playlists"], json!([]));
                break;
            }
            _ => {}
        }
    }
    changed_serials.sort_by_key(|serial| serial.as_str().unwrap().to_string());

    assert_eq!(
        changed_serials,
        vec![json!("000A27002138B0A8"), json!("000A27002138B0B9")]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "000A27002138B0A8").playlists,
        ["before", "after"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "000A27002138B0B9").playlists,
        ["other"]
    );
    assert_eq!(std::fs::read(c_path).unwrap(), c_before);
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        5
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0B9"),
        9
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0CA"),
        12
    );
    assert!(!sandbox.root.join("playlists/gym.m3u8").exists());
    assert!(!sandbox.root.join("devices/playlist-mutations").exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn missing_playlist_is_an_acknowledged_no_op() {
    let sandbox = Sandbox::start(&[("000A27002138B0A8", 2)]).await;
    let mut client = sandbox.connect().await;
    let subscriptions_path =
        save_subscriptions(&sandbox.root, "000A27002138B0A8", &["ghost", "other"]);
    let before = std::fs::read(&subscriptions_path).unwrap();

    client
        .send(json!({"type":"delete_playlist","slug":"ghost","request_id":DELETE_MISSING}))
        .await;

    let update = client.next_type("playlists").await;
    assert_eq!(update["request_id"], DELETE_MISSING);
    assert_eq!(std::fs::read(subscriptions_path).unwrap(), before);
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        2
    );
    client.assert_no_success_broadcast().await;
    sandbox.shutdown().await;
}

#[tokio::test]
async fn registry_publish_failure_rolls_back_and_emits_no_success_broadcast() {
    let sandbox = Sandbox::start(&[("000A27002138B0A8", 3)]).await;
    let mut client = sandbox.connect().await;
    save_playlist(&sandbox.root, "gym");
    let subscriptions_path =
        save_subscriptions(&sandbox.root, "000A27002138B0A8", &["gym", "other"]);
    let playlist_path = sandbox.root.join("playlists/gym.m3u8");
    let playlist_before = std::fs::read(&playlist_path).unwrap();
    let subscriptions_before = std::fs::read(&subscriptions_path).unwrap();
    std::fs::remove_file(&sandbox.registry_path).unwrap();
    std::fs::create_dir(&sandbox.registry_path).unwrap();

    client
        .send(json!({"type":"delete_playlist","slug":"gym","request_id":DELETE_FAILS}))
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
    let sandbox = Sandbox::start(&[("000A27002138B0A8", 3)]).await;
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
            "request_id":DELETE_PENDING
        }))
        .await;

    let event = loop {
        let event = client.next().await;
        if matches!(event["type"].as_str(), Some("command_failed" | "playlists")) {
            break event;
        }
    };
    assert_eq!(event["type"], "command_failed");
    assert_eq!(event["request_id"], DELETE_PENDING);
    assert_eq!(
        event["message"],
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
    let registry_path = write_registry(
        &config_path,
        &[("000A27002138B0A8", 6), ("000A27002138B0B9", 9)],
    );
    save_playlist(&root, "gym");
    save_playlist(&root, "keep-a");
    save_playlist(&root, "keep-b");
    let a_path = save_subscriptions(&root, "000A27002138B0A8", &["gym", "keep-a"]);
    let b_path = save_subscriptions(&root, "000A27002138B0B9", &["keep-b", "gym"]);
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
                serial: "000A27002138B0A8",
                live: &a_path,
                original_stage: &a_original_stage,
                target_stage: &a_target_stage,
                original: &a_original,
                target: &a_target,
                original_revision: 6,
            },
            JournalSubscription {
                serial: "000A27002138B0B9",
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
            json!({"type":"preview_device","device_id":"000A27002138B0A8","request_id":PREVIEW_AFTER_RECOVERY}),
        )
        .await;
    let preview = client.next_type("device_preview").await;

    assert_eq!(preview["request_id"], PREVIEW_AFTER_RECOVERY);
    assert_eq!(preview["unresolved_subscriptions"], json!([]));
    assert_eq!(
        load_subscriptions(&sandbox.root, "000A27002138B0A8").playlists,
        ["keep-a"]
    );
    assert_eq!(
        load_subscriptions(&sandbox.root, "000A27002138B0B9").playlists,
        ["keep-b"]
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        7
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0B9"),
        10
    );
    assert!(!playlist_path.exists());
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_removes_subscriptions_to_missing_playlists() {
    let root = test_root("reconcile-missing-subscriptions");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("000A27002138B0A8", 6)]);
    save_playlist(&root, "keep");
    save_subscriptions(&root, "000A27002138B0A8", &["missing", "keep"]);

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let mut client = sandbox.connect().await;
    client
        .send(
            json!({"type":"preview_device","device_id":"000A27002138B0A8","request_id":PREVIEW_AFTER_RECONCILE}),
        )
        .await;
    let preview = client.next_type("device_preview").await;

    assert_eq!(preview["request_id"], PREVIEW_AFTER_RECONCILE);
    assert_eq!(preview["unresolved_subscriptions"], json!([]));
    assert_eq!(
        load_subscriptions(&sandbox.root, "000A27002138B0A8").playlists,
        ["keep"]
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        7
    );
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_preserves_subscription_when_playlist_exists_but_cannot_be_loaded() {
    let root = test_root("preserve-corrupt-playlist-subscription");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("000A27002138B0A8", 6)]);
    let playlists_root = root.join("playlists");
    std::fs::create_dir_all(&playlists_root).unwrap();
    std::fs::write(playlists_root.join("corrupt.m3u8"), [0xff, 0xfe]).unwrap();
    let subscriptions_path = save_subscriptions(&root, "000A27002138B0A8", &["corrupt"]);
    let subscriptions_before = std::fs::read(&subscriptions_path).unwrap();

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let mut client = sandbox.connect().await;
    client
        .send(
            json!({"type":"preview_device","device_id":"000A27002138B0A8","request_id":PREVIEW_CORRUPT_PLAYLIST}),
        )
        .await;
    let preview = client.next_type("device_preview").await;

    assert_eq!(preview["request_id"], PREVIEW_CORRUPT_PLAYLIST);
    assert_eq!(preview["unresolved_subscriptions"], json!(["corrupt"]));
    assert_eq!(
        std::fs::read(subscriptions_path).unwrap(),
        subscriptions_before
    );
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        6
    );
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_removes_missing_subscriptions_from_portable_host_intent() {
    let root = test_root("reconcile-portable-subscriptions");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("000A27002138B0A8", 6)]);
    save_playlist(&root, "keep");
    initialize_portable_subscriptions(&root, "000A27002138B0A8", &["missing", "keep"]);

    let sandbox = Sandbox::start_from_existing(root, config_path, registry_path).await;
    let _client = sandbox.connect().await;
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let state = PortableStateStore::new(&sandbox.root)
        .load(&device_id)
        .unwrap();
    let snapshot = classick::portable::coordinator::config_snapshot(&state, None).unwrap();

    assert_eq!(
        snapshot.subscriptions.value.playlists,
        [PlaylistSlug::parse("keep").unwrap()]
    );
    sandbox.shutdown().await;
}

#[tokio::test]
async fn startup_restores_prepared_journal_without_mutating_live_state() {
    let root = test_root("recover-prepared");
    let config_path = root.join("config.toml");
    config_file::save(&config_path, &PersistedConfig::default()).unwrap();
    let registry_path = write_registry(&config_path, &[("000A27002138B0A8", 11)]);
    save_playlist(&root, "gym");
    save_playlist(&root, "keep");
    let subscriptions_path = save_subscriptions(&root, "000A27002138B0A8", &["gym", "keep"]);
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
            serial: "000A27002138B0A8",
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
    assert_eq!(
        subscriptions_revision(&sandbox.registry_path, "000A27002138B0A8"),
        11
    );
    assert!(!mutation_root.join(format!("{request_id}.json")).exists());
    assert!(!stage_root.exists());
    sandbox.shutdown().await;
}

fn initialize_portable_subscriptions(root: &std::path::Path, serial: &str, playlists: &[&str]) {
    let device_id = DeviceId::parse(serial).unwrap();
    let mutations = vec![
        PendingMutation::selection(
            MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8300").unwrap(),
            device_id.clone(),
            SelectionValue {
                schema_version: 1,
                mode: SelectionMode::All,
                rules: Vec::new(),
            },
            0,
        )
        .unwrap(),
        PendingMutation::settings(
            MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8301").unwrap(),
            device_id.clone(),
            SettingsValue {
                schema_version: 1,
                auto_sync: true,
                rockbox_compat: false,
                transcode_profile: TranscodeProfile::Alac,
            },
            0,
        )
        .unwrap(),
        PendingMutation::subscriptions(
            MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8302").unwrap(),
            device_id.clone(),
            SubscriptionsValue {
                schema_version: 1,
                playlists: playlists
                    .iter()
                    .map(|slug| PlaylistSlug::parse(slug).unwrap())
                    .collect(),
            },
            0,
        )
        .unwrap(),
    ];
    let outbox = PendingDeviceOutbox {
        schema_version: OUTBOX_SCHEMA_VERSION,
        device_id: device_id.clone(),
        mutations,
    };
    PortableStateStore::new(root)
        .initialize(&HostCache::new(device_id, None).unwrap(), &outbox)
        .unwrap();
}
