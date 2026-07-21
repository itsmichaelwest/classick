use super::{
    hardware_facts_from_decoded_model_code, hardware_facts_from_reported_model_code,
    hardware_facts_from_usb, Fact, HardwareFacts, IpodColour, IpodFamily,
    HARDWARE_CATALOGUE_VERSION,
};

#[test]
fn catalogue_has_stable_version() {
    assert_eq!(HARDWARE_CATALOGUE_VERSION, 1);
}

#[test]
fn classic_model_codes_decode_every_catalogue_row_and_input_shape() {
    let cases = [
        ("MB029", "B029", "1", IpodColour::Silver),
        ("MB147", "B147", "1", IpodColour::Black),
        ("MB145", "B145", "1", IpodColour::Silver),
        ("MB150", "B150", "1", IpodColour::Black),
        ("MB562", "B562", "2", IpodColour::Silver),
        ("MB565", "B565", "2", IpodColour::Black),
        ("MC293", "C293", "3", IpodColour::Silver),
        ("MC297", "C297", "3", IpodColour::Black),
    ];

    for (canonical, abbreviated, generation, colour) in cases {
        let expected = HardwareFacts {
            family: Some(Fact::decoded(IpodFamily::Classic)),
            generation: Some(Fact::decoded(generation.to_owned())),
            model_code: Some(Fact::reported(canonical.to_owned())),
            colour: Some(Fact::decoded(colour)),
            ..HardwareFacts::default()
        };

        assert_eq!(
            hardware_facts_from_reported_model_code(canonical),
            Some(expected.clone()),
            "canonical code {canonical}"
        );
        assert_eq!(
            hardware_facts_from_reported_model_code(&abbreviated.to_ascii_lowercase()),
            Some(expected),
            "abbreviated code {abbreviated}"
        );
    }
}

#[test]
fn classic_model_codes_are_ascii_case_insensitive() {
    let facts = hardware_facts_from_reported_model_code("mC293").unwrap();

    assert_eq!(facts.model_code, Some(Fact::reported("MC293".to_owned())));
}

#[test]
fn model_code_entry_points_preserve_provenance() {
    let reported = hardware_facts_from_reported_model_code("MC297").unwrap();
    let decoded = hardware_facts_from_decoded_model_code("MC297").unwrap();

    assert_eq!(
        reported.model_code,
        Some(Fact::reported("MC297".to_owned()))
    );
    assert_eq!(decoded.model_code, Some(Fact::decoded("MC297".to_owned())));
    assert_eq!(reported.family, Some(Fact::decoded(IpodFamily::Classic)));
    assert_eq!(decoded.family, Some(Fact::decoded(IpodFamily::Classic)));
    assert_eq!(reported.generation, Some(Fact::decoded("3".to_owned())));
    assert_eq!(decoded.generation, Some(Fact::decoded("3".to_owned())));
    assert_eq!(reported.colour, Some(Fact::decoded(IpodColour::Black)));
    assert_eq!(decoded.colour, Some(Fact::decoded(IpodColour::Black)));
}

#[test]
fn model_code_lookup_rejects_unknown_or_malformed_input() {
    for code in [
        "", "MA001", "MB02", "MMB029", " MB029", "MB029 ", "B029\n", "MB02!",
    ] {
        assert_eq!(
            hardware_facts_from_reported_model_code(code),
            None,
            "reported code {code:?}"
        );
        assert_eq!(
            hardware_facts_from_decoded_model_code(code),
            None,
            "decoded code {code:?}"
        );
    }
}

#[test]
fn model_catalogue_never_claims_marketed_capacity_as_bytes() {
    for code in [
        "MB029", "MB147", "MB145", "MB150", "MB562", "MB565", "MC293", "MC297",
    ] {
        let facts = hardware_facts_from_reported_model_code(code).unwrap();
        assert_eq!(facts.capacity_bytes, None, "model code {code}");
    }
}

