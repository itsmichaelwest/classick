use classick::device::DeviceId;
use classick::ipod::{
    inspect_foreign_sysinfo_extended, project_sysinfo_extended,
    resolve_validated_capability_profile, CapabilityProfile, ForeignPixelFormat,
    ForeignSysInfoCollection, ForeignSysInfoFormatField, ForeignSysInfoInspection,
    ForeignSysInfoIssue, ForeignSysInfoStableFacts, ForeignSysInfoStableField,
};
use plist::Value;

const CAPABILITY_FIXTURE: &str =
    include_str!("../data/device-capabilities/classic-late-2009-v1.json");

#[test]
fn malformed_foreign_sysinfo_is_distinct_from_incomplete_content() {
    let bytes = b"not a plist";
    let device_id = DeviceId::parse("000A27002150925D").unwrap();

    let inspection = inspect_foreign_sysinfo_extended(bytes, &device_id);

    assert!(matches!(
        inspection,
        ForeignSysInfoInspection::Malformed { .. }
    ));
    assert_eq!(bytes, b"not a plist");
}

#[test]
fn complete_foreign_capabilities_are_typed_without_granting_ownership() {
    let (device_id, profile, bytes) = complete_foreign_plist();

    let ForeignSysInfoInspection::Parsed {
        stable_facts,
        capability,
        issues,
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("projected plist should parse");
    };

    assert_eq!(
        stable_facts,
        ForeignSysInfoStableFacts {
            family_id: Some(11),
            db_version: Some(3),
            supports_sparse_artwork: Some(true),
            sqlite_db: Some(false),
        }
    );
    assert!(issues.is_empty());
    let album_art = capability.album_art.unwrap();
    assert_eq!(album_art.len(), profile.album_art.len());
    assert_eq!(album_art[0].format_id, profile.album_art[0].format_id);
}

#[test]
fn unrelated_foreign_metadata_is_ignored() {
    let (device_id, _, bytes) = complete_foreign_plist();
    let bytes = rewrite(&bytes, |root| {
        root.insert("SerialNumber".into(), Value::String("PRIVATE".into()));
        root.insert("ModelNumStr".into(), Value::String("MC293".into()));
        root.insert("Color".into(), Value::String("silver".into()));
        root.insert("FutureAppleKey".into(), Value::Boolean(true));
    });

    let ForeignSysInfoInspection::Parsed {
        stable_facts,
        capability,
        issues,
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("foreign plist should parse");
    };

    assert!(issues.is_empty());
    assert_eq!(stable_facts.family_id, Some(11));
    assert!(capability.album_art.is_some());
}

#[test]
fn checked_in_foreign_capture_matches_pinned_libgpod_semantics() {
    let bytes = include_bytes!("../data/sysinfo-extended/classic-late2009.plist");
    let device_id = DeviceId::parse("000A27002150925D").unwrap();

    let ForeignSysInfoInspection::Parsed {
        stable_facts,
        capability,
        issues,
    } = inspect_foreign_sysinfo_extended(bytes, &device_id)
    else {
        panic!("checked-in foreign plist should parse");
    };

    assert_eq!(stable_facts.sqlite_db, Some(false));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(capability.album_art.unwrap().len(), 5);
    assert_eq!(capability.image_specifications.unwrap().len(), 3);
    assert_eq!(capability.chapter_image_specs.unwrap().len(), 2);
}

#[test]
fn identity_mismatch_and_incomplete_scalars_are_explicit() {
    let (device_id, _, bytes) = complete_foreign_plist();
    let bytes = rewrite(&bytes, |root| {
        root.insert(
            "FireWireGUID".into(),
            Value::String("000A270000000001".into()),
        );
        root.remove("DBVersion");
        root.insert("SQLiteDB".into(), Value::String("false".into()));
    });

    let ForeignSysInfoInspection::Parsed {
        stable_facts,
        capability,
        issues,
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("foreign plist should parse");
    };

    assert_eq!(stable_facts.db_version, None);
    assert_eq!(stable_facts.sqlite_db, None);
    assert!(capability.album_art.is_some());
    assert!(capability.image_specifications.is_some());
    assert!(capability.chapter_image_specs.is_some());
    assert!(issues.contains(&ForeignSysInfoIssue::IdentityMismatch {
        actual: DeviceId::parse("000A270000000001").unwrap(),
    }));
    assert!(issues.contains(&ForeignSysInfoIssue::MissingStableField(
        ForeignSysInfoStableField::DbVersion,
    )));
    assert!(issues.contains(&ForeignSysInfoIssue::InvalidStableField(
        ForeignSysInfoStableField::SqliteDb,
    )));
}

