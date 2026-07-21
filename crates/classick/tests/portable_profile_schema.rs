use classick::portable::profile::PortableProfile;
use serde_json::{json, Value};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const MUTATION_SELECTION: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740";
const MUTATION_SETTINGS: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8741";
const MUTATION_SUBSCRIPTIONS: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8742";

fn valid_profile() -> Value {
    json!({
        "schema_version": 1,
        "device_id": "000A270012345678",
        "capability_profile_id": "classic-late-2009",
        "selection": {
            "revision": 1,
            "mutation_id": MUTATION_SELECTION,
            "value": {
                "schema_version": 1,
                "mode": "include",
                "rules": [
                    { "kind": "artist", "name": "Birdy" },
                    { "kind": "album", "artist": "Beck", "album": "Colors" },
                    { "kind": "genre", "name": "Electronic" }
                ]
            }
        },
        "settings": {
            "revision": 2,
            "mutation_id": MUTATION_SETTINGS,
            "value": {
                "schema_version": 1,
                "auto_sync": false,
                "rockbox_compat": true
            }
        },
        "subscriptions": {
            "revision": 3,
            "mutation_id": MUTATION_SUBSCRIPTIONS,
            "value": {
                "schema_version": 1,
                "playlists": ["favourites"]
            }
        },
        "owned_playlists": [{
            "slug": "favourites",
            "apple_playlist_id": 42,
            "apple_kind": "normal",
            "rockbox": {
                "relative_filename": "Playlists/favourites.m3u8",
                "content_hash": HASH_A
            }
        }],
        "companion_authorities": [
            {
                "kind": "manifest",
                "schema_version": 1,
                "relative_path": "manifest.json",
                "content_hash": HASH_B
            },
            {
                "kind": "playlist_definition",
                "slug": "favourites",
                "schema_version": 1,
                "relative_path": "playlists/favourites.m3u8",
                "content_hash": HASH_C
            }
        ],
        "generated_sysinfo_extended_hash": HASH_A
    })
}

fn decode(value: &Value) -> anyhow::Result<PortableProfile> {
    PortableProfile::from_json(&serde_json::to_string(value)?)
}

#[test]
fn accepts_and_canonically_round_trips_a_complete_profile() {
    let profile = decode(&valid_profile()).unwrap();
    let encoded = profile.to_json_pretty().unwrap();
    let decoded = PortableProfile::from_json(&encoded).unwrap();

    assert_eq!(decoded, profile);
    assert!(encoded.ends_with('\n'));
}

#[test]
fn accepts_absent_optional_capability_rockbox_and_sysinfo_hash() {
    let mut profile = valid_profile();
    let object = profile.as_object_mut().unwrap();
    object.remove("capability_profile_id");
    object.remove("generated_sysinfo_extended_hash");
    object["owned_playlists"][0]
        .as_object_mut()
        .unwrap()
        .remove("rockbox");

    decode(&profile).unwrap();
}

#[test]
fn rejects_every_excluded_profile_field() {
    let excluded = [
        "name",
        "display_name",
        "model",
        "model_code",
        "family",
        "generation",
        "colour",
        "color",
        "icon",
        "artwork_choice",
        "capacity",
        "firmware",
        "firmware_build",
        "battery",
        "volume",
        "volume_uuid",
        "mount",
        "mount_path",
        "host_id",
        "install_id",
        "timestamp",
        "last_seen",
        "telemetry",
        "runtime_facts",
        "library_id",
        "library_identity",
        "credentials",
        "username",
        "password",
    ];

    for key in excluded {
        let mut profile = valid_profile();
        profile[key] = json!("forbidden");
        assert!(decode(&profile).is_err(), "accepted excluded key {key:?}");
    }
}

