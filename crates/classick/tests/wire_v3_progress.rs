use classick::device::DeviceId;
use classick::wire::{
    decode_admitted_message, known_message_types, ActionPlanSummary, AdmittedStream,
    DecodedWireMessage, OwnedSessionRoute, PendingWorkerInteraction, PromptId, RequestId,
    SessionId, TrackResult, WireCommand, WireEvent, WireMessage, WorkerCommandAdmission,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const COMMAND_GOLDENS: &str = include_str!("data/wire-v3/progress/commands.ndjson");
const EVENT_GOLDENS: &str = include_str!("data/wire-v3/progress/events.ndjson");
const MANIFEST: &str = include_str!("data/wire-v3/manifest.json");

#[test]
fn every_progress_command_has_a_canonical_language_neutral_vector() {
    assert_golden_lines(
        COMMAND_GOLDENS,
        AdmittedStream::DaemonReceivingDesktopCommands,
        &[
            "apply_review",
            "dry_run_review",
            "quit_review",
            "prompt_decision",
            "form_decision",
            "cancel_sync",
            "pause_sync",
        ],
    );
}

#[test]
fn every_progress_event_has_a_canonical_language_neutral_vector() {
    assert_golden_lines(
        EVENT_GOLDENS,
        AdmittedStream::DesktopReceivingDaemonEvents,
        &[
            "run_header",
            "sync_summary",
            "review_requested",
            "prompt",
            "form",
            "track_start",
            "track_done",
            "finalizing",
            "sync_cancelled",
            "sync_paused",
            "sync_log",
            "sync_error",
            "sync_finished",
            "command_failed",
        ],
    );
}

#[test]
fn shared_manifest_discovers_positive_and_negative_progress_vectors() {
    #[derive(Deserialize)]
    struct Manifest {
        progress: ProgressVectors,
    }
    #[derive(Deserialize)]
    struct ProgressVectors {
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
        expected_device_id: Option<DeviceId>,
        expected_session_id: Option<SessionId>,
        prompt_id: Option<PromptId>,
        option_count: Option<u32>,
    }

    let manifest: Manifest = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(manifest.progress.positive_collections.len(), 2);
    assert_eq!(
        manifest
            .progress
            .positive_collections
            .iter()
            .map(|collection| (collection.path.as_str(), collection.stream.as_str()))
            .collect::<Vec<_>>(),
        [
            ("progress/commands.ndjson", "desktop_to_daemon_commands"),
            ("progress/events.ndjson", "daemon_to_desktop_events")
        ]
    );

    for vector in manifest.progress.negative_vectors {
        assert!(matches!(
            vector.expectation.as_str(),
            "decode_failure"
                | "owned_session_mismatch"
                | "wrong_direction"
                | "semantic_failure"
                | "pending_interaction_mismatch"
        ));
        let stream = match vector.stream.as_str() {
            "daemon_to_desktop_events" => AdmittedStream::DesktopReceivingDaemonEvents,
            "worker_to_daemon_events" => {
                AdmittedStream::DaemonReceivingWorkerEvents(OwnedSessionRoute::new(
                    vector.expected_device_id.unwrap(),
                    vector.expected_session_id.unwrap(),
                ))
            }
            "daemon_to_worker_commands" => worker_commands_for(
                OwnedSessionRoute::new(
                    vector.expected_device_id.unwrap(),
                    vector.expected_session_id.unwrap(),
                ),
                PendingWorkerInteraction::Prompt {
                    prompt_id: vector.prompt_id.unwrap(),
                    option_count: vector.option_count.unwrap(),
                },
            ),
            other => panic!("unknown negative-vector stream {other}"),
        };
        assert!(
            decode_admitted_message(read_negative(&vector.path), &stream).is_err(),
            "accepted {} ({})",
            vector.path,
            vector.expectation
        );
    }
}

#[test]
fn discriminator_catalogue_is_exhaustively_covered_by_shared_goldens() {
    let mut golden_types = BTreeSet::from(["hello".to_owned()]);
    for line in COMMAND_GOLDENS.lines().chain(EVENT_GOLDENS.lines()) {
        golden_types.insert(message_type(line));
    }
    assert_eq!(
        golden_types,
        known_message_types()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn typed_progress_is_flat_and_round_trips_on_both_event_streams() {
    let message = WireMessage::Event(WireEvent::TrackDone {
        device_id: device_id(),
        session_id: session_id(),
        result: TrackResult::Applied,
    });
    let json = serde_json::to_string(&message).unwrap();
    assert_eq!(
        json,
        r#"{"type":"track_done","device_id":"000A27002138B0A8","session_id":42,"result":"applied"}"#
    );
    for stream in [desktop_events(), worker_events()] {
        assert_eq!(
            decode_admitted_message(&json, &stream).unwrap(),
            DecodedWireMessage::Known(message.clone())
        );
    }
}

#[test]
fn decisions_are_flat_and_valid_on_desktop_and_worker_command_streams() {
    let message = WireMessage::Command(WireCommand::PromptDecision {
        device_id: device_id(),
        session_id: session_id(),
        request_id: request_id(),
        prompt_id: PromptId::new(7).unwrap(),
        choice: 2,
    });
    let json = serde_json::to_string(&message).unwrap();
    assert!(!json.contains("line"));
    for stream in [
        AdmittedStream::DaemonReceivingDesktopCommands,
        worker_commands(PendingWorkerInteraction::Prompt {
            prompt_id: PromptId::new(7).unwrap(),
            option_count: 3,
        }),
    ] {
        assert_eq!(
            decode_admitted_message(&json, &stream).unwrap(),
            DecodedWireMessage::Known(message.clone())
        );
    }
}

#[test]
fn known_progress_with_missing_or_malformed_routing_is_rejected_not_ignored() {
    for json in [
        r#"{"type":"track_done","session_id":42,"result":"applied"}"#,
        r#"{"type":"track_done","device_id":"000A27002138B0A8","result":"applied"}"#,
        r#"{"type":"track_done","device_id":"0x000A27002138B0A8","session_id":42,"result":"applied"}"#,
        r#"{"type":"track_done","device_id":"000A27002138B0A8","session_id":0,"result":"applied"}"#,
    ] {
        assert!(
            decode_admitted_message(json, &AdmittedStream::DesktopReceivingDaemonEvents).is_err()
        );
    }
}

#[test]
fn command_and_event_directions_cannot_be_confused() {
    let event = serde_json::to_string(&WireMessage::Event(WireEvent::SyncSummary {
        device_id: device_id(),
        session_id: session_id(),
        summary: ActionPlanSummary {
            add: 1,
            modify: 2,
            metadata_only: 3,
            remove: 4,
            unchanged: 5,
            total_planned: 10,
        },
    }))
    .unwrap();
    let command = serde_json::to_string(&WireMessage::Command(WireCommand::CancelSync {
        device_id: device_id(),
        session_id: session_id(),
        request_id: request_id(),
    }))
    .unwrap();

    assert!(
        decode_admitted_message(&event, &AdmittedStream::DaemonReceivingDesktopCommands).is_err()
    );
    assert!(
        decode_admitted_message(&command, &AdmittedStream::DesktopReceivingDaemonEvents).is_err()
    );
}

#[test]
fn daemon_only_failures_cannot_be_spoofed_by_a_worker() {
    let json = serde_json::to_string(&WireMessage::Event(WireEvent::CommandFailed {
        request_id: request_id(),
        message: "not accepted".to_owned(),
    }))
    .unwrap();
    assert!(decode_admitted_message(&json, &worker_events()).is_err());
    assert!(decode_admitted_message(&json, &AdmittedStream::DesktopReceivingDaemonEvents).is_ok());
}

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn session_id() -> SessionId {
    SessionId::new(42).unwrap()
}

fn request_id() -> RequestId {
    RequestId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8750").unwrap()
}

fn assert_golden_lines(ndjson: &str, stream: AdmittedStream, expected_types: &[&str]) {
    let lines = ndjson.lines().collect::<Vec<_>>();
    let actual_types = lines
        .iter()
        .map(|line| message_type(line))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_types,
        expected_types
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    );

    for line in lines {
        let DecodedWireMessage::Known(message) = decode_admitted_message(line, &stream).unwrap()
        else {
            panic!("golden vector was treated as unknown: {line}");
        };
        assert_eq!(serde_json::to_string(&message).unwrap(), line);
    }
}

fn route() -> OwnedSessionRoute {
    OwnedSessionRoute::new(device_id(), session_id())
}

fn desktop_events() -> AdmittedStream {
    AdmittedStream::DesktopReceivingDaemonEvents
}

fn worker_events() -> AdmittedStream {
    AdmittedStream::DaemonReceivingWorkerEvents(route())
}

fn worker_commands(pending_interaction: PendingWorkerInteraction) -> AdmittedStream {
    worker_commands_for(route(), pending_interaction)
}

fn worker_commands_for(
    route: OwnedSessionRoute,
    pending_interaction: PendingWorkerInteraction,
) -> AdmittedStream {
    AdmittedStream::WorkerReceivingDaemonCommands(WorkerCommandAdmission::new(
        route,
        pending_interaction,
    ))
}

fn message_type(json: &str) -> String {
    serde_json::from_str::<Value>(json).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn read_negative(path: &str) -> &'static str {
    match path {
        "progress/negative/track-done-missing-device.json" => {
            include_str!("data/wire-v3/progress/negative/track-done-missing-device.json")
        }
        "progress/negative/track-done-malformed-device.json" => {
            include_str!("data/wire-v3/progress/negative/track-done-malformed-device.json")
        }
        "progress/negative/track-done-wrong-device.json" => {
            include_str!("data/wire-v3/progress/negative/track-done-wrong-device.json")
        }
        "progress/negative/track-done-wrong-session.json" => {
            include_str!("data/wire-v3/progress/negative/track-done-wrong-session.json")
        }
        "progress/negative/cancel-wrong-direction.json" => {
            include_str!("data/wire-v3/progress/negative/cancel-wrong-direction.json")
        }
        "progress/negative/track-start-invalid-position.json" => {
            include_str!("data/wire-v3/progress/negative/track-start-invalid-position.json")
        }
        "progress/negative/prompt-empty-options.json" => {
            include_str!("data/wire-v3/progress/negative/prompt-empty-options.json")
        }
        "progress/negative/prompt-decision-out-of-range.json" => {
            include_str!("data/wire-v3/progress/negative/prompt-decision-out-of-range.json")
        }
        _ => panic!("manifest references unknown progress vector {path}"),
    }
}