#[test]
fn incomplete_and_duplicate_image_formats_are_explicit() {
    let (device_id, _, bytes) = complete_foreign_plist();
    let bytes = rewrite(&bytes, |root| {
        root.insert("ChapterImageSpecs".into(), Value::Array(Vec::new()));

        let album_art = root.get_mut("AlbumArt").unwrap().as_array_mut().unwrap();
        let duplicate = album_art[0].clone();
        album_art.push(duplicate);

        root.get_mut("ImageSpecifications")
            .unwrap()
            .as_array_mut()
            .unwrap()[0]
            .as_dictionary_mut()
            .unwrap()
            .remove("PixelFormat");
    });

    let ForeignSysInfoInspection::Parsed {
        capability, issues, ..
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("foreign plist should parse");
    };

    assert!(capability.album_art.is_none());
    assert!(capability.image_specifications.is_none());
    assert!(capability.chapter_image_specs.is_none());
    assert!(issues.contains(&ForeignSysInfoIssue::EmptyCollection(
        ForeignSysInfoCollection::ChapterImageSpecs,
    )));
    assert!(issues.contains(&ForeignSysInfoIssue::DuplicateFormatId {
        collection: ForeignSysInfoCollection::AlbumArt,
        format_id: 1069,
    }));
    assert!(issues.contains(&ForeignSysInfoIssue::InvalidFormat {
        collection: ForeignSysInfoCollection::ImageSpecifications,
        index: 0,
        field: Some(ForeignSysInfoFormatField::PixelFormat),
    }));
}

#[test]
fn pixel_order_and_row_alignment_follow_pinned_libgpod() {
    let (device_id, _, bytes) = complete_foreign_plist();
    let bytes = rewrite(&bytes, |root| {
        let format = root.get_mut("AlbumArt").unwrap().as_array_mut().unwrap()[0]
            .as_dictionary_mut()
            .unwrap();
        format.insert("PixelFormat".into(), Value::String("4C353535".into()));
        format.insert("PixelOrder".into(), Value::String("recombined".into()));
        format.remove("AlignRowBytes");
        format.insert("RowBytesAlignment".into(), Value::Integer(8.into()));
    });

    let ForeignSysInfoInspection::Parsed {
        capability, issues, ..
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("foreign plist should parse");
    };

    assert!(issues.is_empty(), "{issues:?}");
    let format = &capability.album_art.unwrap()[0];
    assert_eq!(
        format.pixel_format,
        ForeignPixelFormat::RecombinedRgb555LittleEndian
    );
    assert_eq!(format.row_bytes_alignment, 8);
}

#[test]
fn unsupported_pixel_format_invalidates_only_its_collection() {
    let (device_id, _, bytes) = complete_foreign_plist();
    let bytes = rewrite(&bytes, |root| {
        root.get_mut("AlbumArt").unwrap().as_array_mut().unwrap()[0]
            .as_dictionary_mut()
            .unwrap()
            .insert("PixelFormat".into(), Value::String("DEADBEEF".into()));
    });

    let ForeignSysInfoInspection::Parsed {
        capability, issues, ..
    } = inspect_foreign_sysinfo_extended(&bytes, &device_id)
    else {
        panic!("foreign plist should parse");
    };

    assert!(capability.album_art.is_none());
    assert!(capability.image_specifications.is_some());
    assert!(capability.chapter_image_specs.is_some());
    assert!(issues.contains(&ForeignSysInfoIssue::InvalidFormat {
        collection: ForeignSysInfoCollection::AlbumArt,
        index: 0,
        field: Some(ForeignSysInfoFormatField::PixelFormat),
    }));
}

fn complete_foreign_plist() -> (DeviceId, CapabilityProfile, Vec<u8>) {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let profile = CapabilityProfile::from_json(CAPABILITY_FIXTURE).unwrap();
    let validated = resolve_validated_capability_profile(&profile.profile_id)
        .unwrap()
        .unwrap();
    let projection = project_sysinfo_extended(&device_id, &validated).unwrap();
    (device_id, profile, projection.bytes().to_vec())
}

fn rewrite(bytes: &[u8], mutate: impl FnOnce(&mut plist::Dictionary)) -> Vec<u8> {
    let mut root = plist::from_bytes::<Value>(bytes).unwrap();
    mutate(root.as_dictionary_mut().unwrap());
    let mut rewritten = Vec::new();
    root.to_writer_xml(&mut rewritten).unwrap();
    rewritten
}
