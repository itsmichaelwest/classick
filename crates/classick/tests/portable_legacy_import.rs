use classick::daemon::device_registry_v2::{
    plan_registry_v2_migration, LegacyImportEligibility, RegistryMigrationPlan,
};
use classick::device::DeviceId;
use classick::portable::legacy_import::{
    plan_legacy_host_import, LegacyHostFallbacks, LegacyHostFiles, LegacyHostImportPlan,
    LegacyMutationIds, PortableProfileObservation, ResolvedLegacySelection, ResolvedLegacySettings,
};
use classick::portable::outbox::PendingMutation;
use classick::portable::profile::PortableProfile;
use classick::portable::profile::{MutationId, SelectionMode, SelectionRule};

fn mutation_ids() -> LegacyMutationIds {
    LegacyMutationIds {
        selection: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8750").unwrap(),
        settings: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8751").unwrap(),
        subscriptions: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8752").unwrap(),
    }
}

fn default_fallbacks() -> LegacyHostFallbacks<'static> {
    LegacyHostFallbacks {
        selection: ResolvedLegacySelection::All,
        settings: ResolvedLegacySettings::DaemonDefaults,
    }
}

fn eligibility(device_id: &DeviceId) -> LegacyImportEligibility {
    let legacy = format!(
        r#"{{"version":1,"records":[{{"serial":"{}","configured":true}}]}}"#,
        device_id.as_str()
    );
    let RegistryMigrationPlan::Ready { registry } = plan_registry_v2_migration(legacy.as_bytes())
    else {
        panic!("configured legacy registry should migrate");
    };
    registry
        .legacy_import_eligibility(device_id)
        .expect("configured pending record should authorize legacy import")
}

#[test]
fn valid_legacy_values_plan_one_atomic_initial_outbox_and_retain_every_byte() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let selection = br#"{"version":1,"mode":"include","rules":[{"kind":"artist","name":"Birdy"}]}"#;
    let settings = br#"{"version":1,"auto_sync":false,"rockbox_compat":true}"#;
    let subscriptions = br#"{"version":1,"playlists":["favourites"]}"#;
    let managed = b"legacy ownership is diagnostic only";
    let files = LegacyHostFiles {
        selection: Some(selection),
        settings: Some(settings),
        subscriptions: Some(subscriptions),
        managed_playlists: Some(managed),
    };

    let LegacyHostImportPlan::Ready {
        cache,
        outbox,
        retained_legacy,
    } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Absent,
        files,
        default_fallbacks(),
        mutation_ids(),
    )
    else {
        panic!("valid legacy state should plan");
    };

    assert_eq!(cache.device_id, device_id);
    assert_eq!(cache.last_imported_profile, None);
    assert_eq!(outbox.mutations.len(), 3);
    match &outbox.mutations[0] {
        PendingMutation::Selection { desired, .. } => {
            assert_eq!(desired.mode, SelectionMode::Include);
            assert_eq!(
                desired.rules,
                vec![SelectionRule::Artist {
                    name: "Birdy".into()
                }]
            );
        }
        mutation => panic!("unexpected first mutation {mutation:?}"),
    }
    match &outbox.mutations[1] {
        PendingMutation::Settings { desired, .. } => {
            assert!(!desired.auto_sync);
            assert!(desired.rockbox_compat);
        }
        mutation => panic!("unexpected second mutation {mutation:?}"),
    }
    match &outbox.mutations[2] {
        PendingMutation::Subscriptions { desired, .. } => {
            assert_eq!(desired.playlists[0].as_str(), "favourites");
        }
        mutation => panic!("unexpected third mutation {mutation:?}"),
    }
    assert_eq!(
        retained_legacy.selection.as_deref(),
        Some(selection.as_slice())
    );
    assert_eq!(
        retained_legacy.settings.as_deref(),
        Some(settings.as_slice())
    );
    assert_eq!(
        retained_legacy.subscriptions.as_deref(),
        Some(subscriptions.as_slice())
    );
    assert_eq!(
        retained_legacy.managed_playlists.as_deref(),
        Some(managed.as_slice())
    );
}

