use classick::device::DeviceId;
use classick::portable::device_store::{profile_path, read_profile, OwnedDeviceProfile};
use classick::portable::profile::{
    MutationId, PortableProfile, ProfileComponent, SelectionMode, SelectionValue, SettingsValue,
    SubscriptionsValue, PORTABLE_PROFILE_SCHEMA_VERSION,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn profile() -> PortableProfile {
    PortableProfile {
        schema_version: PORTABLE_PROFILE_SCHEMA_VERSION,
        device_id: device_id(),
        capability_profile_id: None,
        selection: ProfileComponent {
            revision: 1,
            mutation_id: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8801").unwrap(),
            value: SelectionValue {
                schema_version: 1,
                mode: SelectionMode::All,
                rules: Vec::new(),
            },
        },
        settings: ProfileComponent {
            revision: 1,
            mutation_id: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8802").unwrap(),
            value: SettingsValue {
                schema_version: 1,
                auto_sync: false,
                rockbox_compat: false,
                transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
            },
        },
        subscriptions: ProfileComponent {
            revision: 1,
            mutation_id: MutationId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8803").unwrap(),
            value: SubscriptionsValue {
                schema_version: 1,
                playlists: Vec::new(),
            },
        },
        owned_playlists: Vec::new(),
        companion_authorities: Vec::new(),
        generated_sysinfo_extended_hash: None,
    }
}

fn tempdir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "classick-portable-device-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn strict_profile_reader_recognizes_the_canonical_device_authority() {
    let mount = tempdir();
    std::fs::create_dir_all(mount.join("iPod_Control/classick")).unwrap();
    std::fs::write(profile_path(&mount), profile().to_json_pretty().unwrap()).unwrap();

    assert_eq!(
        read_profile(&mount).unwrap(),
        OwnedDeviceProfile::Valid(profile())
    );
    assert!(profile_path(&mount).starts_with(&mount));
}
