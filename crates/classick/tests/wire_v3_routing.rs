use classick::device::DeviceId;
use classick::wire::{PromptId, RequestId, SessionId};

#[test]
fn request_ids_are_non_nil_lowercase_uuids() {
    let value = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8750";
    let id = RequestId::parse(value).unwrap();
    assert_eq!(id.as_str(), value);
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{value}\""));
    assert_eq!(
        serde_json::from_str::<RequestId>(&format!("\"{value}\"")).unwrap(),
        id
    );

    for invalid in [
        "",
        "018F9D7E-2F2B-7B52-9F1D-F78BDB2F8750",
        "00000000-0000-0000-0000-000000000000",
        "018f9d7e2f2b7b529f1df78bdb2f8750",
        "not-a-uuid",
    ] {
        assert!(RequestId::parse(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn session_and_prompt_ids_are_positive_json_integers() {
    let session = SessionId::new(42).unwrap();
    let prompt = PromptId::new(7).unwrap();
    assert_eq!(session.get(), 42);
    assert_eq!(prompt.get(), 7);
    assert_eq!(serde_json::to_string(&session).unwrap(), "42");
    assert_eq!(serde_json::from_str::<PromptId>("7").unwrap(), prompt);

    assert!(SessionId::new(0).is_err());
    assert!(serde_json::from_str::<PromptId>("0").is_err());
    assert!(serde_json::from_str::<SessionId>("-1").is_err());
    assert!(serde_json::from_str::<SessionId>("1.5").is_err());
}

#[test]
fn device_ids_are_canonical_on_the_wire_but_migration_parsing_stays_flexible() {
    let migrated = DeviceId::parse("0x000a27002138b0a8").unwrap();
    assert_eq!(migrated.as_str(), "000A27002138B0A8");
    assert_eq!(
        serde_json::from_str::<DeviceId>("\"000A27002138B0A8\"").unwrap(),
        migrated
    );
    for invalid_wire in [
        "\"0x000A27002138B0A8\"",
        "\"000a27002138b0a8\"",
        "\"000A27002138B0A\"",
    ] {
        assert!(serde_json::from_str::<DeviceId>(invalid_wire).is_err());
    }
}