#[test]
fn corrupt_or_ambiguous_legacy_state_blocks_and_retains_exact_inputs() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let corrupt = b"{ not json";
    let files = LegacyHostFiles {
        selection: Some(corrupt),
        settings: None,
        subscriptions: None,
        managed_playlists: None,
    };

    let LegacyHostImportPlan::Blocked {
        retained_legacy,
        diagnostics,
    } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Absent,
        files,
        default_fallbacks(),
        mutation_ids(),
    )
    else {
        panic!("corrupt legacy state must block");
    };

    assert_eq!(
        retained_legacy.selection.as_deref(),
        Some(corrupt.as_slice())
    );
    assert!(!diagnostics.is_empty());
}

#[test]
fn existing_portable_profile_wins_without_parsing_or_publishing_legacy_values() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let profile = portable_profile();
    let corrupt = b"{ legacy is no longer authority";
    let files = LegacyHostFiles {
        selection: Some(corrupt),
        settings: Some(corrupt),
        subscriptions: Some(corrupt),
        managed_playlists: Some(corrupt),
    };

    let LegacyHostImportPlan::Ready {
        cache,
        outbox,
        retained_legacy,
    } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Valid(&profile),
        files,
        default_fallbacks(),
        mutation_ids(),
    )
    else {
        panic!("valid device profile should remain authoritative");
    };

    assert_eq!(cache.last_imported_profile, Some(profile));
    assert!(outbox.mutations.is_empty());
    assert_eq!(
        retained_legacy.selection.as_deref(),
        Some(corrupt.as_slice())
    );
    assert_eq!(
        retained_legacy.managed_playlists.as_deref(),
        Some(corrupt.as_slice())
    );
}

#[test]
fn absent_device_files_use_explicitly_resolved_legacy_authorities() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let shared_selection =
        br#"{"version":1,"mode":"exclude","rules":[{"kind":"genre","name":"Speech"}]}"#;
    let files = LegacyHostFiles {
        selection: None,
        settings: None,
        subscriptions: None,
        managed_playlists: None,
    };
    let fallbacks = LegacyHostFallbacks {
        selection: ResolvedLegacySelection::SharedFile(shared_selection),
        settings: ResolvedLegacySettings::GlobalConfig {
            auto_sync: false,
            rockbox_compat: true,
            source_bytes: b"[daemon]\nenabled = false\nrockbox_compat = true\n",
        },
    };

    let LegacyHostImportPlan::Ready {
        outbox,
        retained_legacy,
        ..
    } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Absent,
        files,
        fallbacks,
        mutation_ids(),
    )
    else {
        panic!("resolved legacy fallback authorities should import");
    };

    match &outbox.mutations[0] {
        PendingMutation::Selection { desired, .. } => {
            assert_eq!(desired.mode, SelectionMode::Exclude);
            assert_eq!(
                desired.rules,
                vec![SelectionRule::Genre {
                    name: "Speech".into()
                }]
            );
        }
        mutation => panic!("unexpected first mutation {mutation:?}"),
    }
    match &outbox.mutations[1] {
        PendingMutation::Settings { desired, .. } => {
            assert!(!desired.auto_sync);
            assert!(desired.rockbox_compat);
        }
        mutation => panic!("unexpected second mutation {mutation:?}"),
    }
    match &outbox.mutations[2] {
        PendingMutation::Subscriptions { desired, .. } => {
            assert!(desired.playlists.is_empty());
        }
        mutation => panic!("unexpected third mutation {mutation:?}"),
    }
    assert_eq!(
        retained_legacy.shared_selection.as_deref(),
        Some(shared_selection.as_slice())
    );
    assert_eq!(
        retained_legacy.global_config.as_deref(),
        Some(b"[daemon]\nenabled = false\nrockbox_compat = true\n".as_slice())
    );
}

#[test]
fn explicitly_proven_selection_absence_uses_sync_all() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let files = LegacyHostFiles {
        selection: None,
        settings: None,
        subscriptions: None,
        managed_playlists: None,
    };

    let LegacyHostImportPlan::Ready { outbox, .. } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Absent,
        files,
        default_fallbacks(),
        mutation_ids(),
    ) else {
        panic!("explicitly proven selection absence should use the legacy default");
    };

    let PendingMutation::Selection { desired, .. } = &outbox.mutations[0] else {
        panic!("first mutation should be selection");
    };
    assert_eq!(desired.mode, SelectionMode::All);
    assert!(desired.rules.is_empty());
}

