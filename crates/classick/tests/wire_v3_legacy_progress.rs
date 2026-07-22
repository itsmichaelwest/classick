use classick::device::DeviceId;
use classick::wire::{
    LegacyScanDecoder, LegacyWorkerDecoder, RequestId, SessionId, WireEvent, WireMessage,
};
use serde_json::{json, Value};

const HELLO: &str = r#"{"type":"hello","protocol_version":"1.4.0","core_version":"0.0.1"}"#;
const REQUEST_ID: &str = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808";

#[test]
fn normal_worker_stream_preserves_every_progress_field() {
    let mut decoder = admitted_worker();
    let cases = [
        (
            r#"{"type":"header","source":"/Music","ipod":"/Volumes/iPod","manifest":"/state/manifest.json"}"#,
            json!({"type":"run_header","device_id":"000A27002138B0A8","session_id":42,"source":"/Music","ipod":"/Volumes/iPod","manifest":"/state/manifest.json"}),
        ),
        (
            r#"{"type":"summary","add":1,"modify":2,"metadata_only":3,"remove":4,"unchanged":5,"total_planned":10}"#,
            json!({"type":"sync_summary","device_id":"000A27002138B0A8","session_id":42,"summary":{"add":1,"modify":2,"metadata_only":3,"remove":4,"unchanged":5,"total_planned":10}}),
        ),
        (
            r#"{"type":"review","summary":{"add":1,"modify":2,"metadata_only":3,"remove":4,"unchanged":5},"no_delete":false}"#,
            json!({"type":"review_requested","device_id":"000A27002138B0A8","session_id":42,"summary":{"add":1,"modify":2,"metadata_only":3,"remove":4,"unchanged":5,"total_planned":10},"no_delete":false}),
        ),
        (
            r#"{"type":"prompt","id":7,"message":"Retry?","options":["Retry","Abort"]}"#,
            json!({"type":"prompt","device_id":"000A27002138B0A8","session_id":42,"prompt_id":7,"message":"Retry?","options":["Retry","Abort"]}),
        ),
        (
            r#"{"type":"form","id":8,"label":"Name","initial":"iPod","hint":"Required"}"#,
            json!({"type":"form","device_id":"000A27002138B0A8","session_id":42,"prompt_id":8,"label":"Name","initial":"iPod","hint":"Required"}),
        ),
        (
            r#"{"type":"track_start","current":1,"total":2,"label":"First"}"#,
            json!({"type":"track_start","device_id":"000A27002138B0A8","session_id":42,"current":1,"total":2,"label":"First"}),
        ),
        (
            r#"{"type":"track_done","result":"applied"}"#,
            json!({"type":"track_done","device_id":"000A27002138B0A8","session_id":42,"result":"applied"}),
        ),
        (
            r#"{"type":"track_start","current":2,"total":2,"label":"Second","eta_secs":5}"#,
            json!({"type":"track_start","device_id":"000A27002138B0A8","session_id":42,"current":2,"total":2,"label":"Second","eta_secs":5}),
        ),
        (
            r#"{"type":"track_done","result":"skipped"}"#,
            json!({"type":"track_done","device_id":"000A27002138B0A8","session_id":42,"result":"skipped"}),
        ),
        (
            r#"{"type":"log","message":"Working"}"#,
            json!({"type":"sync_log","device_id":"000A27002138B0A8","session_id":42,"message":"Working"}),
        ),
        (
            r#"{"type":"error","message":"Recoverable","recovery_hints":["Retry"]}"#,
            json!({"type":"sync_error","device_id":"000A27002138B0A8","session_id":42,"message":"Recoverable","recovery_hints":["Retry"]}),
        ),
        (
            r#"{"type":"error","message":"No hints"}"#,
            json!({"type":"sync_error","device_id":"000A27002138B0A8","session_id":42,"message":"No hints"}),
        ),
        (
            r#"{"type":"finish","success":true,"skipped_for_space":{"albums":1,"tracks":2,"bytes":3},"artwork":{"embedded":4,"eligible":5,"failed_sources":1},"db_restored":true}"#,
            json!({"type":"sync_finished","device_id":"000A27002138B0A8","session_id":42,"success":true,"skipped_for_space":{"albums":1,"tracks":2,"bytes":3},"artwork":{"embedded":4,"eligible":5,"failed_sources":1},"db_restored":true}),
        ),
    ];

    for (line, expected) in cases {
        assert_eq!(decode_worker(&mut decoder, line), expected);
    }
    decoder.on_eof().unwrap();
}

