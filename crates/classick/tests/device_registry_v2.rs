use classick::daemon::device_registry_v2::{
    plan_registry_v2_migration, DeviceRegistryV2, RegistryMigrationPlan, RegistryMigrationStatus,
};
use classick::device::DeviceId;

#[test]
fn valid_v1_bytes_plan_one_canonical_registry_without_writing() {
    let legacy = br#"{
      "version": 1,
      "records": [{
        "serial": "0x000a27002138b0a8",
        "model_label": "iPod Classic (Late 2009)",
        "name": "Michael West's iPod",
        "configured": true,
        "last_seen_unix_secs": 1234,
        "selection_revision": 7,
        "settings_revision": 8,
        "subscriptions_revision": 9
      }]
    }"#;

    let RegistryMigrationPlan::Ready { registry } = plan_registry_v2_migration(legacy) else {
        panic!("valid legacy registry should be migratable");
    };

    let device_id = DeviceId::parse("000A27002138B0A8").unwrap();
    let record = registry.device(&device_id).unwrap();
    assert!(record.configured);
    assert_eq!(
        record.presentation.name.as_deref(),
        Some("Michael West's iPod")
    );
    assert_eq!(
        record.presentation.model_label.as_deref(),
        Some("iPod Classic (Late 2009)")
    );
    assert_eq!(record.last_seen_unix_secs, Some(1234));
    assert_eq!(record.last_storage, None);
    assert_eq!(record.last_readiness, None);
    assert_eq!(record.last_imported_profile, None);
    assert_eq!(
        record.migration_status,
        RegistryMigrationStatus::PendingLegacyImport {
            selection_revision: 7,
            settings_revision: 8,
            subscriptions_revision: 9,
        }
    );

    let json = registry.to_json_pretty().unwrap();
    assert!(json.contains("\"schema_version\": 2"));
    assert!(json.contains("\"000A27002138B0A8\""));
    assert!(!json.contains("0x000a27002138b0a8"));
    assert_eq!(legacy[0], b'{', "planner must not mutate its input bytes");
}

#[test]
fn invalid_or_colliding_legacy_ids_block_without_a_partial_registry() {
    let invalid = br#"{"version":1,"records":[{"serial":"not-an-id"}]}"#;
    let collision = br#"{"version":1,"records":[
      {"serial":"000A27002138B0A8"},
      {"serial":"0x000a27002138b0a8"}
    ]}"#;

    for legacy in [invalid.as_slice(), collision.as_slice()] {
        let RegistryMigrationPlan::Blocked {
            retained_legacy_bytes,
            diagnostics,
        } = plan_registry_v2_migration(legacy)
        else {
            panic!("ambiguous legacy registry must block");
        };
        assert_eq!(retained_legacy_bytes, legacy);
        assert!(!diagnostics.is_empty());
    }
}

#[test]
fn zero_revision_configured_and_forgotten_v1_records_migrate_without_inventing_authority() {
    let legacy = br#"{"version":1,"records":[
      {"serial":"000A27002138B0A8","configured":true},
      {"serial":"000A27002138B0A9","configured":false}
    ]}"#;

    let RegistryMigrationPlan::Ready { registry } = plan_registry_v2_migration(legacy) else {
        panic!("known v1 defaults should migrate");
    };
    let configured = registry
        .device(&DeviceId::parse("000A27002138B0A8").unwrap())
        .unwrap();
    assert_eq!(
        configured.migration_status,
        RegistryMigrationStatus::PendingLegacyImport {
            selection_revision: 0,
            settings_revision: 0,
            subscriptions_revision: 0,
        }
    );
    let forgotten = registry
        .device(&DeviceId::parse("000A27002138B0A9").unwrap())
        .unwrap();
    assert_eq!(
        forgotten.migration_status,
        RegistryMigrationStatus::Complete
    );
    assert_eq!(forgotten.last_imported_profile, None);

    let json = registry.to_json_pretty().unwrap();
    assert_eq!(DeviceRegistryV2::from_json(&json).unwrap(), registry);
}