#[test]
fn malformed_present_portable_profile_blocks_legacy_adoption() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let files = LegacyHostFiles {
        selection: None,
        settings: None,
        subscriptions: None,
        managed_playlists: None,
    };

    let LegacyHostImportPlan::Blocked { diagnostics, .. } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Invalid("profile.json has unsupported schema 99"),
        files,
        default_fallbacks(),
        mutation_ids(),
    ) else {
        panic!("present invalid device authority must block");
    };
    assert!(diagnostics[0].contains("unsupported schema 99"));
}

#[test]
fn unconfigured_or_complete_registry_record_cannot_authorize_legacy_adoption() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let legacy = br#"{"version":1,"records":[{"serial":"000A27002138B0A8","configured":false}]}"#;
    let RegistryMigrationPlan::Ready { registry } = plan_registry_v2_migration(legacy) else {
        panic!("unconfigured legacy registry should migrate");
    };
    assert!(registry.legacy_import_eligibility(&device_id).is_none());
}

#[test]
fn missing_legacy_subscriptions_version_blocks() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let subscriptions = br#"{"playlists":["favourites"]}"#;
    let files = LegacyHostFiles {
        selection: None,
        settings: None,
        subscriptions: Some(subscriptions),
        managed_playlists: None,
    };

    let LegacyHostImportPlan::Blocked {
        retained_legacy, ..
    } = plan_legacy_host_import(
        &eligibility(&device_id),
        PortableProfileObservation::Absent,
        files,
        default_fallbacks(),
        mutation_ids(),
    )
    else {
        panic!("missing subscriptions version must remain ambiguous");
    };
    assert_eq!(
        retained_legacy.subscriptions.as_deref(),
        Some(subscriptions.as_slice())
    );
}

#[test]
fn mismatched_profile_and_duplicate_mutation_ids_are_rejected() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let other_profile = PortableProfile::from_json(
        &portable_profile()
            .to_json_pretty()
            .unwrap()
            .replace("000A27002138B0A8", "000A27002138B0A9"),
    )
    .unwrap();
    let files = LegacyHostFiles {
        selection: None,
        settings: None,
        subscriptions: None,
        managed_playlists: None,
    };
    assert!(matches!(
        plan_legacy_host_import(
            &eligibility(&device_id),
            PortableProfileObservation::Valid(&other_profile),
            files,
            default_fallbacks(),
            mutation_ids(),
        ),
        LegacyHostImportPlan::Blocked { .. }
    ));

    let mut duplicate_ids = mutation_ids();
    duplicate_ids.settings = duplicate_ids.selection.clone();
    assert!(matches!(
        plan_legacy_host_import(
            &eligibility(&device_id),
            PortableProfileObservation::Absent,
            files,
            default_fallbacks(),
            duplicate_ids,
        ),
        LegacyHostImportPlan::Blocked { .. }
    ));
}

#[test]
fn unsupported_versions_and_invalid_subscription_sets_block() {
    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    for (selection, settings, subscriptions) in [
        (Some(br#"{"version":2}"#.as_slice()), None, None),
        (None, Some(br#"{"version":2}"#.as_slice()), None),
        (
            None,
            None,
            Some(br#"{"version":2,"playlists":[]}"#.as_slice()),
        ),
        (
            None,
            None,
            Some(br#"{"version":1,"playlists":["same","same"]}"#.as_slice()),
        ),
        (
            None,
            None,
            Some(br#"{"version":1,"playlists":["../unsafe"]}"#.as_slice()),
        ),
    ] {
        let files = LegacyHostFiles {
            selection,
            settings,
            subscriptions,
            managed_playlists: None,
        };
        assert!(matches!(
            plan_legacy_host_import(
                &eligibility(&device_id),
                PortableProfileObservation::Absent,
                files,
                default_fallbacks(),
                mutation_ids(),
            ),
            LegacyHostImportPlan::Blocked { .. }
        ));
    }
}

fn portable_profile() -> PortableProfile {
    PortableProfile::from_json(
        r#"{
          "schema_version": 1,
          "device_id": "000A27002138B0A8",
          "selection": {
            "revision": 1,
            "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8760",
            "value": {"schema_version":1,"mode":"all","rules":[]}
          },
          "settings": {
            "revision": 1,
            "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8761",
            "value": {"schema_version":1,"auto_sync":false,"rockbox_compat":false}
          },
          "subscriptions": {
            "revision": 1,
            "mutation_id": "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8762",
            "value": {"schema_version":1,"playlists":[]}
          },
          "owned_playlists": [],
          "companion_authorities": []
        }"#,
    )
    .unwrap()
}
