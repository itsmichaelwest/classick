use classick::wire::{
    decode_admitted_message, known_message_types, AdmittedStream, DecodedWireMessage,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const MANIFEST: &str = include_str!("data/wire-v3/manifest.json");
const COMMANDS: &str = include_str!("data/wire-v3/operations/commands.ndjson");
const EVENTS: &str = include_str!("data/wire-v3/operations/events.ndjson");
const STATE_MATRIX_COMMANDS: &str =
    include_str!("data/wire-v3/operations/state-matrix-commands.ndjson");
const STATE_MATRIX_EVENTS: &str =
    include_str!("data/wire-v3/operations/state-matrix-events.ndjson");
const LEGACY_DECISIONS: &str = include_str!("data/wire-v3/legacy-decisions.json");
const OPERATION_CONTRACT: &str = include_str!("data/wire-v3/operations/contract.json");

#[derive(Deserialize)]
struct Manifest {
    operations: OperationsVectors,
}

#[derive(Deserialize)]
struct OperationsVectors {
    positive_collections: Vec<Collection>,
    negative_vectors: Vec<Collection>,
}

#[derive(Deserialize)]
struct Collection {
    path: String,
    stream: String,
}

#[test]
fn remaining_operations_round_trip_the_shared_goldens() {
    assert_golden_lines(COMMANDS, &AdmittedStream::DaemonReceivingDesktopCommands);
    assert_golden_lines(EVENTS, &AdmittedStream::DesktopReceivingDaemonEvents);
    assert_golden_lines(
        STATE_MATRIX_COMMANDS,
        &AdmittedStream::DaemonReceivingDesktopCommands,
    );
    assert_golden_lines(
        STATE_MATRIX_EVENTS,
        &AdmittedStream::DesktopReceivingDaemonEvents,
    );
}

#[test]
fn manifest_discovers_the_operation_collections() {
    let manifest: Manifest = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(
        manifest
            .operations
            .positive_collections
            .iter()
            .map(|collection| (collection.path.as_str(), collection.stream.as_str()))
            .collect::<Vec<_>>(),
        [
            ("operations/commands.ndjson", "desktop_to_daemon_commands"),
            ("operations/events.ndjson", "daemon_to_desktop_events"),
            (
                "operations/state-matrix-commands.ndjson",
                "desktop_to_daemon_commands"
            ),
            (
                "operations/state-matrix-events.ndjson",
                "daemon_to_desktop_events"
            ),
        ]
    );
    for vector in manifest.operations.negative_vectors {
        let stream = match vector.stream.as_str() {
            "desktop_to_daemon_commands" => AdmittedStream::DaemonReceivingDesktopCommands,
            "daemon_to_desktop_events" => AdmittedStream::DesktopReceivingDaemonEvents,
            other => panic!("unknown operation vector stream {other}"),
        };
        assert!(
            decode_admitted_message(read_negative(&vector.path), &stream).is_err(),
            "accepted {}",
            vector.path
        );
    }
}

#[test]
fn operation_vectors_cover_every_new_discriminator() {
    let actual = COMMANDS
        .lines()
        .chain(EVENTS.lines())
        .chain(STATE_MATRIX_COMMANDS.lines())
        .chain(STATE_MATRIX_EVENTS.lines())
        .map(message_type)
        .collect::<BTreeSet<_>>();
    for required in [
        "get_global_config",
        "set_source_location",
        "set_global_settings",
        "trigger_sync",
        "backfill_rockbox",
        "replace_library",
        "get_history",
        "get_library",
        "scan_library",
        "retry_source_mount",
        "preview_selection",
        "preview_device",
        "resolve_tracks",
        "add_selection_to_device",
        "list_playlists",
        "get_playlist",
        "save_playlist",
        "delete_playlist",
        "append_selection_to_playlist",
        "shutdown",
        "global_config",
        "source_availability",
        "sync_accepted",
        "sync_rejected",
        "history",
        "library",
        "library_scan_started",
        "library_scan_progress",
        "library_scan_finished",
        "selection_preview",
        "device_preview",
        "resolved_tracks",
        "playlists",
        "playlist_detail",
        "playlist_saved",
        "device_selection_added",
        "playlist_selection_appended",
        "library_mutation_rejected",
        "daemon_shutdown_started",
    ] {
        assert!(actual.contains(required), "missing {required} vector");
        assert!(known_message_types().any(|known| known == required));
    }
}

#[test]
fn playlist_create_contract_returns_and_replays_the_canonical_slug() {
    let contract = serde_json::from_str::<Value>(OPERATION_CONTRACT).unwrap();
    for key in [
        "request_replay",
        "save_playlist",
        "playlist_broadcast",
        "scan_correlation",
    ] {
        assert!(!contract[key].as_str().unwrap().is_empty());
    }
    let create_request = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8817";
    let create = COMMANDS
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["request_id"] == create_request)
        .unwrap();
    assert_eq!(create["type"], "save_playlist");
    assert!(create["playlist"].get("slug").is_none());
    let saved = EVENTS
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["type"] == "playlist_saved" && value["request_id"] == create_request)
        .unwrap();
    assert_eq!(saved["playlist"]["slug"], "recent-pop");
}

