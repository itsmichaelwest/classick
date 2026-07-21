use classick::ipod::{CapabilityProfile, CapabilityProfileId};
use serde::Deserialize;
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("fixtures/device-capabilities/classic-late-2009-v1.json");
const MANIFEST: &str =
    include_str!("fixtures/device-capabilities/classic-late-2009-v1.manifest.json");

#[test]
fn late_2009_fixture_is_a_complete_validated_stable_profile() {
    let profile = CapabilityProfile::from_json(FIXTURE).expect("validated capability fixture");

    assert_eq!(
        profile.profile_id,
        CapabilityProfileId::parse("classic-late-2009-v1").unwrap()
    );
    assert_eq!(profile.schema_version, 1);
    assert_eq!(profile.family_id, 11);
    assert_eq!(profile.db_version, 3);
    assert!(profile.supports_sparse_artwork);
    assert!(!profile.sqlite_db);
    assert_eq!(
        format_ids(&profile.album_art),
        [1069, 1055, 1068, 1060, 1061]
    );
    assert_eq!(
        format_ids(&profile.image_specifications),
        [1067, 1024, 1066]
    );
    assert_eq!(format_ids(&profile.chapter_image_specs), [1055, 1029]);
}

#[test]
fn validated_fixture_has_a_deterministic_canonical_encoding() {
    let profile = CapabilityProfile::from_json(FIXTURE).expect("validated capability fixture");

    assert_eq!(profile.to_json_pretty().unwrap(), FIXTURE);
}

#[test]
fn profile_and_formats_reject_keys_outside_the_capability_allowlist() {
    for prohibited_key in ["RentalClockBias", "rbsync", "colour", "model_code"] {
        let mut value: Value = serde_json::from_str(FIXTURE).unwrap();
        value.as_object_mut().unwrap().insert(
            prohibited_key.to_owned(),
            json!("private-or-appearance-data"),
        );

        assert!(
            CapabilityProfile::from_json(&serde_json::to_string(&value).unwrap()).is_err(),
            "accepted prohibited profile key {prohibited_key}"
        );
    }

    let mut value: Value = serde_json::from_str(FIXTURE).unwrap();
    value["album_art"][0]["owner_name"] = json!("private-data");
    assert!(CapabilityProfile::from_json(&serde_json::to_string(&value).unwrap()).is_err());
}

#[test]
fn profile_validation_rejects_incomplete_or_ambiguous_format_data() {
    let mut duplicate: Value = serde_json::from_str(FIXTURE).unwrap();
    duplicate["album_art"][1]["format_id"] = duplicate["album_art"][0]["format_id"].clone();
    assert!(CapabilityProfile::from_json(&serde_json::to_string(&duplicate).unwrap()).is_err());

    let mut empty: Value = serde_json::from_str(FIXTURE).unwrap();
    empty["chapter_image_specs"] = json!([]);
    assert!(CapabilityProfile::from_json(&serde_json::to_string(&empty).unwrap()).is_err());

    let mut malformed_pixel_format: Value = serde_json::from_str(FIXTURE).unwrap();
    malformed_pixel_format["image_specifications"][0]["pixel_format"] = json!("y420");
    assert!(
        CapabilityProfile::from_json(&serde_json::to_string(&malformed_pixel_format).unwrap())
            .is_err()
    );
}

#[test]
fn capability_profile_id_is_a_canonical_non_appearance_identifier() {
    let id = CapabilityProfileId::parse("classic-late-2009-v1").unwrap();
    assert_eq!(id.as_str(), "classic-late-2009-v1");
    assert_eq!(
        serde_json::to_string(&id).unwrap(),
        "\"classic-late-2009-v1\""
    );

    for invalid in [
        "",
        "Classic-late-2009-v1",
        "classic_late_2009_v1",
        "-classic-late-2009-v1",
        "classic-late-2009-v1-",
        "classic--late-2009-v1",
        "classic/late-2009/v1",
    ] {
        assert!(
            CapabilityProfileId::parse(invalid).is_err(),
            "accepted capability profile ID {invalid:?}"
        );
    }
}

