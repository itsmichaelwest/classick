use classick::device::{DeviceId, DeviceReadiness};
use classick::device_coordination::DeviceMutationSession;
use classick::ipod::{
    project_sysinfo_extended, resolve_validated_capability_profile, CapabilityProfileId,
};
use classick::pending_session::{PendingSession, PendingSessionStore};
use classick::portable::coordinator::{
    publish_manifest_authority, reconcile_connected, ConnectedReconciliation,
};
use classick::portable::device_store::{read_profile, OwnedDeviceProfile};
use classick::portable::outbox::PendingMutation;
use classick::portable::profile::{
    CompanionAuthority, MutationId, SelectionMode, SelectionValue, SettingsValue,
    SubscriptionsValue,
};
use classick::portable::state_store::PortableStateStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn mutation_id(suffix: u8) -> MutationId {
    MutationId::parse(&format!("018f9d7e-2f2b-7b52-9f1d-f78bdb2f88{suffix:02x}")).unwrap()
}

fn fixture(label: &str) -> (PathBuf, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "classick-portable-coordinator-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let mount = root.join("mount");
    let host = root.join("host");
    std::fs::create_dir_all(mount.join("iPod_Control/iTunes")).unwrap();
    std::fs::create_dir_all(mount.join("iPod_Control/Device")).unwrap();
    std::fs::create_dir_all(&host).unwrap();
    (mount, host)
}

fn accept_adoption(host: &PathBuf) {
    let store = PortableStateStore::new(host);
    store
        .accept_mutation(
            &PendingMutation::selection(
                mutation_id(1),
                device_id(),
                SelectionValue {
                    schema_version: 1,
                    mode: SelectionMode::All,
                    rules: Vec::new(),
                },
                0,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .accept_mutation(
            &PendingMutation::settings(
                mutation_id(2),
                device_id(),
                SettingsValue {
                    schema_version: 1,
                    auto_sync: false,
                    rockbox_compat: false,
                    transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
                },
                0,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .accept_mutation(
            &PendingMutation::subscriptions(
                mutation_id(3),
                device_id(),
                SubscriptionsValue {
                    schema_version: 1,
                    playlists: Vec::new(),
                },
                0,
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
fn adoption_commits_profile_and_validated_sysinfo_then_clears_host_intent() {
    let (mount, host) = fixture("generate");
    accept_adoption(&host);
    let session = DeviceMutationSession::acquire(&mount, device_id()).unwrap();

    let outcome =
        reconcile_connected(&host, &session, DeviceReadiness::Ready, Some("MC293")).unwrap();

    let ConnectedReconciliation::DeviceCommitted(state) = outcome else {
        panic!("adoption should commit");
    };
    assert!(state.outbox.mutations.is_empty());
    let OwnedDeviceProfile::Valid(profile) = read_profile(&mount).unwrap() else {
        panic!("profile should be valid");
    };
    assert!(profile.generated_sysinfo_extended_hash.is_some());
    assert!(mount.join("iPod_Control/Device/SysInfoExtended").is_file());
    let mut classick_files = std::fs::read_dir(mount.join("iPod_Control/classick"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    classick_files.sort();
    assert_eq!(classick_files, ["device.lock", "profile.json"]);
}

#[test]
fn preexisting_unowned_sysinfo_is_preserved_byte_for_byte() {
    let (mount, host) = fixture("foreign");
    accept_adoption(&host);
    let profile_id = CapabilityProfileId::parse("classic-late-2009-v1").unwrap();
    let validated = resolve_validated_capability_profile(&profile_id)
        .unwrap()
        .unwrap();
    let foreign = project_sysinfo_extended(&device_id(), &validated)
        .unwrap()
        .bytes()
        .to_vec();
    let path = mount.join("iPod_Control/Device/SysInfoExtended");
    std::fs::write(&path, &foreign).unwrap();
    let session = DeviceMutationSession::acquire(&mount, device_id()).unwrap();

    reconcile_connected(&host, &session, DeviceReadiness::Ready, Some("MC293")).unwrap();

    assert_eq!(std::fs::read(path).unwrap(), foreign);
    let OwnedDeviceProfile::Valid(profile) = read_profile(&mount).unwrap() else {
        panic!("profile should be valid");
    };
    assert!(profile.generated_sysinfo_extended_hash.is_none());
}

#[test]
fn pending_sync_blocks_portable_publication_before_any_device_write() {
    let (mount, host) = fixture("pending-sync");
    accept_adoption(&host);
    PendingSessionStore::new(&mount)
        .save(&PendingSession::new(41, device_id().as_str(), Vec::new()))
        .unwrap();
    let session = DeviceMutationSession::acquire(&mount, device_id()).unwrap();

    let error =
        reconcile_connected(&host, &session, DeviceReadiness::Ready, Some("MC293")).unwrap_err();

    assert!(format!("{error:#}").contains("pending sync transaction"));
    assert_eq!(read_profile(&mount).unwrap(), OwnedDeviceProfile::Absent);
    assert_eq!(
        PortableStateStore::new(&host)
            .load(&device_id())
            .unwrap()
            .outbox
            .mutations
            .len(),
        3
    );
}

#[test]
fn manifest_publication_is_attested_in_the_portable_profile() {
    let (mount, host) = fixture("manifest-authority");
    accept_adoption(&host);
    let session = DeviceMutationSession::acquire(&mount, device_id()).unwrap();
    reconcile_connected(&host, &session, DeviceReadiness::Ready, Some("MC293")).unwrap();
    let manifest = br#"{"version":2,"source":{"resolved_path":"source"},"tracks":[]}"#;
    session
        .publish_verified(|| {
            std::fs::write(mount.join("iPod_Control/classick/manifest.json"), manifest)
                .map_err(anyhow::Error::from)
        })
        .unwrap();

    assert!(publish_manifest_authority(&session).unwrap());
    assert!(!publish_manifest_authority(&session).unwrap());

    let OwnedDeviceProfile::Valid(profile) = read_profile(&mount).unwrap() else {
        panic!("profile should be valid");
    };
    assert_eq!(profile.companion_authorities.len(), 1);
    let CompanionAuthority::Manifest {
        schema_version,
        relative_path,
        content_hash,
    } = &profile.companion_authorities[0]
    else {
        panic!("expected manifest authority");
    };
    assert_eq!(*schema_version, 1);
    assert_eq!(relative_path.as_str(), "manifest.json");
    assert_eq!(
        content_hash.as_str(),
        blake3::hash(manifest).to_hex().as_str()
    );

    PortableStateStore::new(&host)
        .accept_mutation(
            &PendingMutation::settings(
                mutation_id(4),
                device_id(),
                SettingsValue {
                    schema_version: 1,
                    auto_sync: true,
                    rockbox_compat: false,
                    transcode_profile: classick::portable::profile::TranscodeProfile::Alac,
                },
                1,
            )
            .unwrap(),
        )
        .unwrap();
    let outcome =
        reconcile_connected(&host, &session, DeviceReadiness::Ready, Some("MC293")).unwrap();
    assert!(matches!(
        outcome,
        ConnectedReconciliation::DeviceCommitted(_)
    ));
}