#[test]
fn unsolicited_scan_events_omit_request_authority() {
    for json in [
        r#"{"type":"library_scan_started","session_id":43}"#,
        r#"{"type":"library_scan_progress","session_id":43,"files_scanned":5,"tracks_indexed":2}"#,
        r#"{"type":"library_scan_finished","session_id":43,"success":true}"#,
    ] {
        let decoded =
            decode_admitted_message(json, &AdmittedStream::DesktopReceivingDaemonEvents).unwrap();
        let DecodedWireMessage::Known(message) = decoded else {
            panic!("scan event must be known");
        };
        assert_eq!(serde_json::to_string(&*message).unwrap(), json);
    }
}

#[test]
fn operation_vectors_cover_every_public_enum_variant() {
    let commands = parsed_lines(&format!("{COMMANDS}\n{STATE_MATRIX_COMMANDS}"));
    let events = parsed_lines(&format!("{EVENTS}\n{STATE_MATRIX_EVENTS}"));
    assert_field_values(&commands, "notify_on", &["all", "errors_only", "none"]);
    assert_field_values(&commands, "trigger", &["manual", "plug_in", "scheduled"]);
    assert_field_values(
        &events,
        "trigger",
        &["coalesced", "manual", "plug_in", "scheduled"],
    );
    assert_field_values(&events, "outcome", &["aborted", "cancelled", "error", "ok"]);
    assert_field_values(&commands, "field", &["album", "artist", "genre", "year"]);
    assert_field_values(&commands, "op", &["contains", "gte", "is", "lte"]);
    assert!(STATE_MATRIX_COMMANDS.contains(r#""limit":{"tracks":100}"#));
    assert!(STATE_MATRIX_COMMANDS.contains(r#""limit":{"bytes":1073741824}"#));
    assert!(COMMANDS.contains(r#""limit":null"#));
}

#[test]
fn every_legacy_discriminator_has_a_v3_mapping_or_removal_decision() {
    let decisions = serde_json::from_str::<Value>(LEGACY_DECISIONS).unwrap();
    let known = known_message_types().collect::<BTreeSet<_>>();
    for (section, expected) in [
        (
            "v1_commands",
            &[
                "start",
                "review_decision",
                "prompt_decision",
                "form_decision",
                "cancel",
                "pause",
            ][..],
        ),
        (
            "v1_events",
            &[
                "hello",
                "header",
                "summary",
                "review",
                "prompt",
                "form",
                "track_start",
                "track_done",
                "finalizing",
                "cancelled",
                "log",
                "error",
                "finish",
                "paused",
            ][..],
        ),
        (
            "v2_commands",
            &[
                "get_status",
                "get_config",
                "save_config",
                "forget_ipod",
                "trigger_sync",
                "get_history",
                "subscribe_device_events",
                "unsubscribe_device_events",
                "cancel_sync",
                "pause",
                "decide_prompt",
                "backfill_rockbox",
                "replace_library",
                "get_library",
                "scan_library",
                "retry_source_mount",
                "preview_selection",
                "list_playlists",
                "get_playlist",
                "save_playlist",
                "delete_playlist",
                "get_device_config",
                "save_device_config",
                "preview_device",
                "resolve_tracks",
                "add_selection_to_device",
                "append_selection_to_playlist",
                "shutdown",
            ][..],
        ),
        (
            "v2_events",
            &[
                "hello",
                "status_update",
                "config_update",
                "history_update",
                "device_connected",
                "device_disconnected",
                "sync_rejected",
                "command_failed",
                "device_selection_added",
                "playlist_selection_appended",
                "library_mutation_rejected",
                "sync_event",
                "device_inventory_snapshot",
                "library_update",
                "selection_update",
                "selection_preview",
                "playlists_update",
                "playlist_detail",
                "device_config_update",
                "device_preview",
                "resolved_tracks",
                "source_availability",
            ][..],
        ),
    ] {
        let mapped = decisions["mapped"][section].as_object();
        let removed = decisions["removed"][section].as_object();
        for target in mapped
            .into_iter()
            .flat_map(|values| values.values())
            .flat_map(|value| value.as_str().unwrap().split('|'))
        {
            assert!(known.contains(target), "unknown v3 target {target}");
        }
        for reason in removed
            .into_iter()
            .flat_map(|values| values.values())
            .map(|value| value.as_str().unwrap())
        {
            assert!(!reason.is_empty(), "empty removal reason in {section}");
        }
        let actual = mapped
            .into_iter()
            .flat_map(|values| values.keys())
            .chain(removed.into_iter().flat_map(|values| values.keys()))
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected.iter().copied().collect());
    }
    let field_decisions = decisions["removed"]["v2_fields"].as_object().unwrap();
    assert_eq!(
        field_decisions
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        [
            "config.ipod",
            "daemon_settings.autostart_with_windows",
            "daemon_settings.enabled",
            "daemon_settings.rockbox_compat",
            "history.empty_serial",
            "history.missing_operation",
            "history.raw_serial",
        ]
        .into_iter()
        .collect()
    );
    assert!(field_decisions
        .values()
        .all(|reason| !reason.as_str().unwrap().is_empty()));
}

fn assert_golden_lines(ndjson: &str, stream: &AdmittedStream) {
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

fn parsed_lines(ndjson: &str) -> Vec<Value> {
    ndjson
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_field_values(values: &[Value], field: &str, expected: &[&str]) {
    let mut actual = BTreeSet::new();
    for value in values {
        collect_field_values(value, field, &mut actual);
    }
    assert_eq!(actual, expected.iter().copied().collect());
}

fn collect_field_values<'a>(value: &'a Value, field: &str, found: &mut BTreeSet<&'a str>) {
    match value {
        Value::Object(object) => {
            if let Some(value) = object.get(field).and_then(Value::as_str) {
                found.insert(value);
            }
            for value in object.values() {
                collect_field_values(value, field, found);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_field_values(value, field, found);
            }
        }
        _ => {}
    }
}

fn read_negative(path: &str) -> &'static str {
    match path {
        "operations/negative/source-credentials.json" => {
            include_str!("data/wire-v3/operations/negative/source-credentials.json")
        }
        "operations/negative/source-relative.json" => {
            include_str!("data/wire-v3/operations/negative/source-relative.json")
        }
        "operations/negative/history-zero-limit.json" => {
            include_str!("data/wire-v3/operations/negative/history-zero-limit.json")
        }
        "operations/negative/source-available-without-root.json" => {
            include_str!("data/wire-v3/operations/negative/source-available-without-root.json")
        }
        "operations/negative/scan-failure-without-message.json" => {
            include_str!("data/wire-v3/operations/negative/scan-failure-without-message.json")
        }
        "operations/negative/history-success-with-error.json" => {
            include_str!("data/wire-v3/operations/negative/history-success-with-error.json")
        }
        "operations/negative/library-unconfigured-with-content.json" => {
            include_str!("data/wire-v3/operations/negative/library-unconfigured-with-content.json")
        }
        "operations/negative/resolved-tracks-unsorted.json" => {
            include_str!("data/wire-v3/operations/negative/resolved-tracks-unsorted.json")
        }
        "operations/negative/playlist-detail-slug-mismatch.json" => {
            include_str!("data/wire-v3/operations/negative/playlist-detail-slug-mismatch.json")
        }
        "operations/negative/playlist-zero-limit.json" => {
            include_str!("data/wire-v3/operations/negative/playlist-zero-limit.json")
        }
        "operations/negative/mutation-empty-rules.json" => {
            include_str!("data/wire-v3/operations/negative/mutation-empty-rules.json")
        }
        "operations/negative/shutdown-missing-request.json" => {
            include_str!("data/wire-v3/operations/negative/shutdown-missing-request.json")
        }
        _ => panic!("manifest references unknown operation vector {path}"),
    }
}
