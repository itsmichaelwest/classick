use classick::device::DeviceId;
use classick::portable::outbox::{PendingDeviceOutbox, PendingMutation};
use classick::portable::profile::{
    CompanionAuthority, ContentHash, MutationId, PlaylistSlug, PortableProfile, ProfilePath,
    SelectionMode, SelectionValue, SettingsValue, SubscriptionsValue,
};
use classick::portable::reconcile::{
    confirm_reconciled_profile, plan_portable_reconciliation, CommitConfirmation,
    CompanionFileReadback, DeviceProfileObservation, PortableReconciliationPlan,
    ProfilePublicationContext,
};

#[test]
fn pending_host_setting_wins_and_outbox_remains_until_confirmation() {
    let device_id = device_id("000A27002138B0A8");
    let profile = profile(&device_id, false);
    let outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            mutation_id(10),
            device_id.clone(),
            settings(true, true),
            profile.settings.revision,
        )
        .unwrap()],
    };

    let PortableReconciliationPlan::PublishPending { publication } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Valid(&profile),
        &outbox,
        &ProfilePublicationContext::default(),
    ) else {
        panic!("pending host intent should publish");
    };

    let candidate_profile = publication.candidate_profile();
    assert!(candidate_profile.settings.value.auto_sync);
    assert!(candidate_profile.settings.value.rockbox_compat);
    assert_eq!(candidate_profile.settings.revision, 8);
    assert_eq!(candidate_profile.settings.mutation_id, mutation_id(10));
    assert_eq!(candidate_profile.selection, profile.selection);
    assert_eq!(candidate_profile.owned_playlists, profile.owned_playlists);
    assert_eq!(publication.retained_outbox(), &outbox);
}

#[test]
fn subscription_publication_replaces_playlist_definition_authority() {
    let device_id = device_id("000A27002138B0A8");
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
    assert_eq!(
        publication.candidate_profile().companion_authorities.len(),
        2
    );
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
    let stale = CompanionFileReadback {
        relative_path: &definition_path,
        bytes: Some(b"stale"),
    };
    let manifest = CompanionFileReadback {
        relative_path: &manifest_path,
        bytes: Some(manifest_bytes),
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
    let manifest = CompanionFileReadback {
        relative_path: &manifest_path,
        bytes: Some(manifest_bytes),
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

#[test]
fn no_pending_intent_imports_the_connected_device_profile() {
    let device_id = device_id("000A27002138B0A8");
    let profile = profile(&device_id, false);
    let outbox = PendingDeviceOutbox::empty(device_id.clone());

    let PortableReconciliationPlan::ImportDevice { cache } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Valid(&profile),
        &outbox,
        &ProfilePublicationContext::default(),
    ) else {
        panic!("device should win when the host has no pending intent");
    };
    assert_eq!(cache.last_imported_profile, Some(profile));
}

#[test]
fn identical_mutation_replay_is_idempotent_but_changed_reuse_blocks() {
    let device_id = device_id("000A27002138B0A8");
    let profile = profile(&device_id, false);
    let replay = PendingMutation::settings(
        profile.settings.mutation_id.clone(),
        device_id.clone(),
        profile.settings.value.clone(),
        1,
    )
    .unwrap();
    let replay_outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![replay],
    };
    let PortableReconciliationPlan::PublishPending { publication } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Valid(&profile),
        &replay_outbox,
        &ProfilePublicationContext::default(),
    ) else {
        panic!("identical replay should remain publishable for readback confirmation");
    };
    assert_eq!(publication.candidate_profile().settings, profile.settings);

    let subscription_replay = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::subscriptions(
            profile.subscriptions.mutation_id.clone(),
            device_id.clone(),
            profile.subscriptions.value.clone(),
            1,
        )
        .unwrap()],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Valid(&profile),
            &subscription_replay,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::PublishPending { .. }
    ));

    let changed = PendingMutation::settings(
        profile.settings.mutation_id.clone(),
        device_id.clone(),
        settings(true, false),
        1,
    )
    .unwrap();
    let changed_outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![changed],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Valid(&profile),
            &changed_outbox,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));
}

#[test]
fn revision_overflow_and_cross_component_mutation_reuse_block() {
    let device_id = device_id("000A27002138B0A8");
    let mut overflow = profile(&device_id, false);
    overflow.settings.revision = u64::MAX;
    let overflow_outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            mutation_id(30),
            device_id.clone(),
            settings(true, false),
            u64::MAX,
        )
        .unwrap()],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Valid(&overflow),
            &overflow_outbox,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));

    let profile = profile(&device_id, false);
    let reused = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            profile.selection.mutation_id.clone(),
            device_id.clone(),
            settings(true, false),
            profile.settings.revision,
        )
        .unwrap()],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Valid(&profile),
            &reused,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));
}

