use super::{DeviceId, Fact, FactConfidence, FactSource, HardwareFacts, IpodColour, IpodFamily};

#[test]
fn parses_hexadecimal_input_to_canonical_uppercase() {
    let id: DeviceId = "a1b2c3d4e5f60708".parse().unwrap();
    assert_eq!(id.to_string(), "A1B2C3D4E5F60708");

    let prefixed: DeviceId = "0Xabcdef0123456789".parse().unwrap();
    assert_eq!(prefixed.to_string(), "ABCDEF0123456789");
}

#[test]
fn rejects_malformed_input_without_trimming() {
    for value in [
        "",
        "0123456789ABCDE",
        "0123456789ABCDEF0",
        " 0123456789ABCDEF",
        "0123456789ABCDEF ",
        "0123456789ABCDEG",
        "0123-456789ABCDEF",
        "0x0123456789ABCDEG",
    ] {
        assert!(value.parse::<DeviceId>().is_err(), "accepted {value:?}");
    }
}

#[test]
fn serde_serialization_emits_canonical_value() {
    let id: DeviceId = "0xa1b2c3d4e5f60708".parse().unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"A1B2C3D4E5F60708\"");
}

#[test]
fn serde_deserialization_normalizes_valid_input() {
    let decoded: DeviceId = serde_json::from_str("\"0Xabcdef0123456789\"").unwrap();
    assert_eq!(decoded.to_string(), "ABCDEF0123456789");
}

#[test]
fn serde_deserialization_rejects_malformed_input() {
    for value in [
        "",
        "0123456789ABCDE",
        "0123456789ABCDEG",
        " ABCDEF0123456789",
    ] {
        let json = serde_json::to_string(value).unwrap();
        assert!(
            serde_json::from_str::<DeviceId>(&json).is_err(),
            "accepted {value:?}"
        );
    }
}

#[test]
fn fact_constructors_encode_their_provenance_and_confidence() {
    let reported = Fact::reported("1.3");
    assert_eq!(reported.source, FactSource::Reported);
    assert_eq!(reported.confidence, FactConfidence::Certain);

    let decoded = Fact::decoded(IpodFamily::Classic);
    assert_eq!(decoded.source, FactSource::Decoded);
    assert_eq!(decoded.confidence, FactConfidence::Certain);

    let inferred = Fact::inferred(160_000_000_000_u64);
    assert_eq!(inferred.source, FactSource::Inferred);
    assert_eq!(inferred.confidence, FactConfidence::Heuristic);
}

#[test]
fn default_hardware_facts_contain_no_claims() {
    let facts = HardwareFacts::default();

    assert_eq!(facts.family, None);
    assert_eq!(facts.generation, None);
    assert_eq!(facts.model_code, None);
    assert_eq!(facts.colour, None);
    assert_eq!(facts.firmware, None);
    assert_eq!(facts.capacity_bytes, None);
}

#[test]
fn exact_classic_facts_round_trip_with_snake_case_json() {
    let facts = HardwareFacts {
        family: Some(Fact::decoded(IpodFamily::Classic)),
        generation: Some(Fact::decoded("late_2009".to_owned())),
        model_code: Some(Fact::reported("MC297".to_owned())),
        colour: Some(Fact::decoded(IpodColour::Black)),
        firmware: Some(Fact::reported("2.0.5".to_owned())),
        capacity_bytes: Some(Fact::reported(160_000_000_000)),
    };

    let json = serde_json::to_string(&facts).unwrap();
    assert_eq!(
        json,
        r#"{"family":{"value":"classic","source":"decoded","confidence":"certain"},"generation":{"value":"late_2009","source":"decoded","confidence":"certain"},"model_code":{"value":"MC297","source":"reported","confidence":"certain"},"colour":{"value":"black","source":"decoded","confidence":"certain"},"firmware":{"value":"2.0.5","source":"reported","confidence":"certain"},"capacity_bytes":{"value":160000000000,"source":"reported","confidence":"certain"}}"#
    );
    assert_eq!(serde_json::from_str::<HardwareFacts>(&json).unwrap(), facts);
}

#[test]
fn absent_hardware_facts_are_omitted_without_inventing_an_appearance() {
    let facts = HardwareFacts {
        firmware: Some(Fact::reported("2.0.5".to_owned())),
        ..HardwareFacts::default()
    };

    assert_eq!(
        serde_json::to_string(&facts).unwrap(),
        r#"{"firmware":{"value":"2.0.5","source":"reported","confidence":"certain"}}"#
    );
    assert_eq!(facts.family, None);
    assert_eq!(facts.colour, None);
}