#[test]
fn privacy_manifest_accounts_for_every_source_key_and_dynamic_exclusions() {
    let manifest: FixtureManifest = serde_json::from_str(MANIFEST).expect("strict manifest");
    let mut accounted_for = manifest.retained_keys.clone();
    accounted_for.extend(manifest.removed_keys.iter().cloned());
    accounted_for.sort();

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.fixture, "classic-late-2009-v1.json");
    assert!(manifest.source.contains("privacy-redacted physical"));
    assert_eq!(
        manifest
            .provenance
            .iter()
            .map(|entry| (
                entry.capture_state.as_str(),
                entry.observation.as_str(),
                entry.observed_on.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("factory_restored", "live_extended_inquiry", "2026-07-19"),
            ("finder_initialized", "live_extended_inquiry", "2026-07-19"),
            (
                "finder_initialized",
                "immediate_repeat_extended_inquiry",
                "2026-07-19"
            ),
        ]
    );
    assert_eq!(accounted_for, SOURCE_KEYS);
    assert!(manifest
        .removed_keys
        .windows(2)
        .all(|pair| pair[0] < pair[1]));
    assert!(manifest
        .removed_keys
        .contains(&"RentalClockBias".to_owned()));
    assert!(manifest.removed_keys.contains(&"rbsync".to_owned()));
    assert!(manifest.removed_keys.contains(&"FireWireGUID".to_owned()));
    assert!(manifest.removed_keys.contains(&"SerialNumber".to_owned()));
    assert_eq!(manifest.derived_fields.len(), 1);
    assert_eq!(manifest.derived_fields[0].fixture_field, "sqlite_db");
    assert_eq!(manifest.derived_fields[0].source_key, "SQLiteDB");
    assert!(!manifest.derived_fields[0].value);
    assert_eq!(
        manifest.derived_fields[0].basis,
        "absent_in_all_source_observations"
    );
    assert_eq!(
        manifest
            .validation_authorities
            .iter()
            .map(|authority| (
                authority.kind.as_str(),
                authority.revision.as_str(),
                authority.location.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "pinned_libgpod_parser",
                "4a8a33ef4bc58eee1baca6793618365f75a5c3fa",
                "src/itdb_sysinfo_extended_parser.c"
            ),
            (
                "approved_classick_design",
                "2026-07-19",
                "docs/design/2026-07-19-native-device-protocol.md#8-sysinfoextended-capability-projection"
            ),
        ]
    );
    assert!(!FIXTURE.contains("RentalClockBias"));
    assert!(!FIXTURE.contains("rbsync"));
}

#[test]
fn fixture_manifest_rejects_unaccounted_metadata() {
    let mut value: Value = serde_json::from_str(MANIFEST).unwrap();
    value["device_name"] = json!("private-data");

    assert!(serde_json::from_value::<FixtureManifest>(value).is_err());
}

fn format_ids(formats: &[classick::ipod::ImageFormat]) -> Vec<u32> {
    formats.iter().map(|format| format.format_id).collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureManifest {
    schema_version: u32,
    fixture: String,
    source: String,
    provenance: Vec<Provenance>,
    retained_keys: Vec<String>,
    derived_fields: Vec<DerivedField>,
    removed_keys: Vec<String>,
    validation_authorities: Vec<ValidationAuthority>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    capture_state: String,
    observation: String,
    observed_on: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedField {
    fixture_field: String,
    source_key: String,
    value: bool,
    basis: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationAuthority {
    kind: String,
    revision: String,
    location: String,
}

const SOURCE_KEYS: [&str; 66] = [
    "64Bit",
    "AlbumArt",
    "AppleDRMVersion",
    "AudioCodecs",
    "AutoRebootAfterFirmwareUpdate",
    "BangFolder",
    "BatteryPollInterval",
    "BuildID",
    "BuiltInGames",
    "CameWithCD",
    "CanFlashBacklight",
    "CanHibernate",
    "ChapterImageSpecs",
    "ConnectedBus",
    "CorruptDataPartition",
    "DBVersion",
    "FWPartSize",
    "FamilyID",
    "FireWireGUID",
    "FireWireVersion",
    "ForcedDiskMode",
    "GamesPlatformID",
    "GamesPlatformVersion",
    "GeniusConfigMaxVersion",
    "GeniusConfigMinVersion",
    "GeniusMetadataMaxVersion",
    "GeniusMetadataMinVersion",
    "GeniusSimilaritiesMaxVersion",
    "GeniusSimilaritiesMinVersion",
    "HotPlugState",
    "ImageSpecifications",
    "KeyTypeSupportVersion",
    "Language",
    "MaxFWBlocks",
    "MaxFileSizeInGB",
    "MaxThumbFileSize",
    "MaxTracks",
    "MaxTransferSpeed",
    "MinBuildID",
    "MinITunesVersion",
    "OEMA",
    "OEMID",
    "OEMU",
    "PlaylistFoldersSupported",
    "PodcastsSupported",
    "PowerInformation",
    "RAM",
    "RBRequestVersion",
    "RentalClockBias",
    "ReservedMB",
    "SerialNumber",
    "SortFieldsSupported",
    "SupportsGenius",
    "SupportsGeniusMixes",
    "SupportsSparseArtwork",
    "UpdateMethod",
    "UpdaterFamilyID",
    "VideoCodecs",
    "VisibleBuildID",
    "VoiceMemoFormats",
    "VoiceMemosSupported",
    "VolumeFormat",
    "VolumeInformation",
    "iTunesUSupported",
    "rbsync",
    "vCardWithJPEGSupported",
];