#[test]
fn worker_terminal_variants_require_the_documented_order() {
    for (reason, outcome, expected_type) in [
        ("cancelled", r#"{"type":"cancelled"}"#, "sync_cancelled"),
        ("paused", r#"{"type":"paused"}"#, "sync_paused"),
    ] {
        let mut decoder = admitted_worker();
        let finalizing = format!(
            r#"{{"type":"finalizing","reason":"{reason}","staged_albums":1,"staged_tracks":2}}"#
        );
        assert_eq!(
            decode_worker(&mut decoder, &finalizing),
            json!({"type":"finalizing","device_id":"000A27002138B0A8","session_id":42,"reason":reason,"staged_albums":1,"staged_tracks":2})
        );
        assert_eq!(
            decode_worker(&mut decoder, r#"{"type":"log","message":"Publishing"}"#),
            json!({"type":"sync_log","device_id":"000A27002138B0A8","session_id":42,"message":"Publishing"})
        );
        assert_eq!(decode_worker(&mut decoder, outcome)["type"], expected_type);
        assert_eq!(
            decode_worker(&mut decoder, r#"{"type":"finish","success":true}"#),
            json!({"type":"sync_finished","device_id":"000A27002138B0A8","session_id":42,"success":true})
        );
        decoder.on_eof().unwrap();
    }

    let mut failed = admitted_worker();
    assert_eq!(
        decode_worker(
            &mut failed,
            r#"{"type":"error","message":"Publish failed"}"#
        ),
        json!({"type":"sync_error","device_id":"000A27002138B0A8","session_id":42,"message":"Publish failed"})
    );
    assert_eq!(
        decode_worker(&mut failed, r#"{"type":"finish","success":false}"#),
        json!({"type":"sync_finished","device_id":"000A27002138B0A8","session_id":42,"success":false})
    );
    failed.on_eof().unwrap();

    for reason in ["cancelled", "paused"] {
        let mut failed_finalization = admitted_worker();
        failed_finalization
            .decode(&format!(
                r#"{{"type":"finalizing","reason":"{reason}","staged_albums":1,"staged_tracks":2}}"#
            ))
            .unwrap();
        assert_eq!(
            decode_worker(
                &mut failed_finalization,
                r#"{"type":"error","message":"Publication failed"}"#
            ),
            json!({"type":"sync_error","device_id":"000A27002138B0A8","session_id":42,"message":"Publication failed"})
        );
        assert_eq!(
            decode_worker(
                &mut failed_finalization,
                r#"{"type":"finish","success":false}"#
            ),
            json!({"type":"sync_finished","device_id":"000A27002138B0A8","session_id":42,"success":false})
        );
        failed_finalization.on_eof().unwrap();
    }
}

#[test]
fn worker_rejects_incomplete_or_contradictory_lifecycles() {
    let mut missing_hello = worker();
    assert!(missing_hello
        .decode(r#"{"type":"log","message":"Working"}"#)
        .is_err());

    let mut second_hello = admitted_worker();
    assert!(second_hello.decode(HELLO).is_err());

    for invalid in [
        r#"{"type":"prompt","id":0,"message":"Retry?","options":["Retry"]}"#,
        r#"{"type":"summary","add":1,"modify":0,"metadata_only":0,"remove":0,"unchanged":0,"total_planned":2}"#,
        r#"{"type":"finish","success":true,"skipped_for_space":{"albums":0,"tracks":1,"bytes":1}}"#,
        r#"{"type":"finish","success":false}"#,
        r#"{"type":"cancelled"}"#,
        r#"{"type":"paused"}"#,
    ] {
        assert!(admitted_worker().decode(invalid).is_err(), "{invalid}");
    }

    let mut contradictory = admitted_worker();
    contradictory
        .decode(r#"{"type":"finalizing","reason":"paused","staged_albums":0,"staged_tracks":0}"#)
        .unwrap();
    assert!(contradictory.decode(r#"{"type":"cancelled"}"#).is_err());

    let unfinished = admitted_worker();
    assert!(unfinished.on_eof().is_err());

    let mut finished = admitted_worker();
    finished
        .decode(r#"{"type":"finish","success":true}"#)
        .unwrap();
    assert!(finished
        .decode(r#"{"type":"log","message":"late"}"#)
        .is_err());
    assert!(finished
        .decode(r#"{"type":"finish","success":true}"#)
        .is_err());

    let mut graceful_failure = admitted_worker();
    graceful_failure
        .decode(r#"{"type":"finalizing","reason":"paused","staged_albums":0,"staged_tracks":0}"#)
        .unwrap();
    graceful_failure.decode(r#"{"type":"paused"}"#).unwrap();
    assert!(graceful_failure
        .decode(r#"{"type":"finish","success":false}"#)
        .is_err());
}

#[test]
fn scan_stream_uses_global_request_routing_and_scan_events() {
    let mut decoder = LegacyScanDecoder::new(Some(request_id()), SessionId::new(43).unwrap());
    assert_eq!(
        decoder.decode(HELLO).unwrap().map(event_value),
        Some(json!({"type":"library_scan_started","request_id":REQUEST_ID,"session_id":43}))
    );
    let cases = [
        (
            r#"{"type":"header","source":"/Music","ipod":"","manifest":"/state/library-index.json"}"#,
            None,
        ),
        (
            r#"{"type":"summary","add":2,"modify":0,"metadata_only":0,"remove":0,"unchanged":3,"total_planned":2}"#,
            Some(
                json!({"type":"library_scan_progress","request_id":REQUEST_ID,"session_id":43,"files_scanned":5,"tracks_indexed":0}),
            ),
        ),
        (
            r#"{"type":"track_start","current":1,"total":2,"label":"One.flac"}"#,
            None,
        ),
        (
            r#"{"type":"track_done","result":"applied"}"#,
            Some(
                json!({"type":"library_scan_progress","request_id":REQUEST_ID,"session_id":43,"files_scanned":5,"tracks_indexed":1}),
            ),
        ),
        (
            r#"{"type":"track_start","current":2,"total":2,"label":"Two.flac"}"#,
            None,
        ),
        (
            r#"{"type":"track_done","result":"applied"}"#,
            Some(
                json!({"type":"library_scan_progress","request_id":REQUEST_ID,"session_id":43,"files_scanned":5,"tracks_indexed":2}),
            ),
        ),
        (r#"{"type":"log","message":"scan: probed=2"}"#, None),
        (
            r#"{"type":"finish","success":true}"#,
            Some(
                json!({"type":"library_scan_finished","request_id":REQUEST_ID,"session_id":43,"success":true}),
            ),
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(
            decoder.decode(line).unwrap().map(event_value),
            expected,
            "{line}"
        );
    }
    decoder.on_eof().unwrap();

    let mut failed = admitted_scan();
    failed
        .decode(r#"{"type":"header","source":"/Music","ipod":"","manifest":"/state/library-index.json"}"#)
        .unwrap();
    assert!(failed
        .decode(r#"{"type":"error","message":"Index failed"}"#)
        .unwrap()
        .is_none());
    assert_eq!(
        failed
            .decode(r#"{"type":"finish","success":false}"#)
            .unwrap()
            .map(event_value),
        Some(
            json!({"type":"library_scan_finished","request_id":REQUEST_ID,"session_id":43,"success":false,"message":"Index failed"})
        )
    );
    failed.on_eof().unwrap();

    let mut early_failure = admitted_scan();
    assert!(early_failure
        .decode(r#"{"type":"error","message":"Source unavailable"}"#)
        .unwrap()
        .is_none());
    assert_eq!(
        early_failure
            .decode(r#"{"type":"finish","success":false}"#)
            .unwrap()
            .map(event_value),
        Some(
            json!({"type":"library_scan_finished","request_id":REQUEST_ID,"session_id":43,"success":false,"message":"Source unavailable"})
        )
    );
    early_failure.on_eof().unwrap();
}

#[test]
fn scan_decoder_rejects_device_sync_shapes_and_invalid_termination() {
    let mut device_header = admitted_scan();
    assert!(device_header
        .decode(r#"{"type":"header","source":"/Music","ipod":"/Volumes/iPod","manifest":"/state/library-index.json"}"#)
        .is_err());

    for invalid in [
        r#"{"type":"summary","add":1,"modify":0,"metadata_only":0,"remove":0,"unchanged":0,"total_planned":1}"#,
        r#"{"type":"prompt","id":1,"message":"Retry?","options":["Retry"]}"#,
        r#"{"type":"finish","success":false}"#,
    ] {
        assert!(admitted_scan().decode(invalid).is_err(), "{invalid}");
    }
    assert!(admitted_scan().on_eof().is_err());

    let mut truncated = admitted_scan();
    truncated
        .decode(r#"{"type":"header","source":"/Music","ipod":"","manifest":"/state/library-index.json"}"#)
        .unwrap();
    truncated
        .decode(r#"{"type":"summary","add":1,"modify":0,"metadata_only":0,"remove":0,"unchanged":2,"total_planned":1}"#)
        .unwrap();
    assert!(truncated
        .decode(r#"{"type":"finish","success":true}"#)
        .is_err());
}

fn worker() -> LegacyWorkerDecoder {
    LegacyWorkerDecoder::new(
        DeviceId::parse("000A27002138B0A8").unwrap(),
        SessionId::new(42).unwrap(),
    )
}

fn admitted_worker() -> LegacyWorkerDecoder {
    let mut decoder = worker();
    assert!(decoder.decode(HELLO).unwrap().is_none());
    decoder
}

fn admitted_scan() -> LegacyScanDecoder {
    let mut decoder = LegacyScanDecoder::new(Some(request_id()), SessionId::new(43).unwrap());
    assert_eq!(
        decoder.decode(HELLO).unwrap().map(event_value),
        Some(json!({"type":"library_scan_started","request_id":REQUEST_ID,"session_id":43}))
    );
    decoder
}

fn request_id() -> RequestId {
    RequestId::parse(REQUEST_ID).unwrap()
}

fn decode_worker(decoder: &mut LegacyWorkerDecoder, line: &str) -> Value {
    event_value(decoder.decode(line).unwrap().unwrap())
}

fn event_value(event: WireEvent) -> Value {
    serde_json::to_value(WireMessage::Event(event)).unwrap()
}