#[test]
fn rejects_unknown_fields_at_every_nested_boundary() {
    let pointers = [
        "/selection",
        "/selection/value",
        "/selection/value/rules/0",
        "/settings",
        "/settings/value",
        "/subscriptions",
        "/subscriptions/value",
        "/owned_playlists/0",
        "/owned_playlists/0/rockbox",
        "/companion_authorities/0",
        "/companion_authorities/1",
    ];

    for pointer in pointers {
        let mut profile = valid_profile();
        profile
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(true));
        assert!(
            decode(&profile).is_err(),
            "accepted unknown key at {pointer}"
        );
    }
}

#[test]
fn rejects_noncanonical_or_invalid_device_ids() {
    for device_id in [
        "0x000A270012345678",
        "000a270012345678",
        "000A27001234567",
        "000A27001234567Z",
    ] {
        let mut profile = valid_profile();
        profile["device_id"] = json!(device_id);
        assert!(
            decode(&profile).is_err(),
            "accepted device ID {device_id:?}"
        );
    }
}

#[test]
fn rejects_unsupported_or_zero_schema_versions() {
    let pointers = [
        "/schema_version",
        "/selection/value/schema_version",
        "/settings/value/schema_version",
        "/subscriptions/value/schema_version",
        "/companion_authorities/0/schema_version",
        "/companion_authorities/1/schema_version",
    ];

    for pointer in pointers {
        for version in [0, 2] {
            let mut profile = valid_profile();
            *profile.pointer_mut(pointer).unwrap() = json!(version);
            assert!(
                decode(&profile).is_err(),
                "accepted version {version} at {pointer}"
            );
        }
    }
}

#[test]
fn rejects_zero_revisions_and_invalid_or_reused_mutation_ids() {
    for component in ["selection", "settings", "subscriptions"] {
        let mut profile = valid_profile();
        profile[component]["revision"] = json!(0);
        assert!(
            decode(&profile).is_err(),
            "accepted zero {component} revision"
        );

        for mutation_id in [
            "018F9D7E-2F2B-7B52-9F1D-F78BDB2F8740",
            "018f9d7e2f2b7b529f1df78bdb2f8740",
            "not-a-uuid",
        ] {
            let mut profile = valid_profile();
            profile[component]["mutation_id"] = json!(mutation_id);
            assert!(
                decode(&profile).is_err(),
                "accepted mutation ID {mutation_id:?}"
            );
        }
    }

    let mut duplicate = valid_profile();
    duplicate["settings"]["mutation_id"] = json!(MUTATION_SELECTION);
    assert!(decode(&duplicate).is_err());
}

#[test]
fn rejects_duplicate_subscription_and_ownership_claims() {
    let mut duplicate_subscription = valid_profile();
    duplicate_subscription["subscriptions"]["value"]["playlists"] =
        json!(["favourites", "favourites"]);
    assert!(decode(&duplicate_subscription).is_err());

    let mut duplicate_slug = valid_profile();
    let owned = duplicate_slug["owned_playlists"].as_array_mut().unwrap();
    owned.push(owned[0].clone());
    assert!(decode(&duplicate_slug).is_err());

    let mut duplicate_id = valid_profile();
    let mut second = duplicate_id["owned_playlists"][0].clone();
    second["slug"] = json!("running");
    second["rockbox"] = Value::Null;
    duplicate_id["owned_playlists"]
        .as_array_mut()
        .unwrap()
        .push(second);
    assert!(decode(&duplicate_id).is_err());

    let mut duplicate_rockbox_path = valid_profile();
    let mut second = duplicate_rockbox_path["owned_playlists"][0].clone();
    second["slug"] = json!("running");
    second["apple_playlist_id"] = json!(43);
    duplicate_rockbox_path["owned_playlists"]
        .as_array_mut()
        .unwrap()
        .push(second);
    assert!(decode(&duplicate_rockbox_path).is_err());
}