#[test]
fn exact_readback_is_required_before_clearing_pending_intent() {
    let device_id = device_id("000A27002138B0A8");
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
    let CommitConfirmation::Pending {
        retained_outbox, ..
    } = confirm_reconciled_profile(
        &device_id,
        &publication,
        DeviceProfileObservation::Valid(&changed),
        &[],
    )
    else {
        panic!("non-exact readback must retain intent");
    };
    assert_eq!(retained_outbox, outbox);

    let newer_outbox = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![PendingMutation::settings(
            mutation_id(11),
            device_id.clone(),
            settings(true, false),
            expected.settings.revision,
        )
        .unwrap()],
    };
    assert!(outbox_clear.apply_to(&newer_outbox).is_err());
}

#[test]
fn absent_profile_requires_a_complete_initial_mutation_set() {
    let device_id = device_id("000A27002138B0A8");
    let complete = initial_outbox(&device_id);
    let PortableReconciliationPlan::PublishPending { publication } = plan_portable_reconciliation(
        &device_id,
        DeviceProfileObservation::Absent,
        &complete,
        &ProfilePublicationContext::default(),
    ) else {
        panic!("complete explicit initial host state should adopt");
    };
    assert_eq!(publication.candidate_profile().selection.revision, 1);
    assert_eq!(publication.candidate_profile().settings.revision, 1);
    assert_eq!(publication.candidate_profile().subscriptions.revision, 1);

    let incomplete = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![complete.mutations[1].clone()],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Absent,
            &incomplete,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));

    let based_on_lost_profile = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![
            PendingMutation::selection(
                mutation_id(20),
                device_id.clone(),
                SelectionValue {
                    schema_version: 1,
                    mode: SelectionMode::All,
                    rules: vec![],
                },
                4,
            )
            .unwrap(),
            complete.mutations[1].clone(),
            complete.mutations[2].clone(),
        ],
    };
    assert!(matches!(
        plan_portable_reconciliation(
            &device_id,
            DeviceProfileObservation::Absent,
            &based_on_lost_profile,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));
}

#[test]
fn invalid_or_cross_device_authority_blocks_without_partial_output() {
    let target_id = device_id("000A27002138B0A8");
    let other_id = device_id("000A27002138B0A9");
    let outbox = PendingDeviceOutbox::empty(target_id.clone());
    assert!(matches!(
        plan_portable_reconciliation(
            &target_id,
            DeviceProfileObservation::Invalid("unsupported profile schema"),
            &outbox,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));
    assert!(matches!(
        plan_portable_reconciliation(
            &target_id,
            DeviceProfileObservation::Valid(&profile(&other_id, false)),
            &outbox,
            &ProfilePublicationContext::default(),
        ),
        PortableReconciliationPlan::Blocked { .. }
    ));
}

fn initial_outbox(device_id: &DeviceId) -> PendingDeviceOutbox {
    PendingDeviceOutbox {
        schema_version: 1,
        device_id: device_id.clone(),
        mutations: vec![
            PendingMutation::selection(
                mutation_id(20),
                device_id.clone(),
                SelectionValue {
                    schema_version: 1,
                    mode: SelectionMode::All,
                    rules: vec![],
                },
                0,
            )
            .unwrap(),
            PendingMutation::settings(mutation_id(21), device_id.clone(), settings(false, true), 0)
                .unwrap(),
            PendingMutation::subscriptions(
                mutation_id(22),
                device_id.clone(),
                SubscriptionsValue {
                    schema_version: 1,
                    playlists: vec![],
                },
                0,
            )
            .unwrap(),
        ],
    }
}

fn profile(device_id: &DeviceId, auto_sync: bool) -> PortableProfile {
    PortableProfile::from_json(
        &format!(
            r#"{{
              "schema_version":1,
              "device_id":"{}",
              "selection":{{"revision":3,"mutation_id":"{}","value":{{"schema_version":1,"mode":"all","rules":[]}}}},
              "settings":{{"revision":7,"mutation_id":"{}","value":{{"schema_version":1,"auto_sync":{},"rockbox_compat":false}}}},
              "subscriptions":{{"revision":4,"mutation_id":"{}","value":{{"schema_version":1,"playlists":[]}}}},
              "owned_playlists":[],
              "companion_authorities":[]
            }}"#,
            device_id.as_str(),
            mutation_id(1),
            mutation_id(2),
            auto_sync,
            mutation_id(3),
        ),
    )
    .unwrap()
}

fn settings(auto_sync: bool, rockbox_compat: bool) -> SettingsValue {
    SettingsValue {
        schema_version: 1,
        auto_sync,
        rockbox_compat,
    }
}

fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash::parse(blake3::hash(bytes).to_hex().as_str()).unwrap()
}

fn device_id(value: &str) -> DeviceId {
    DeviceId::parse(value).unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f87{suffix:02x}")).unwrap()
}
