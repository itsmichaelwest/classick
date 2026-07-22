use classick::device::{DeviceId, DeviceReadiness};
use classick::ipod::{
    assess_sysinfo_for_artwork, project_sysinfo_extended, resolve_validated_capability_profile,
    CapabilityProfileId, OwnedSysInfoAuthority, SysInfoArtworkAdmission, SysInfoArtworkBlockReason,
};
use classick::portable::profile::{ContentHash, PortableProfile};
use plist::Value;

fn expected() -> (DeviceId, classick::ipod::SysInfoExtendedProjection) {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let profile_id = CapabilityProfileId::parse("classic-late-2009-v1").unwrap();
    let validated = resolve_validated_capability_profile(&profile_id)
        .unwrap()
        .unwrap();
    let projection = project_sysinfo_extended(&device_id, &validated).unwrap();
    (device_id, projection)
}

#[test]
fn absent_file_requires_bound_coordinated_generation() {
    let (device_id, expected) = expected();

    let SysInfoArtworkAdmission::GenerateInTransaction { projection } =
        assess(&device_id, None, &expected, None)
    else {
        panic!("ready device with absent file should plan generation");
    };
    assert!(std::ptr::eq(projection, &expected));
}

#[test]
fn exact_owned_projection_is_bound_and_immediately_usable() {
    let (device_id, expected) = expected();
    let authority = owned_authority(&device_id, expected.content_hash(), "classic-late-2009-v1");

    let SysInfoArtworkAdmission::UseOwnedProjection { projection } = assess(
        &device_id,
        Some(expected.bytes()),
        &expected,
        Some(&authority),
    ) else {
        panic!("exact owned projection should be usable");
    };
    assert!(std::ptr::eq(projection, &expected));
}

#[test]
fn complete_foreign_file_carries_verified_album_art_without_claiming_bytes() {
    let (device_id, expected) = expected();

    let SysInfoArtworkAdmission::UseForeign {
        existing_bytes,
        album_art,
    } = assess(&device_id, Some(expected.bytes()), &expected, None)
    else {
        panic!("complete foreign album-art capability should be usable");
    };
    assert_eq!(existing_bytes, expected.bytes());
    assert!(!album_art.is_empty());
}

#[test]
fn malformed_or_incomplete_foreign_file_blocks_only_artwork() {
    let (device_id, expected) = expected();
    let malformed = b"not a plist";
    assert_eq!(
        assess(&device_id, Some(malformed), &expected, None),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(malformed),
            reason: SysInfoArtworkBlockReason::ForeignMalformed,
        }
    );

    let incomplete = rewrite(expected.bytes(), |root| {
        root.remove("AlbumArt");
    });
    assert_eq!(
        assess(&device_id, Some(&incomplete), &expected, None),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(&incomplete),
            reason: SysInfoArtworkBlockReason::ForeignAlbumArtIncomplete,
        }
    );
}

#[test]
fn foreign_identity_and_artwork_scalars_block_but_unrelated_collections_do_not() {
    let (device_id, expected) = expected();
    for identity in [
        rewrite(expected.bytes(), |root| {
            root.remove("FireWireGUID");
        }),
        rewrite(expected.bytes(), |root| {
            root.insert("FireWireGUID".into(), Value::String("invalid".into()));
        }),
        rewrite(expected.bytes(), |root| {
            root.insert(
                "FireWireGUID".into(),
                Value::String("000A27002138B0A9".into()),
            );
        }),
    ] {
        assert_eq!(
            assess(&device_id, Some(&identity), &expected, None),
            SysInfoArtworkAdmission::Blocked {
                existing_bytes: Some(&identity),
                reason: SysInfoArtworkBlockReason::ForeignIdentityInvalid,
            }
        );
    }

    let invalid_sparse = rewrite(expected.bytes(), |root| {
        root.insert(
            "SupportsSparseArtwork".into(),
            Value::String("false".into()),
        );
    });
    assert_eq!(
        assess(&device_id, Some(&invalid_sparse), &expected, None),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(&invalid_sparse),
            reason: SysInfoArtworkBlockReason::ForeignArtworkFactsInvalid,
        }
    );

    let unrelated = rewrite(expected.bytes(), |root| {
        root.remove("ImageSpecifications");
        root.remove("ChapterImageSpecs");
    });
    assert!(matches!(
        assess(&device_id, Some(&unrelated), &expected, None),
        SysInfoArtworkAdmission::UseForeign { .. }
    ));
}

