use classick::device::DeviceId;
use classick::portable::outbox::{PendingDeviceOutbox, PendingMutation};
use classick::portable::profile::{
    MutationId, PortableProfile, SelectionMode, SelectionValue, SettingsValue, SubscriptionsValue,
};
use classick::portable::reconcile::{
    plan_portable_reconciliation, DeviceProfileObservation, PortableReconciliationPlan,
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
              "settings":{{"revision":7,"mutation_id":"{}","value":{{"schema_version":1,"auto_sync":{},"rockbox_compat":false,"transcode_profile":"alac"}}}},
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
        transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
    }
}

fn device_id(value: &str) -> DeviceId {
    DeviceId::parse(value).unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f87{suffix:02x}")).unwrap()
}
