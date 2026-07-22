use classick::wire::{
    decode_admitted_message, decode_initial_hello, validate_peer_hello, AdmittedStream,
    CapabilityName, DecodedWireMessage, EndpointRole, WireHello, WireMessage,
    WIRE_PROTOCOL_VERSION,
};
use serde::Deserialize;

const MANIFEST: &str = include_str!("data/wire-v3/manifest.json");

#[derive(Deserialize)]
struct VectorManifest {
    vectors: Vec<VectorCase>,
}

#[derive(Deserialize)]
struct VectorCase {
    path: String,
    expectation: String,
    expected_role: Option<EndpointRole>,
    required_capabilities: Option<Vec<CapabilityName>>,
}

#[test]
fn valid_hellos_match_language_neutral_golden_shapes() {
    for (role, path, capabilities) in [
        (
            EndpointRole::Desktop,
            "valid/hello-desktop.json",
            vec![capability("config_mutation")],
        ),
        (
            EndpointRole::Daemon,
            "valid/hello-daemon.json",
            vec![
                capability("typed_sync_progress"),
                capability("device_inventory"),
                capability("portable_profile"),
            ],
        ),
        (
            EndpointRole::Worker,
            "valid/hello-worker.json",
            vec![capability("typed_sync_progress")],
        ),
    ] {
        let hello = WireHello::new(role, "0.0.1", capabilities).unwrap();
        let message = WireMessage::Hello(hello.clone());
        let golden = read_vector(path);
        assert_eq!(serde_json::to_string(&message).unwrap(), golden.trim());
        assert_eq!(decode_initial_hello(golden).unwrap(), hello);
    }
}

#[test]
fn manifest_vectors_have_unambiguous_cross_language_expectations() {
    let manifest: VectorManifest = serde_json::from_str(MANIFEST).unwrap();
    for case in manifest.vectors {
        let json = read_vector(&case.path);
        match case.expectation.as_str() {
            "valid_hello" => {
                let hello = decode_initial_hello(json).unwrap();
                validate_peer_hello(
                    &hello,
                    case.expected_role.unwrap(),
                    &case.required_capabilities.unwrap_or_default(),
                )
                .unwrap();
            }
            "admission_failure" => {
                let hello = decode_initial_hello(json).unwrap();
                assert!(validate_peer_hello(
                    &hello,
                    case.expected_role.unwrap(),
                    &case.required_capabilities.unwrap_or_default(),
                )
                .is_err());
            }
            "decode_failure" => assert!(decode_initial_hello(json).is_err()),
            "canonicalize_hello" => {
                let hello = decode_initial_hello(json).unwrap();
                let canonical = serde_json::to_string(&WireMessage::Hello(hello)).unwrap();
                assert_eq!(canonical, read_vector("valid/hello-daemon.json").trim());
            }
            "ignored_desktop_event" => assert!(matches!(
                decode_admitted_message(json, &AdmittedStream::DesktopReceivingDaemonEvents),
                Ok(DecodedWireMessage::IgnoredUnknownEvent { .. })
            )),
            other => panic!("unknown vector expectation {other}"),
        }
    }
}

#[test]
fn unknown_additive_messages_are_ignored_only_on_the_desktop_event_stream() {
    let json = read_vector("compatibility/unknown-daemon-event.json");
    assert_eq!(
        decode_admitted_message(json, &AdmittedStream::DesktopReceivingDaemonEvents).unwrap(),
        DecodedWireMessage::IgnoredUnknownEvent {
            message_type: "future_device_hint".to_owned()
        }
    );
    assert!(
        decode_admitted_message(json, &AdmittedStream::DaemonReceivingDesktopCommands).is_err()
    );
}

#[test]
fn malformed_envelopes_and_repeated_hello_are_never_ignored() {
    for json in [
        "[]",
        r#"{"future":true}"#,
        r#"{"type":42}"#,
        r#"{"type":""}"#,
        read_vector("valid/hello-daemon.json"),
    ] {
        assert!(
            decode_admitted_message(json, &AdmittedStream::DesktopReceivingDaemonEvents).is_err()
        );
    }
}

#[test]
fn semantic_versions_accept_standard_suffixes_and_reject_ambiguous_forms() {
    WireHello::new(EndpointRole::Daemon, "3.0.0-beta.1+build.7", []).unwrap();
    for json in [
        read_vector("decode-failure/hello-leading-zero-version.json"),
        read_vector("decode-failure/hello-empty-prerelease.json"),
        read_vector("decode-failure/hello-malformed-software-version.json"),
    ] {
        assert!(decode_initial_hello(json).is_err());
    }
}

#[test]
fn protocol_constant_is_the_frozen_breaking_major() {
    assert_eq!(WIRE_PROTOCOL_VERSION, "3.0.0");
}

fn capability(value: &str) -> CapabilityName {
    CapabilityName::parse(value).unwrap()
}

fn read_vector(path: &str) -> &'static str {
    match path {
        "valid/hello-desktop.json" => include_str!("data/wire-v3/valid/hello-desktop.json"),
        "valid/hello-daemon.json" => include_str!("data/wire-v3/valid/hello-daemon.json"),
        "valid/hello-worker.json" => include_str!("data/wire-v3/valid/hello-worker.json"),
        "admission-failure/hello-old-major.json" => {
            include_str!("data/wire-v3/admission-failure/hello-old-major.json")
        }
        "admission-failure/hello-wrong-role.json" => {
            include_str!("data/wire-v3/admission-failure/hello-wrong-role.json")
        }
        "decode-failure/hello-duplicate-capability.json" => {
            include_str!("data/wire-v3/decode-failure/hello-duplicate-capability.json")
        }
        "decode-failure/hello-leading-zero-version.json" => {
            include_str!("data/wire-v3/decode-failure/hello-leading-zero-version.json")
        }
        "decode-failure/hello-empty-prerelease.json" => {
            include_str!("data/wire-v3/decode-failure/hello-empty-prerelease.json")
        }
        "decode-failure/hello-malformed-software-version.json" => {
            include_str!("data/wire-v3/decode-failure/hello-malformed-software-version.json")
        }
        "canonicalize/hello-unsorted-capabilities.json" => {
            include_str!("data/wire-v3/canonicalize/hello-unsorted-capabilities.json")
        }
        "compatibility/unknown-daemon-event.json" => {
            include_str!("data/wire-v3/compatibility/unknown-daemon-event.json")
        }
        _ => panic!("manifest references uncompiled vector {path}"),
    }
}
