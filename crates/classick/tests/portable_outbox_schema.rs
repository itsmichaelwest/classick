use classick::device::DeviceId;
use classick::portable::outbox::{
    OutboxLoad, PendingDeviceOutbox, PendingOutboxStore, OUTBOX_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn temp_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "portable-outbox-schema-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn device_id() -> DeviceId {
    DeviceId::parse("000A270012345678").unwrap()
}

fn mutation(component: &str, mutation_id: &str) -> Value {
    let desired = match component {
        "selection" => json!({ "schema_version": 1, "mode": "all", "rules": [] }),
        "settings" => json!({
            "schema_version": 1,
            "auto_sync": false,
            "rockbox_compat": true
        }),
        "subscriptions" => json!({ "schema_version": 1, "playlists": [] }),
        _ => unreachable!(),
    };
    json!({
        "component": component,
        "mutation_id": mutation_id,
        "device_id": device_id().as_str(),
        "desired": desired,
        "last_imported_device_revision": 0,
        "state": "pending_device"
    })
}

fn valid_outbox() -> Value {
    json!({
        "schema_version": OUTBOX_SCHEMA_VERSION,
        "device_id": device_id().as_str(),
        "mutations": [
            mutation("selection", "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740"),
            mutation("settings", "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8741"),
            mutation("subscriptions", "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8742")
        ]
    })
}

fn write_outbox(root: &Path, value: &Value) {
    let path = root.join("devices/000A270012345678/outbox.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn missing_outbox_is_an_explicit_empty_value_without_creating_state() {
    let root = temp_root("missing");
    let store = PendingOutboxStore::new(&root);
    let expected = PendingDeviceOutbox::empty(device_id());

    assert_eq!(
        store.load(&device_id()).unwrap(),
        OutboxLoad::Missing(expected)
    );
    assert_eq!(
        store.path(&device_id()),
        root.join("devices/000A270012345678/outbox.json")
    );
    assert!(!root.exists());
}

#[test]
fn loads_a_strict_canonical_outbox_in_component_order() {
    let root = temp_root("valid");
    write_outbox(&root, &valid_outbox());
    let store = PendingOutboxStore::new(&root);

    let OutboxLoad::Loaded(outbox) = store.load(&device_id()).unwrap() else {
        panic!("existing outbox was reported missing");
    };

    assert_eq!(outbox.mutations.len(), 3);
    assert_eq!(outbox.mutations[0].component_name(), "selection");
    assert_eq!(outbox.mutations[1].component_name(), "settings");
    assert_eq!(outbox.mutations[2].component_name(), "subscriptions");
}

#[test]
fn rejects_unknown_corrupt_noncanonical_mismatched_or_unordered_state() {
    let root = temp_root("invalid");
    let store = PendingOutboxStore::new(&root);
    let mut invalid_values = Vec::new();

    let mut unknown = valid_outbox();
    unknown["host_id"] = json!("forbidden");
    invalid_values.push(unknown);
    let mut nested_unknown = valid_outbox();
    nested_unknown["mutations"][0]["timestamp"] = json!(123);
    invalid_values.push(nested_unknown);
    let mut desired_unknown = valid_outbox();
    desired_unknown["mutations"][1]["desired"]["name"] = json!("forbidden");
    invalid_values.push(desired_unknown);
    let mut version = valid_outbox();
    version["schema_version"] = json!(OUTBOX_SCHEMA_VERSION + 1);
    invalid_values.push(version);
    let mut noncanonical = valid_outbox();
    noncanonical["device_id"] = json!("000a270012345678");
    invalid_values.push(noncanonical);
    let mut file_mismatch = valid_outbox();
    file_mismatch["device_id"] = json!("000A270087654321");
    invalid_values.push(file_mismatch);
    let mut mutation_mismatch = valid_outbox();
    mutation_mismatch["mutations"][0]["device_id"] = json!("000A270087654321");
    invalid_values.push(mutation_mismatch);
    let mut wrong_state = valid_outbox();
    wrong_state["mutations"][0]["state"] = json!("committed");
    invalid_values.push(wrong_state);
    let mut nil_id = valid_outbox();
    nil_id["mutations"][0]["mutation_id"] = json!("00000000-0000-0000-0000-000000000000");
    invalid_values.push(nil_id);
    let mut duplicate_id = valid_outbox();
    duplicate_id["mutations"][1]["mutation_id"] =
        duplicate_id["mutations"][0]["mutation_id"].clone();
    invalid_values.push(duplicate_id);
    let mut duplicate_component = valid_outbox();
    duplicate_component["mutations"][1] = duplicate_component["mutations"][0].clone();
    duplicate_component["mutations"][1]["mutation_id"] =
        json!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8799");
    invalid_values.push(duplicate_component);
    let mut unordered = valid_outbox();
    unordered["mutations"].as_array_mut().unwrap().swap(0, 1);
    invalid_values.push(unordered);

    for invalid in invalid_values {
        write_outbox(&root, &invalid);
        assert!(store.load(&device_id()).is_err(), "accepted {invalid}");
    }

    std::fs::write(store.path(&device_id()), b"{not-json").unwrap();
    assert!(store.load(&device_id()).is_err());
}

#[cfg(unix)]
#[test]
fn detects_an_unexpected_outbox_symlink_below_the_trusted_host_root() {
    use std::os::unix::fs::symlink;

    let root = temp_root("symlink");
    let outside = temp_root("symlink-outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("devices")).unwrap();

    assert!(PendingOutboxStore::new(&root).load(&device_id()).is_err());
}
