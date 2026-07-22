use classick::atomic_file::AtomicFileWriter;
use classick::device::DeviceId;
use classick::portable::outbox::{
    CommittedComponentProof, OutboxLoad, PendingMutation, PendingOutboxStore,
};
use classick::portable::profile::{
    ContentHash, MutationId, PlaylistSlug, ProfileComponent, SelectionMode, SelectionValue,
    SettingsValue, SubscriptionsValue,
};
use std::path::{Path, PathBuf};

fn temp_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "portable-outbox-operations-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn device_id(value: &str) -> DeviceId {
    DeviceId::parse(value).unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f87{suffix:02x}")).unwrap()
}

fn selection(id: &DeviceId, mutation: MutationId, mode: SelectionMode) -> PendingMutation {
    PendingMutation::selection(
        mutation,
        id.clone(),
        SelectionValue {
            schema_version: 1,
            mode,
            rules: vec![],
        },
        0,
    )
    .unwrap()
}

fn settings(id: &DeviceId, mutation: MutationId, auto_sync: bool) -> PendingMutation {
    PendingMutation::settings(
        mutation,
        id.clone(),
        SettingsValue {
            schema_version: 1,
            auto_sync,
            rockbox_compat: false,
        },
        7,
    )
    .unwrap()
}

fn subscriptions(id: &DeviceId, mutation: MutationId) -> PendingMutation {
    PendingMutation::subscriptions(
        mutation,
        id.clone(),
        SubscriptionsValue {
            schema_version: 1,
            playlists: vec![],
        },
        3,
    )
    .unwrap()
}

fn loaded(
    store: &PendingOutboxStore,
    device: &DeviceId,
) -> classick::portable::outbox::PendingDeviceOutbox {
    match store.load(device).unwrap() {
        OutboxLoad::Missing(empty) | OutboxLoad::Loaded(empty) => empty,
    }
}

fn component_hash<T: serde::Serialize>(component: &ProfileComponent<T>) -> ContentHash {
    let bytes = serde_json::to_vec(component).unwrap();
    ContentHash::parse(blake3::hash(&bytes).to_hex().as_str()).unwrap()
}

#[test]
fn accepts_identical_replay_as_a_no_op_and_rejects_changed_reuse() {
    let root = temp_root("idempotence");
    let store = PendingOutboxStore::new(&root);
    let device = device_id("000A270012345678");
    let pending = settings(&device, mutation_id(1), false);
    store.accept(&device, pending.clone()).unwrap();
    let bytes = std::fs::read(store.path(&device)).unwrap();

    assert_eq!(
        store.accept(&device, pending.clone()).unwrap(),
        loaded(&store, &device)
    );
    assert_eq!(std::fs::read(store.path(&device)).unwrap(), bytes);

    let reused = settings(&device, pending.mutation_id().clone(), true);
    assert!(store.accept(&device, reused).is_err());
    assert_eq!(std::fs::read(store.path(&device)).unwrap(), bytes);

    let other_device = device_id("000A270087654321");
    let reused_for_other_device = settings(&other_device, pending.mutation_id().clone(), false);
    assert!(store.accept(&device, reused_for_other_device).is_err());
    assert!(!store.path(&other_device).exists());
}

#[test]
fn coalesces_only_the_same_component_and_orders_components_deterministically() {
    let root = temp_root("coalesce");
    let store = PendingOutboxStore::new(&root);
    let first_device = device_id("000A270012345678");
    let second_device = device_id("000A270087654321");
    store
        .accept(&first_device, subscriptions(&first_device, mutation_id(1)))
        .unwrap();
    store
        .accept(
            &first_device,
            settings(&first_device, mutation_id(2), false),
        )
        .unwrap();
    store
        .accept(
            &first_device,
            selection(&first_device, mutation_id(3), SelectionMode::All),
        )
        .unwrap();
    store
        .accept(&first_device, settings(&first_device, mutation_id(4), true))
        .unwrap();
    store
        .accept(
            &second_device,
            settings(&second_device, mutation_id(5), false),
        )
        .unwrap();

    let first = loaded(&store, &first_device);
    assert_eq!(first.mutations.len(), 3);
    assert_eq!(first.mutations[0].component_name(), "selection");
    assert_eq!(first.mutations[1].component_name(), "settings");
    assert_eq!(first.mutations[1].mutation_id(), &mutation_id(4));
    assert_eq!(first.mutations[2].component_name(), "subscriptions");
    assert_eq!(loaded(&store, &second_device).mutations.len(), 1);
}

#[test]
fn failed_coalescing_save_retains_the_prior_durable_outbox() {
    let root = temp_root("failed-save");
    let normal = PendingOutboxStore::new(&root);
    let device = device_id("000A270012345678");
    normal
        .accept(&device, settings(&device, mutation_id(1), false))
        .unwrap();
    let path = normal.path(&device);
    let old_bytes = std::fs::read(&path).unwrap();
    let failing = PendingOutboxStore::with_writer(
        &root,
        AtomicFileWriter::failing_before_replace(path.clone()),
    );

    assert!(failing
        .accept(&device, settings(&device, mutation_id(2), true))
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), old_bytes);
    assert_eq!(
        loaded(&normal, &device).mutations[0].mutation_id(),
        &mutation_id(1)
    );
}

