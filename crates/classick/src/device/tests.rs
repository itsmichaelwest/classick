use super::DeviceId;

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