#[test]
fn conflicting_owned_authority_never_falls_back_to_foreign_use() {
    let (device_id, expected) = expected();
    let conflict = b"previous Classick projection";
    let actual_hash = content_hash(conflict);
    let old_authority = owned_authority(&device_id, &actual_hash, "classic-late-2009-v1");
    let expected_authority =
        owned_authority(&device_id, expected.content_hash(), "classic-late-2009-v1");

    assert_eq!(
        assess(&device_id, Some(conflict), &expected, Some(&old_authority),),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(conflict),
            reason: SysInfoArtworkBlockReason::OwnedProjectionNeedsReplacement,
        }
    );
    assert_eq!(
        assess(
            &device_id,
            Some(conflict),
            &expected,
            Some(&expected_authority),
        ),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(conflict),
            reason: SysInfoArtworkBlockReason::OwnedHashMismatch,
        }
    );
}

#[test]
fn readiness_device_and_capability_authority_are_all_bound() {
    let (device_id, expected) = expected();
    assert_eq!(
        assess_sysinfo_for_artwork(
            &device_id,
            DeviceReadiness::NeedsAppleInitialization,
            None,
            &expected,
            None,
        ),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: None,
            reason: SysInfoArtworkBlockReason::DeviceNotReady,
        }
    );

    let other_id = DeviceId::parse("000A27002138B0A9").unwrap();
    assert_eq!(
        assess(&other_id, None, &expected, None),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: None,
            reason: SysInfoArtworkBlockReason::ProjectionDeviceMismatch,
        }
    );

    let wrong_capability =
        owned_authority(&device_id, expected.content_hash(), "classic-late-2009-v2");
    assert_eq!(
        assess(
            &device_id,
            Some(expected.bytes()),
            &expected,
            Some(&wrong_capability),
        ),
        SysInfoArtworkAdmission::Blocked {
            existing_bytes: Some(expected.bytes()),
            reason: SysInfoArtworkBlockReason::OwnershipAuthorityMismatch,
        }
    );
}

fn assess<'a>(
    device_id: &DeviceId,
    existing: Option<&'a [u8]>,
    expected: &'a classick::ipod::SysInfoExtendedProjection,
    authority: Option<&OwnedSysInfoAuthority>,
) -> SysInfoArtworkAdmission<'a> {
    assess_sysinfo_for_artwork(
        device_id,
        DeviceReadiness::Ready,
        existing,
        expected,
        authority,
    )
}

fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash::parse(blake3::hash(bytes).to_hex().as_str()).unwrap()
}

fn owned_authority(
    device_id: &DeviceId,
    hash: &ContentHash,
    capability_profile_id: &str,
) -> OwnedSysInfoAuthority {
    let json = format!(
        r#"{{
          "schema_version":1,
          "device_id":"{}",
          "capability_profile_id":"{}",
          "selection":{{"revision":1,"mutation_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8760","value":{{"schema_version":1,"mode":"all","rules":[]}}}},
          "settings":{{"revision":1,"mutation_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8761","value":{{"schema_version":1,"auto_sync":false,"rockbox_compat":false}}}},
          "subscriptions":{{"revision":1,"mutation_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8762","value":{{"schema_version":1,"playlists":[]}}}},
          "owned_playlists":[],
          "companion_authorities":[],
          "generated_sysinfo_extended_hash":"{}"
        }}"#,
        device_id.as_str(),
        capability_profile_id,
        hash.as_str(),
    );
    let profile = PortableProfile::from_json(&json).unwrap();
    OwnedSysInfoAuthority::from_portable_profile(&profile)
        .unwrap()
        .unwrap()
}

fn rewrite(bytes: &[u8], mutate: impl FnOnce(&mut plist::Dictionary)) -> Vec<u8> {
    let mut root = plist::from_bytes::<Value>(bytes).unwrap();
    mutate(root.as_dictionary_mut().unwrap());
    let mut rewritten = Vec::new();
    root.to_writer_xml(&mut rewritten).unwrap();
    rewritten
}