#[test]
fn confirmation_requires_exact_committed_revision_value_and_hash_proof() {
    let root = temp_root("confirm");
    let store = PendingOutboxStore::new(&root);
    let device = device_id("000A270012345678");
    let pending_id = mutation_id(1);
    store
        .accept(
            &device,
            selection(&device, pending_id.clone(), SelectionMode::All),
        )
        .unwrap();
    store
        .accept(&device, settings(&device, mutation_id(2), false))
        .unwrap();
    let committed = ProfileComponent {
        revision: 9,
        mutation_id: pending_id.clone(),
        value: SelectionValue {
            schema_version: 1,
            mode: SelectionMode::All,
            rules: vec![],
        },
    };
    let proof = CommittedComponentProof::selection(
        device.clone(),
        committed.clone(),
        component_hash(&committed),
    );

    assert!(store.confirm(&device, &mutation_id(99), &proof).is_err());
    let wrong_device = CommittedComponentProof::selection(
        device_id("000A270087654321"),
        committed.clone(),
        component_hash(&committed),
    );
    assert!(store.confirm(&device, &pending_id, &wrong_device).is_err());
    let mut wrong_value = committed.clone();
    wrong_value.value.mode = SelectionMode::Exclude;
    let wrong_value_proof = CommittedComponentProof::selection(
        device.clone(),
        wrong_value.clone(),
        component_hash(&wrong_value),
    );
    assert!(store
        .confirm(&device, &pending_id, &wrong_value_proof)
        .is_err());
    let wrong_hash = CommittedComponentProof::selection(
        device.clone(),
        committed.clone(),
        ContentHash::parse(&"a".repeat(64)).unwrap(),
    );
    assert!(store.confirm(&device, &pending_id, &wrong_hash).is_err());
    assert_eq!(loaded(&store, &device).mutations.len(), 2);

    let confirmed = store.confirm(&device, &pending_id, &proof).unwrap();
    assert_eq!(confirmed.mutations.len(), 1);
    assert_eq!(confirmed.mutations[0].component_name(), "settings");
    assert!(store.confirm(&device, &pending_id, &proof).is_err());
}

#[test]
fn failed_confirmation_save_leaves_the_mutation_pending() {
    let root = temp_root("failed-confirm");
    let normal = PendingOutboxStore::new(&root);
    let device = device_id("000A270012345678");
    let pending_id = mutation_id(1);
    normal
        .accept(
            &device,
            selection(&device, pending_id.clone(), SelectionMode::All),
        )
        .unwrap();
    let path = normal.path(&device);
    let old_bytes = std::fs::read(&path).unwrap();
    let committed = ProfileComponent {
        revision: 1,
        mutation_id: pending_id.clone(),
        value: SelectionValue {
            schema_version: 1,
            mode: SelectionMode::All,
            rules: vec![],
        },
    };
    let proof = CommittedComponentProof::selection(
        device.clone(),
        committed.clone(),
        component_hash(&committed),
    );
    let failing = PendingOutboxStore::with_writer(
        &root,
        AtomicFileWriter::failing_before_replace(path.clone()),
    );

    assert!(failing.confirm(&device, &pending_id, &proof).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), old_bytes);
    assert_eq!(loaded(&normal, &device).mutations.len(), 1);
}

#[test]
fn confirmation_proof_is_typed_for_settings_and_subscriptions() {
    let root = temp_root("typed-proofs");
    let store = PendingOutboxStore::new(&root);
    let device = device_id("000A270012345678");
    let settings_id = mutation_id(1);
    let subscriptions_id = mutation_id(2);
    store
        .accept(&device, settings(&device, settings_id.clone(), false))
        .unwrap();
    store
        .accept(&device, subscriptions(&device, subscriptions_id.clone()))
        .unwrap();

    let committed_settings = ProfileComponent {
        revision: 11,
        mutation_id: settings_id.clone(),
        value: SettingsValue {
            schema_version: 1,
            auto_sync: false,
            rockbox_compat: false,
        },
    };
    let settings_proof = CommittedComponentProof::settings(
        device.clone(),
        committed_settings.clone(),
        component_hash(&committed_settings),
    );
    store
        .confirm(&device, &settings_id, &settings_proof)
        .unwrap();

    let committed_subscriptions = ProfileComponent {
        revision: 12,
        mutation_id: subscriptions_id.clone(),
        value: SubscriptionsValue {
            schema_version: 1,
            playlists: vec![],
        },
    };
    let subscriptions_proof = CommittedComponentProof::subscriptions(
        device.clone(),
        committed_subscriptions.clone(),
        component_hash(&committed_subscriptions),
    );
    let confirmed = store
        .confirm(&device, &subscriptions_id, &subscriptions_proof)
        .unwrap();

    assert!(confirmed.mutations.is_empty());
}

#[test]
fn rejects_noncanonical_desired_component_values_before_persistence() {
    let device = device_id("000A270012345678");
    assert!(PendingMutation::selection(
        mutation_id(1),
        device.clone(),
        SelectionValue {
            schema_version: 2,
            mode: SelectionMode::All,
            rules: vec![],
        },
        0,
    )
    .is_err());
    assert!(PendingMutation::settings(
        mutation_id(2),
        device.clone(),
        SettingsValue {
            schema_version: 2,
            auto_sync: false,
            rockbox_compat: false,
        },
        0,
    )
    .is_err());
    assert!(PendingMutation::subscriptions(
        mutation_id(3),
        device,
        SubscriptionsValue {
            schema_version: 1,
            playlists: vec![
                PlaylistSlug::parse("favourites").unwrap(),
                PlaylistSlug::parse("favourites").unwrap(),
            ],
        },
        0,
    )
    .is_err());
}
