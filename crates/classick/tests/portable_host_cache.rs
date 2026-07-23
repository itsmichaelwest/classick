use classick::device::DeviceId;
use classick::portable::host_cache::{
    HostCache, HostCacheLoad, HostCacheStore, HOST_CACHE_SCHEMA_VERSION,
};
use classick::portable::profile::PortableProfile;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn temp_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "portable-host-cache-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn device_id() -> DeviceId {
    DeviceId::parse("000A270012345678").unwrap()
}

fn other_device_id() -> DeviceId {
    DeviceId::parse("000A270087654321").unwrap()
}

fn profile(device_id: &DeviceId) -> PortableProfile {
    PortableProfile::from_json(
        &json!({
            "schema_version": 1,
            "device_id": device_id.as_str(),
            "selection": {
                "revision": 1,
                "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740",
                "value": { "schema_version": 1, "mode": "all", "rules": [] }
            },
            "settings": {
                "revision": 2,
                "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8741",
                "value": {
                    "schema_version": 1,
                    "auto_sync": false,
                    "rockbox_compat": true,
                    "transcode_profile": "alac"
                }
            },
            "subscriptions": {
                "revision": 3,
                "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8742",
                "value": { "schema_version": 1, "playlists": [] }
            },
            "owned_playlists": [],
            "companion_authorities": []
        })
        .to_string(),
    )
    .unwrap()
}

fn write_json(root: &Path, device_id: &DeviceId, value: &Value) {
    let path = root
        .join("devices")
        .join(device_id.as_str())
        .join("cache.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn missing_cache_is_explicit_and_read_only() {
    let root = temp_root("missing");
    let store = HostCacheStore::new(&root);

    assert_eq!(store.load(&device_id()).unwrap(), HostCacheLoad::Missing);
    assert_eq!(
        store.path(&device_id()),
        root.join("devices/000A270012345678/cache.json")
    );
    assert!(!root.exists());
}

#[test]
fn loads_only_the_last_imported_canonical_profile() {
    let root = temp_root("round-trip");
    let store = HostCacheStore::new(&root);
    let expected = HostCache::new(device_id(), Some(profile(&device_id()))).unwrap();
    let value = serde_json::to_value(&expected).unwrap();
    write_json(&root, &device_id(), &value);

    assert_eq!(
        store.load(&device_id()).unwrap(),
        HostCacheLoad::Loaded(expected)
    );
    let json: Value =
        serde_json::from_slice(&std::fs::read(store.path(&device_id())).unwrap()).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 3);
    assert!(json.get("device_id").is_some());
    assert!(json.get("last_imported_profile").is_some());
    assert_eq!(json["schema_version"], HOST_CACHE_SCHEMA_VERSION);
}

#[test]
fn accepts_an_explicitly_empty_cache() {
    let root = temp_root("empty");
    let store = HostCacheStore::new(&root);
    let expected = HostCache::new(device_id(), None).unwrap();
    let value = serde_json::to_value(&expected).unwrap();

    write_json(&root, &device_id(), &value);

    assert_eq!(
        store.load(&device_id()).unwrap(),
        HostCacheLoad::Loaded(expected)
    );
}

#[test]
fn rejects_corrupt_unknown_versioned_or_mismatched_cache_files() {
    let root = temp_root("invalid");
    let store = HostCacheStore::new(&root);
    let expected_device = device_id();
    let valid_profile = serde_json::to_value(profile(&expected_device)).unwrap();
    let valid = json!({
        "schema_version": HOST_CACHE_SCHEMA_VERSION,
        "device_id": expected_device.as_str(),
        "last_imported_profile": valid_profile
    });

    let mut invalid_values = Vec::new();
    let mut unknown = valid.clone();
    unknown["name"] = json!("forbidden");
    invalid_values.push(unknown);
    let mut version = valid.clone();
    version["schema_version"] = json!(HOST_CACHE_SCHEMA_VERSION + 1);
    invalid_values.push(version);
    let mut noncanonical = valid.clone();
    noncanonical["device_id"] = json!("000a270012345678");
    invalid_values.push(noncanonical);
    let mut embedded_mismatch = valid.clone();
    embedded_mismatch["last_imported_profile"]["device_id"] = json!(other_device_id().as_str());
    invalid_values.push(embedded_mismatch);
    let mut file_mismatch = valid;
    file_mismatch["device_id"] = json!(other_device_id().as_str());
    invalid_values.push(file_mismatch);

    for invalid in invalid_values {
        write_json(&root, &expected_device, &invalid);
        assert!(store.load(&expected_device).is_err(), "accepted {invalid}");
    }

    std::fs::write(store.path(&expected_device), b"{not-json").unwrap();
    assert!(store.load(&expected_device).is_err());
}

#[cfg(unix)]
#[test]
fn detects_an_unexpected_symlink_below_the_trusted_host_root() {
    use std::os::unix::fs::symlink;

    let root = temp_root("symlink");
    let outside = temp_root("symlink-outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("devices")).unwrap();
    let store = HostCacheStore::new(&root);

    assert!(store.load(&device_id()).is_err());
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
}
