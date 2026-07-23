use classick::device::DeviceId;
use classick::portable::outbox::{PendingDeviceOutbox, PendingMutation};
use classick::portable::profile::{
    CompanionAuthority, ContentHash, MutationId, PlaylistSlug, PortableProfile, ProfilePath,
    SettingsValue, SubscriptionsValue,
};
use classick::portable::reconcile::{
    confirm_reconciled_profile, plan_portable_reconciliation, CommitConfirmation,
    CompanionFileReadback, DeviceProfileObservation, PortableReconciliationPlan,
    ProfilePublicationContext,
};

#[test]
fn exact_readback_issues_only_a_compare_and_swap_outbox_clear() {
    let device_id = device_id();
    let expected = profile(&device_id, false);
    let outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            expected.settings.mutation_id.clone(),
            device_id.clone(),
            expected.settings.value.clone(),
            6,
        )
        .unwrap()],
    };
    let PortableReconciliationPlan::PublishPending { publication } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Valid(&expected),
        &outbox,
        &ProfilePublicationContext::default(),
    ) else {
        panic!("replay should produce a confirmable publication");
    };
    let CommitConfirmation::Confirmed {
        cache,
        outbox_clear,
    } = confirm_reconciled_profile(
        &device_id,
        &publication,
        DeviceProfileObservation::Valid(&expected),
        &[],
    )
    else {
        panic!("exact readback should confirm");
    };
    assert_eq!(cache.last_imported_profile, Some(expected.clone()));
    assert!(outbox_clear.apply_to(&outbox).unwrap().mutations.is_empty());

    let changed = profile(&device_id, true);
    assert!(matches!(
        confirm_reconciled_profile(
            &device_id,
            &publication,
            DeviceProfileObservation::Valid(&changed),
            &[],
        ),
        CommitConfirmation::Pending { .. }
    ));
    let newer_outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            mutation_id(11),
            device_id,
            settings(true, false),
            expected.settings.revision,
        )
        .unwrap()],
    };
    assert!(outbox_clear.apply_to(&newer_outbox).is_err());
}

#[test]
fn subscription_confirmation_verifies_definition_and_preserved_manifest_bytes() {
    let device_id = device_id();
    let mut profile = profile(&device_id, false);
    let old_slug = PlaylistSlug::parse("old-mix").unwrap();
    let manifest_bytes = b"{\"version\":1}\n";
    let manifest_path = ProfilePath::parse("manifest.json").unwrap();
    profile.subscriptions.value.playlists = vec![old_slug.clone()];
    profile.companion_authorities = vec![
        CompanionAuthority::Manifest {
            schema_version: 1,
            relative_path: manifest_path.clone(),
            content_hash: content_hash(manifest_bytes),
        },
        CompanionAuthority::PlaylistDefinition {
            slug: old_slug,
            schema_version: 1,
            relative_path: ProfilePath::parse("playlists/old-mix.m3u8").unwrap(),
            content_hash: content_hash(b"old definition"),
        },
    ];
    profile.validate().unwrap();

    let slug = PlaylistSlug::parse("favourites").unwrap();
    let outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::subscriptions(
            mutation_id(12),
            device_id.clone(),
            SubscriptionsValue {
                schema_version: 1,
                playlists: vec![slug.clone()],
            },
            profile.subscriptions.revision,
        )
        .unwrap()],
    };
    let definition_bytes = b"#EXTM3U\n/Music/song.m4a\n";
    let definition_path = ProfilePath::parse("playlists/favourites.m3u8").unwrap();
    let definition = CompanionAuthority::PlaylistDefinition {
        slug,
        schema_version: 1,
        relative_path: definition_path.clone(),
        content_hash: content_hash(definition_bytes),
    };
    let context = ProfilePublicationContext {
        capability_profile_id: None,
        generated_sysinfo_extended_hash: None,
        companion_authorities: vec![definition.clone()],
    };
    let PortableReconciliationPlan::PublishPending { publication } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Valid(&profile),
        &outbox,
        &context,
    ) else {
        panic!("subscription plus exact definition authority should publish");
    };
    assert!(matches!(
        publication.candidate_profile().companion_authorities[0],
        CompanionAuthority::Manifest { .. }
    ));
    assert_eq!(
        publication.candidate_profile().companion_authorities[1],
        definition
    );

    assert!(matches!(
        confirm_reconciled_profile(
            &device_id,
            &publication,
            DeviceProfileObservation::Valid(publication.candidate_profile()),
            &[],
        ),
        CommitConfirmation::Pending { .. }
    ));
    let manifest = CompanionFileReadback {
        relative_path: &manifest_path,
        bytes: Some(manifest_bytes),
    };
    let stale = CompanionFileReadback {
        relative_path: &definition_path,
        bytes: Some(b"stale"),
    };
    assert!(matches!(
        confirm_reconciled_profile(
            &device_id,
            &publication,
            DeviceProfileObservation::Valid(publication.candidate_profile()),
            &[manifest, stale],
        ),
        CommitConfirmation::Pending { .. }
    ));
    let verified = CompanionFileReadback {
        relative_path: &definition_path,
        bytes: Some(definition_bytes),
    };
    assert!(matches!(
        confirm_reconciled_profile(
            &device_id,
            &publication,
            DeviceProfileObservation::Valid(publication.candidate_profile()),
            &[manifest, verified],
        ),
        CommitConfirmation::Confirmed { .. }
    ));
}

fn profile(device_id: &DeviceId, auto_sync: bool) -> PortableProfile {
    PortableProfile::from_json(&format!(
        r#"{{
          "schema_version":1,
          "device_id":"{}",
          "selection":{{"revision":3,"mutation_id":"{}","value":{{"schema_version":1,"mode":"all","rules":[]}}}},
          "settings":{{"revision":7,"mutation_id":"{}","value":{{"schema_version":1,"auto_sync":{},"rockbox_compat":false,"transcode_profile":"alac"}}}},
          "subscriptions":{{"revision":4,"mutation_id":"{}","value":{{"schema_version":1,"playlists":[]}}}},
          "owned_playlists":[],
          "companion_authorities":[]
        }}"#,
        device_id.as_str(), mutation_id(1), mutation_id(2), auto_sync, mutation_id(3)
    )).unwrap()
}

fn settings(auto_sync: bool, rockbox_compat: bool) -> SettingsValue {
    SettingsValue {
        schema_version: 1,
        auto_sync,
        rockbox_compat,
        transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
    }
}

fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash::parse(blake3::hash(bytes).to_hex().as_str()).unwrap()
}

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f87{suffix:02x}")).unwrap()
}