#[test]
fn rejects_duplicate_authority_slugs_and_portable_path_claims() {
    let mut duplicate_definition = valid_profile();
    let duplicate = duplicate_definition["companion_authorities"][1].clone();
    duplicate_definition["companion_authorities"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(decode(&duplicate_definition).is_err());

    let mut duplicate_path = valid_profile();
    duplicate_path["subscriptions"]["value"]["playlists"] = json!(["favourites", "running"]);
    let mut second = duplicate_path["companion_authorities"][1].clone();
    second["slug"] = json!("running");
    duplicate_path["companion_authorities"]
        .as_array_mut()
        .unwrap()
        .push(second);
    assert!(decode(&duplicate_path).is_err());
}

#[test]
fn rejects_zero_apple_ids_unknown_kinds_and_unsafe_slugs() {
    let mut zero_id = valid_profile();
    zero_id["owned_playlists"][0]["apple_playlist_id"] = json!(0);
    assert!(decode(&zero_id).is_err());

    let mut wrong_kind = valid_profile();
    wrong_kind["owned_playlists"][0]["apple_kind"] = json!("smart");
    assert!(decode(&wrong_kind).is_err());

    for slug in [
        "",
        "../escape",
        "UPPER",
        "two--hyphens",
        "has space",
        "under_score",
    ] {
        let mut profile = valid_profile();
        profile["subscriptions"]["value"]["playlists"][0] = json!(slug);
        profile["owned_playlists"][0]["slug"] = json!(slug);
        profile["companion_authorities"][1]["slug"] = json!(slug);
        assert!(decode(&profile).is_err(), "accepted slug {slug:?}");
    }
}

#[test]
fn rejects_invalid_hashes_everywhere() {
    let pointers = [
        "/owned_playlists/0/rockbox/content_hash",
        "/companion_authorities/0/content_hash",
        "/companion_authorities/1/content_hash",
        "/generated_sysinfo_extended_hash",
    ];
    for pointer in pointers {
        for hash in ["a", HASH_A.to_ascii_uppercase().as_str(), &"g".repeat(64)] {
            let mut profile = valid_profile();
            *profile.pointer_mut(pointer).unwrap() = json!(hash);
            assert!(
                decode(&profile).is_err(),
                "accepted hash {hash:?} at {pointer}"
            );
        }
    }
}

#[test]
fn rejects_nonportable_paths_and_credentials() {
    let hostile = [
        "",
        "/absolute/file.json",
        "//server/share/file.json",
        r"\\server\share\file.json",
        "C:/file.json",
        "C:\\file.json",
        "../file.json",
        "dir/../file.json",
        "./file.json",
        "dir//file.json",
        "https://example.test/file.json",
        "user:password@example.test/file.json",
        "dir\\file.json",
        "dir:file.json",
        "dir|file.json",
    ];

    for path in hostile {
        for pointer in [
            "/owned_playlists/0/rockbox/relative_filename",
            "/companion_authorities/0/relative_path",
            "/companion_authorities/1/relative_path",
        ] {
            let mut profile = valid_profile();
            *profile.pointer_mut(pointer).unwrap() = json!(path);
            assert!(
                decode(&profile).is_err(),
                "accepted path {path:?} at {pointer}"
            );
        }
    }
}

#[test]
fn requires_exact_definition_authority_for_each_subscription() {
    let mut missing = valid_profile();
    missing["companion_authorities"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(decode(&missing).is_err());

    let mut extra = valid_profile();
    extra["subscriptions"]["value"]["playlists"] = json!([]);
    assert!(decode(&extra).is_err());

    let mut mismatched = valid_profile();
    mismatched["companion_authorities"][1]["slug"] = json!("running");
    assert!(decode(&mismatched).is_err());
}

#[test]
fn complete_component_values_do_not_default_missing_fields() {
    let pointers = [
        "/selection/value/mode",
        "/selection/value/rules",
        "/settings/value/auto_sync",
        "/settings/value/rockbox_compat",
        "/subscriptions/value/playlists",
    ];

    for pointer in pointers {
        let mut profile = valid_profile();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        profile
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(
            decode(&profile).is_err(),
            "defaulted missing field {pointer}"
        );
    }
}
