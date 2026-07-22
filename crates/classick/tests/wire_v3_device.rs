use classick::device::DeviceId;
use classick::wire::{
    decode_admitted_message, known_message_types, AdmittedStream, DecodedWireMessage,
    OwnedSessionRoute, PendingWorkerInteraction, SessionId, WorkerCommandAdmission,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const MANIFEST: &str = include_str!("data/wire-v3/manifest.json");
const DEVICE_COMMANDS: &str = include_str!("data/wire-v3/device/commands.ndjson");
const DEVICE_EVENTS: &str = include_str!("data/wire-v3/device/events.ndjson");
const DEVICE_STATE_MATRIX: &str = include_str!("data/wire-v3/device/state-matrix.ndjson");
const PROGRESS_COMMANDS: &str = include_str!("data/wire-v3/progress/commands.ndjson");
const PROGRESS_EVENTS: &str = include_str!("data/wire-v3/progress/events.ndjson");

#[derive(Deserialize)]
struct Manifest {
    device: DeviceVectors,
}

#[derive(Deserialize)]
struct DeviceVectors {
    positive_collections: Vec<Collection>,
    negative_vectors: Vec<NegativeVector>,
}

#[derive(Deserialize)]
struct Collection {
    path: String,
    stream: String,
}

#[derive(Deserialize)]
struct NegativeVector {
    path: String,
    expectation: String,
    stream: String,
}

#[test]
fn device_commands_and_events_round_trip_the_shared_goldens() {
    assert_golden_lines(
        DEVICE_COMMANDS,
        &AdmittedStream::DaemonReceivingDesktopCommands,
        &[
            "get_inventory",
            "subscribe_inventory",
            "unsubscribe_inventory",
            "adopt_device",
            "forget_device",
            "get_device_config",
            "set_selection",
            "set_settings",
            "set_subscriptions",
        ],
    );
    assert_golden_lines(
        DEVICE_STATE_MATRIX,
        &AdmittedStream::DesktopReceivingDaemonEvents,
        &["device_inventory", "device_config"],
    );
    assert_golden_lines(
        DEVICE_EVENTS,
        &AdmittedStream::DesktopReceivingDaemonEvents,
        &[
            "device_inventory",
            "inventory_subscription_changed",
            "device_config",
            "config_mutation_failed",
            "device_forgotten",
        ],
    );
}

#[test]
fn manifest_discovers_every_device_vector_and_negative_cases_fail() {
    let manifest: Manifest = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(
        manifest
            .device
            .positive_collections
            .iter()
            .map(|collection| (collection.path.as_str(), collection.stream.as_str()))
            .collect::<Vec<_>>(),
        [
            ("device/commands.ndjson", "desktop_to_daemon_commands"),
            ("device/events.ndjson", "daemon_to_desktop_events"),
            ("device/state-matrix.ndjson", "daemon_to_desktop_events")
        ]
    );
    for vector in manifest.device.negative_vectors {
        assert!(matches!(
            vector.expectation.as_str(),
            "semantic_failure" | "forbidden_observation_id" | "decode_failure"
        ));
        let stream = match vector.stream.as_str() {
            "desktop_to_daemon_commands" => AdmittedStream::DaemonReceivingDesktopCommands,
            "daemon_to_desktop_events" => AdmittedStream::DesktopReceivingDaemonEvents,
            other => panic!("unknown device vector stream {other}"),
        };
        assert!(
            decode_admitted_message(read_negative(&vector.path), &stream).is_err(),
            "accepted {}",
            vector.path
        );
    }
}

#[test]
fn unidentified_observations_cannot_form_mutation_commands() {
    let json = r#"{"type":"set_settings","observation_id":7,"request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8767","mutation_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8774","settings":{"schema_version":1,"auto_sync":false,"rockbox_compat":false}}"#;
    assert!(
        decode_admitted_message(json, &AdmittedStream::DaemonReceivingDesktopCommands).is_err()
    );
}

#[test]
fn correlated_config_vectors_bind_requests_to_the_exact_component_mutation() {
    let configs = DEVICE_STATE_MATRIX
        .lines()
        .filter(|line| message_type(line) == "device_config")
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(configs.len(), 4);
    assert_eq!(
        configs[0]["request_id"],
        "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8763"
    );
    for config in &configs[1..3] {
        assert_eq!(config["request_id"], "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8767");
        assert_eq!(
            config["settings"]["mutation_id"],
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8774"
        );
    }
    assert_eq!(
        configs[1]["settings"]["delivery"]["state"],
        "pending_device"
    );
    assert_eq!(
        configs[2]["settings"]["delivery"]["state"],
        "device_committed"
    );
}

#[test]
fn device_messages_are_forbidden_on_the_owned_worker_channel() {
    let route = OwnedSessionRoute::new(
        DeviceId::parse("000A27002138B0A8").unwrap(),
        SessionId::new(42).unwrap(),
    );
    let worker_commands = AdmittedStream::WorkerReceivingDaemonCommands(
        WorkerCommandAdmission::new(route.clone(), PendingWorkerInteraction::None),
    );
    assert!(
        decode_admitted_message(DEVICE_COMMANDS.lines().next().unwrap(), &worker_commands).is_err()
    );
    assert!(decode_admitted_message(
        DEVICE_EVENTS.lines().next().unwrap(),
        &AdmittedStream::DaemonReceivingWorkerEvents(route)
    )
    .is_err());
}

#[test]
fn portable_config_vectors_contain_no_appearance_or_host_runtime_fields() {
    for line in DEVICE_COMMANDS
        .lines()
        .filter(|line| line.contains("selection") || line.contains("settings"))
    {
        for prohibited in [
            "appearance",
            "artwork_choice",
            "colour",
            "capacity",
            "firmware",
            "mount",
            "volume",
            "host_id",
            "library_path",
        ] {
            assert!(!line.contains(&format!(r#""{prohibited}""#)), "{line}");
        }
    }
}

#[test]
fn every_current_discriminator_has_a_shared_positive_vector() {
    let mut golden_types = BTreeSet::from(["hello".to_owned()]);
    for line in PROGRESS_COMMANDS
        .lines()
        .chain(PROGRESS_EVENTS.lines())
        .chain(DEVICE_COMMANDS.lines())
        .chain(DEVICE_EVENTS.lines())
        .chain(DEVICE_STATE_MATRIX.lines())
    {
        golden_types.insert(message_type(line));
    }
    assert_eq!(
        golden_types,
        known_message_types()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
}

fn assert_golden_lines(ndjson: &str, stream: &AdmittedStream, expected_types: &[&str]) {
    let actual_types = ndjson.lines().map(message_type).collect::<BTreeSet<_>>();
    assert_eq!(
        actual_types,
        expected_types
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    );
    for line in ndjson.lines() {
        let DecodedWireMessage::Known(message) = decode_admitted_message(line, stream).unwrap()
        else {
            panic!("golden vector was treated as unknown: {line}");
        };
        assert_eq!(serde_json::to_string(&message).unwrap(), line);
    }
}

fn message_type(json: &str) -> String {
    serde_json::from_str::<Value>(json).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn read_negative(path: &str) -> &'static str {
    match path {
        "device/negative/inventory-duplicate-device.json" => {
            include_str!("data/wire-v3/device/negative/inventory-duplicate-device.json")
        }
        "device/negative/inventory-disconnected-storage.json" => {
            include_str!("data/wire-v3/device/negative/inventory-disconnected-storage.json")
        }
        "device/negative/config-duplicate-mutation.json" => {
            include_str!("data/wire-v3/device/negative/config-duplicate-mutation.json")
        }
        "device/negative/config-empty-delivery-error.json" => {
            include_str!("data/wire-v3/device/negative/config-empty-delivery-error.json")
        }
        "device/negative/set-subscriptions-duplicate.json" => {
            include_str!("data/wire-v3/device/negative/set-subscriptions-duplicate.json")
        }
        "device/negative/inventory-connected-disconnected-phase.json" => {
            include_str!("data/wire-v3/device/negative/inventory-connected-disconnected-phase.json")
        }
        "device/negative/inventory-connected-missing-mount.json" => {
            include_str!("data/wire-v3/device/negative/inventory-connected-missing-mount.json")
        }
        "device/negative/inventory-relative-mount.json" => {
            include_str!("data/wire-v3/device/negative/inventory-relative-mount.json")
        }
        "device/negative/inventory-duplicate-mount.json" => {
            include_str!("data/wire-v3/device/negative/inventory-duplicate-mount.json")
        }
        "device/negative/inventory-unready-syncing.json" => {
            include_str!("data/wire-v3/device/negative/inventory-unready-syncing.json")
        }
        "device/negative/inventory-invalid-provenance.json" => {
            include_str!("data/wire-v3/device/negative/inventory-invalid-provenance.json")
        }
        "device/negative/inventory-empty-hardware-string.json" => {
            include_str!("data/wire-v3/device/negative/inventory-empty-hardware-string.json")
        }
        "device/negative/inventory-zero-capacity.json" => {
            include_str!("data/wire-v3/device/negative/inventory-zero-capacity.json")
        }
        "device/negative/set-settings-with-observation.json" => {
            include_str!("data/wire-v3/device/negative/set-settings-with-observation.json")
        }
        "device/negative/inventory-zero-observation.json" => {
            include_str!("data/wire-v3/device/negative/inventory-zero-observation.json")
        }
        "device/negative/inventory-duplicate-observation.json" => {
            include_str!("data/wire-v3/device/negative/inventory-duplicate-observation.json")
        }
        "device/negative/inventory-zero-revision.json" => {
            include_str!("data/wire-v3/device/negative/inventory-zero-revision.json")
        }
        "device/negative/inventory-invalid-storage.json" => {
            include_str!("data/wire-v3/device/negative/inventory-invalid-storage.json")
        }
        "device/negative/inventory-identified-unavailable.json" => {
            include_str!("data/wire-v3/device/negative/inventory-identified-unavailable.json")
        }
        _ => panic!("manifest references unknown device vector {path}"),
    }
}
