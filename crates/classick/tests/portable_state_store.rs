use classick::device::DeviceId;
use classick::portable::host_cache::HostCache;
use classick::portable::outbox::{PendingDeviceOutbox, PendingMutation, OUTBOX_SCHEMA_VERSION};
use classick::portable::profile::{MutationId, SelectionMode, SelectionValue};
use classick::portable::state_store::PortableStateStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn mutation() -> PendingMutation {
    PendingMutation::selection(
        MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808").unwrap(),
        device_id(),
        SelectionValue {
            schema_version: 1,
            mode: SelectionMode::All,
            rules: Vec::new(),
        },
        0,
    )
    .unwrap()
}

fn tempdir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "classick-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn host_acceptance_is_durable_before_device_delivery() {
    let root = tempdir("portable-host-accept");
    let store = PortableStateStore::new(&root);

    let accepted = store.accept_mutation(&mutation()).unwrap();

    assert_eq!(accepted.outbox.mutations, vec![mutation()]);
    assert_eq!(
        store.load(&device_id()).unwrap().outbox.mutations,
        vec![mutation()]
    );
}

#[test]
fn device_import_is_persisted_when_no_host_intent_is_pending() {
    let root = tempdir("portable-host-import");
    let store = PortableStateStore::new(&root);
    let cache = HostCache::new(device_id(), None).unwrap();

    store.import_device(&cache).unwrap();

    assert_eq!(store.load(&device_id()).unwrap().cache, Some(cache));
}

#[test]
fn legacy_initialization_is_complete_and_idempotent() {
    let root = tempdir("portable-legacy-initialize");
    let store = PortableStateStore::new(&root);
    let cache = HostCache::new(device_id(), None).unwrap();
    let outbox = PendingDeviceOutbox {
        schema_version: OUTBOX_SCHEMA_VERSION,
        device_id: device_id(),
        mutations: vec![mutation()],
    };

    let initialized = store.initialize(&cache, &outbox).unwrap();

    assert_eq!(initialized.cache, Some(cache.clone()));
    assert_eq!(initialized.outbox, outbox);
    assert!(store.is_initialized(&device_id()));
    assert_eq!(
        store.initialize(&cache, &initialized.outbox).unwrap(),
        initialized
    );

    assert!(store
        .initialize(&cache, &PendingDeviceOutbox::empty(device_id()))
        .is_err());
}