#[test]
fn v2_schema_rejects_noncanonical_keys_and_unaccounted_metadata() {
    let noncanonical = r#"{
      "schema_version": 2,
      "devices": {
        "000a27002138b0a8": {
          "configured": false,
          "presentation": {"hardware_facts": {}},
          "migration_status": {"state": "complete"}
        }
      }
    }"#;
    let unaccounted = r#"{
      "schema_version": 2,
      "devices": {
        "000A27002138B0A8": {
          "configured": false,
          "presentation": {"hardware_facts": {}},
          "migration_status": {"state": "complete"},
          "generated_sysinfo_extended_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
      }
    }"#;

    assert!(DeviceRegistryV2::from_json(noncanonical).is_err());
    assert!(DeviceRegistryV2::from_json(unaccounted).is_err());
}

#[test]
fn v2_schema_rejects_duplicate_device_keys_and_unknown_nested_metadata() {
    let record = r#"{
      "configured": false,
      "presentation": {"hardware_facts": {}},
      "migration_status": {"state": "complete"}
    }"#;
    let duplicate = format!(
        r#"{{
      "schema_version": 2,
      "devices": {{
        "000A27002138B0A8": {record},
        "000A27002138B0A8": {record}
      }}
    }}"#
    );
    let unknown_fact = r#"{
      "schema_version": 2,
      "devices": {"000A27002138B0A8": {
        "configured": false,
        "presentation": {"hardware_facts": {"appearance_override": "black"}},
        "migration_status": {"state": "complete"}
      }}
    }"#;
    let unknown_storage = r#"{
      "schema_version": 2,
      "devices": {"000A27002138B0A8": {
        "configured": false,
        "presentation": {"hardware_facts": {}},
        "last_storage": {"total_bytes": 10, "free_bytes": 5, "battery": 99},
        "migration_status": {"state": "complete"}
      }}
    }"#;

    assert!(DeviceRegistryV2::from_json(&duplicate).is_err());
    assert!(DeviceRegistryV2::from_json(unknown_fact).is_err());
    assert!(DeviceRegistryV2::from_json(unknown_storage).is_err());
}

#[test]
fn v2_schema_rejects_impossible_migration_and_import_states() {
    let forgotten_pending = registry_with_record(
        false,
        r#"{"state":"pending_legacy_import","selection_revision":0,"settings_revision":0,"subscriptions_revision":0}"#,
        "",
    );
    let pending_with_import = registry_with_record(
        true,
        r#"{"state":"pending_legacy_import","selection_revision":1,"settings_revision":1,"subscriptions_revision":1}"#,
        r#","last_imported_profile":{"schema_version":1,"selection_revision":1,"settings_revision":1,"subscriptions_revision":1}"#,
    );
    let configured_complete_without_import =
        registry_with_record(true, r#"{"state":"complete"}"#, "");
    let unsupported_import = registry_with_record(
        true,
        r#"{"state":"complete"}"#,
        r#","last_imported_profile":{"schema_version":99,"selection_revision":1,"settings_revision":1,"subscriptions_revision":1}"#,
    );
    let impossible_storage = registry_with_record(
        false,
        r#"{"state":"complete"}"#,
        r#","last_storage":{"total_bytes":10,"free_bytes":11}"#,
    );

    for json in [
        forgotten_pending,
        pending_with_import,
        configured_complete_without_import,
        unsupported_import,
        impossible_storage,
    ] {
        assert!(DeviceRegistryV2::from_json(&json).is_err(), "{json}");
    }
}

fn registry_with_record(configured: bool, migration_status: &str, extra: &str) -> String {
    format!(
        r#"{{
          "schema_version": 2,
          "devices": {{"000A27002138B0A8": {{
            "configured": {configured},
            "presentation": {{"hardware_facts": {{}}}},
            "migration_status": {migration_status}{extra}
          }}}}
        }}"#
    )
}
