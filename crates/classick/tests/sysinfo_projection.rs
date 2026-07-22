use classick::device::DeviceId;
use classick::ipod::{
    decide_sysinfo_extended, project_sysinfo_extended, CapabilityProfile, ImageFormat,
    SysInfoExtendedDecision,
};
use classick::portable::profile::ContentHash;
use plist::{Dictionary, Value};
use std::collections::BTreeSet;

const CAPABILITY_FIXTURE: &str =
    include_str!("fixtures/device-capabilities/classic-late-2009-v1.json");
const LATE_2009_PROJECTION_HASH: &str =
    "6b143e08ca34df8ab9ac50957fe927c46fb516c0af7c110ead8a78c6a39af453";

#[test]
fn projects_the_complete_validated_profile_to_deterministic_golden_bytes() {
    let device_id = DeviceId::parse("0x000a27002138b0a8").unwrap();
    let profile = CapabilityProfile::from_json(CAPABILITY_FIXTURE).unwrap();

    let first = project_sysinfo_extended(&device_id, &profile).unwrap();
    let second = project_sysinfo_extended(&device_id, &profile).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.content_hash().as_str(),
        blake3::hash(first.bytes()).to_hex().as_str()
    );
    assert_eq!(first.content_hash().as_str(), LATE_2009_PROJECTION_HASH);
    assert!(first
        .content_hash()
        .as_str()
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));

    let root = plist::from_bytes::<Value>(first.bytes()).unwrap();
    let dictionary = root.as_dictionary().unwrap();
    assert_eq!(
        dictionary
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "AlbumArt",
            "ChapterImageSpecs",
            "DBVersion",
            "FamilyID",
            "FireWireGUID",
            "ImageSpecifications",
            "SQLiteDB",
            "SupportsSparseArtwork",
        ])
    );
    assert_eq!(string(dictionary, "FireWireGUID"), "000A27002138B0A8");
    assert_eq!(integer(dictionary, "FamilyID"), 11);
    assert_eq!(integer(dictionary, "DBVersion"), 3);
    assert_eq!(boolean(dictionary, "SupportsSparseArtwork"), true);
    assert_eq!(boolean(dictionary, "SQLiteDB"), false);

    assert_formats(dictionary, "AlbumArt", &profile.album_art);
    assert_formats(
        dictionary,
        "ImageSpecifications",
        &profile.image_specifications,
    );
    assert_formats(
        dictionary,
        "ChapterImageSpecs",
        &profile.chapter_image_specs,
    );

    let xml = std::str::from_utf8(first.bytes()).unwrap();
    for prohibited in [
        "RentalClockBias",
        "rbsync",
        "SerialNumber",
        "ModelNumStr",
        "BuildID",
        "VisibleBuildID",
        "ProductType",
        "SKU",
        "Colour",
        "Color",
        "Capacity",
        "BatteryPollInterval",
        "VolumeFormat",
        "ConnectedBus",
        "HostID",
        "Classick",
    ] {
        assert!(
            !xml.contains(&format!("<key>{prohibited}</key>")),
            "projected prohibited key {prohibited}"
        );
    }
}

#[test]
fn classifies_existing_bytes_only_from_exact_owned_hash_authority() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let profile = CapabilityProfile::from_json(CAPABILITY_FIXTURE).unwrap();
    let expected = project_sysinfo_extended(&device_id, &profile).unwrap();
    let legacy_donor = include_bytes!("../data/sysinfo-extended/classic-late2009.plist");

    assert_eq!(
        decide_sysinfo_extended(None, &expected, None),
        SysInfoExtendedDecision::EligibleToGenerate
    );
    assert_eq!(
        decide_sysinfo_extended(None, &expected, Some(expected.content_hash())),
        SysInfoExtendedDecision::EligibleToGenerate
    );
    assert_eq!(
        decide_sysinfo_extended(Some(b"not a plist"), &expected, None),
        SysInfoExtendedDecision::PreserveForeign {
            existing_bytes: b"not a plist"
        }
    );
    assert_eq!(
        decide_sysinfo_extended(Some(legacy_donor), &expected, None),
        SysInfoExtendedDecision::PreserveForeign {
            existing_bytes: legacy_donor
        }
    );
    assert_eq!(
        decide_sysinfo_extended(Some(expected.bytes()), &expected, None),
        SysInfoExtendedDecision::PreserveForeign {
            existing_bytes: expected.bytes()
        }
    );
    assert_eq!(
        decide_sysinfo_extended(
            Some(expected.bytes()),
            &expected,
            Some(expected.content_hash())
        ),
        SysInfoExtendedDecision::ExistingOwnedValid
    );

    let conflicting = b"previous Classick projection";
    let conflicting_hash = content_hash(conflicting);
    assert_eq!(
        decide_sysinfo_extended(Some(conflicting), &expected, Some(&conflicting_hash)),
        SysInfoExtendedDecision::OwnedConflict {
            existing_bytes: conflicting
        }
    );
    assert_eq!(
        decide_sysinfo_extended(Some(conflicting), &expected, Some(expected.content_hash())),
        SysInfoExtendedDecision::OwnershipMismatch {
            existing_bytes: conflicting
        }
    );
}

