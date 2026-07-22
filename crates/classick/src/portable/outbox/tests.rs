use super::{
    coalesce_pending, parse_outbox, serialize_outbox, PendingDeviceOutbox, PendingMutation,
};
use crate::device::DeviceId;
use crate::portable::profile::{MutationId, SelectionMode, SelectionValue, SettingsValue};

fn device_id(value: &str) -> DeviceId {
    DeviceId::parse(value).unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f87{suffix:02x}")).unwrap()
}

fn selection(device_id: &DeviceId, mutation_id: MutationId) -> PendingMutation {
    PendingMutation::selection(
        mutation_id,
        device_id.clone(),
        SelectionValue {
            schema_version: 1,
            mode: SelectionMode::All,
            rules: vec![],
        },
        0,
    )
    .unwrap()
}

fn settings(device_id: &DeviceId, mutation_id: MutationId, auto_sync: bool) -> PendingMutation {
    PendingMutation::settings(
        mutation_id,
        device_id.clone(),
        SettingsValue {
            schema_version: 1,
            auto_sync,
            rockbox_compat: false,
        },
        7,
    )
    .unwrap()
}

#[test]
fn pure_coalescing_accepts_an_identical_replay_and_rejects_changed_id_reuse() {
    let device = device_id("000A270012345678");
    let pending = settings(&device, mutation_id(1), false);
    let current = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device,
        mutations: vec![pending.clone()],
    };

    assert_eq!(coalesce_pending(&current, &pending).unwrap(), current);
    let changed = settings(&current.device_id, pending.mutation_id().clone(), true);
    assert!(coalesce_pending(&current, &changed).is_err());
}

#[test]
fn pure_coalescing_replaces_only_its_component_in_deterministic_order() {
    let device = device_id("000A270012345678");
    let current = PendingDeviceOutbox {
        schema_version: 1,
        device_id: device.clone(),
        mutations: vec![settings(&device, mutation_id(1), false)],
    };

    let with_selection = coalesce_pending(&current, &selection(&device, mutation_id(2))).unwrap();
    let replaced =
        coalesce_pending(&with_selection, &settings(&device, mutation_id(3), true)).unwrap();

    assert_eq!(replaced.mutations.len(), 2);
    assert_eq!(replaced.mutations[0].component_name(), "selection");
    assert_eq!(replaced.mutations[1].mutation_id(), &mutation_id(3));
}

#[test]
fn pure_coalescing_rejects_a_mutation_for_another_device() {
    let device = device_id("000A270012345678");
    let other = device_id("000A270087654321");
    let current = PendingDeviceOutbox::empty(device);

    assert!(coalesce_pending(&current, &selection(&other, mutation_id(1))).is_err());
}

#[test]
fn private_serialization_round_trips_without_granting_write_authority() {
    let outbox = PendingDeviceOutbox::empty(device_id("000A270012345678"));

    let bytes = serialize_outbox(&outbox).unwrap();

    assert_eq!(parse_outbox(&bytes).unwrap(), outbox);
    assert!(bytes.ends_with(b"\n"));
}