#[test]
fn classic_usb_capacity_inference_is_bounded_and_ambiguous_at_160_gb() {
    let cases = [
        (Some(80_000_000_000), Some(Fact::inferred("1".to_owned()))),
        (Some(120_000_000_000), Some(Fact::inferred("2".to_owned()))),
        (Some(160_000_000_000), None),
        (None, None),
    ];

    for (capacity, generation) in cases {
        let facts = hardware_facts_from_usb(0x1261, capacity);
        assert_eq!(facts.family, Some(Fact::decoded(IpodFamily::Classic)));
        assert_eq!(facts.generation, generation);
        assert_eq!(facts.capacity_bytes, capacity.map(Fact::reported));
        assert_usb_has_no_exact_variant(&facts);
    }
}

#[test]
fn classic_usb_capacity_thresholds_use_decimal_gigabytes() {
    let cases = [
        (99_999_999_999, Some(Fact::inferred("1".to_owned()))),
        (100_000_000_000, Some(Fact::inferred("2".to_owned()))),
        (139_999_999_999, Some(Fact::inferred("2".to_owned()))),
        (140_000_000_000, None),
    ];

    for (capacity, generation) in cases {
        let facts = hardware_facts_from_usb(0x1261, Some(capacity));
        assert_eq!(facts.generation, generation, "capacity {capacity}");
    }
}

#[test]
fn usb_catalogue_decodes_every_unambiguous_family_and_generation() {
    let cases = [
        (0x1240, IpodFamily::Nano, "1"),
        (0x1260, IpodFamily::Nano, "2"),
        (0x1262, IpodFamily::Nano, "3"),
        (0x1263, IpodFamily::Nano, "4"),
        (0x1265, IpodFamily::Nano, "5"),
        (0x1266, IpodFamily::Nano, "6"),
        (0x1267, IpodFamily::Nano, "7"),
        (0x1209, IpodFamily::Video, "5"),
        (0x1206, IpodFamily::Video, "5.5"),
        (0x1201, IpodFamily::Ipod, "3"),
        (0x1203, IpodFamily::Ipod, "4"),
        (0x1300, IpodFamily::Shuffle, "1"),
        (0x1301, IpodFamily::Shuffle, "2"),
        (0x1302, IpodFamily::Shuffle, "3"),
        (0x1303, IpodFamily::Shuffle, "4"),
    ];

    for (pid, family, generation) in cases {
        let facts = hardware_facts_from_usb(pid, None);
        assert_eq!(facts.family, Some(Fact::decoded(family)), "PID {pid:#06x}");
        assert_eq!(
            facts.generation,
            Some(Fact::decoded(generation.to_owned())),
            "PID {pid:#06x}"
        );
        assert_eq!(facts.capacity_bytes, None);
        assert_usb_has_no_exact_variant(&facts);
    }
}

#[test]
fn usb_catalogue_keeps_shared_or_unnumbered_generations_absent() {
    let cases = [
        (0x1205, IpodFamily::Mini),
        (0x1204, IpodFamily::Photo),
        (0x1202, IpodFamily::Ipod),
    ];

    for (pid, family) in cases {
        let facts = hardware_facts_from_usb(pid, None);
        assert_eq!(facts.family, Some(Fact::decoded(family)), "PID {pid:#06x}");
        assert_eq!(facts.generation, None, "PID {pid:#06x}");
        assert_usb_has_no_exact_variant(&facts);
    }
}

#[test]
fn unknown_usb_pid_preserves_only_reported_capacity() {
    let facts = hardware_facts_from_usb(0xffff, Some(123_456_789));

    assert_eq!(
        facts,
        HardwareFacts {
            capacity_bytes: Some(Fact::reported(123_456_789)),
            ..HardwareFacts::default()
        }
    );
}

fn assert_usb_has_no_exact_variant(facts: &HardwareFacts) {
    assert_eq!(facts.model_code, None);
    assert_eq!(facts.colour, None);
}