fn assert_formats(root: &Dictionary, key: &str, expected: &[ImageFormat]) {
    let actual = root.get(key).and_then(Value::as_array).unwrap();
    assert_eq!(actual.len(), expected.len(), "{key} format count");

    for (actual, expected) in actual.iter().zip(expected) {
        let actual = actual.as_dictionary().unwrap();
        let mut expected_keys = BTreeSet::from([
            "AlignRowBytes",
            "AssociatedFormat",
            "ColorAdjustment",
            "Crop",
            "FormatId",
            "GammaAdjustment",
            "Interlaced",
            "PixelFormat",
            "RenderHeight",
            "RenderWidth",
        ]);
        for (name, present) in [
            ("DisplayWidth", expected.display_width.is_some()),
            ("Rotation", expected.rotation.is_some()),
            ("BackColor", expected.back_color.is_some()),
            ("ExcludedFormats", expected.excluded_formats.is_some()),
        ] {
            if present {
                expected_keys.insert(name);
            }
        }
        assert_eq!(
            actual.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected_keys,
            "format {} keys",
            expected.format_id
        );
        assert_eq!(integer(actual, "FormatId"), i64::from(expected.format_id));
        assert_eq!(
            integer(actual, "RenderWidth"),
            i64::from(expected.render_width)
        );
        assert_eq!(
            integer(actual, "RenderHeight"),
            i64::from(expected.render_height)
        );
        assert_optional_integer(
            actual,
            "DisplayWidth",
            expected.display_width.map(i64::from),
        );
        assert_eq!(string(actual, "PixelFormat"), expected.pixel_format);
        assert_eq!(boolean(actual, "Interlaced"), expected.interlaced);
        assert_eq!(boolean(actual, "Crop"), expected.crop);
        assert_eq!(boolean(actual, "AlignRowBytes"), expected.align_row_bytes);
        assert_optional_integer(actual, "Rotation", expected.rotation.map(i64::from));
        assert_eq!(
            actual.get("BackColor").and_then(Value::as_string),
            expected.back_color.as_deref()
        );
        assert_eq!(
            integer(actual, "ColorAdjustment"),
            i64::from(expected.color_adjustment)
        );
        assert_eq!(
            actual
                .get("GammaAdjustment")
                .and_then(Value::as_real)
                .unwrap(),
            expected.gamma_adjustment
        );
        assert_eq!(
            integer(actual, "AssociatedFormat"),
            i64::from(expected.associated_format)
        );
        assert_optional_integer(actual, "ExcludedFormats", expected.excluded_formats);
    }
}

fn assert_optional_integer(dictionary: &Dictionary, key: &str, expected: Option<i64>) {
    assert_eq!(
        dictionary.get(key).and_then(Value::as_signed_integer),
        expected,
        "{key}"
    );
}

fn string<'a>(dictionary: &'a Dictionary, key: &str) -> &'a str {
    dictionary.get(key).and_then(Value::as_string).unwrap()
}

fn integer(dictionary: &Dictionary, key: &str) -> i64 {
    dictionary
        .get(key)
        .and_then(Value::as_signed_integer)
        .unwrap()
}

fn boolean(dictionary: &Dictionary, key: &str) -> bool {
    dictionary.get(key).and_then(Value::as_boolean).unwrap()
}

fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash::parse(blake3::hash(bytes).to_hex().as_str()).unwrap()
}
