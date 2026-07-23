use super::*;
use crate::config_file::IpodIdentity;
use crate::selection::{Selection, SelectionMode};
use std::sync::atomic::{AtomicU32, Ordering};

fn fixture(label: &str) -> (PathBuf, DeviceRegistry) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "device-config-transaction-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let registry = DeviceRegistry::load_or_migrate(
        root.join("devices.json"),
        Some(&IpodIdentity {
            serial: "RAW-A".into(),
            model_label: "iPod Classic".into(),
            name: None,
            custom_selection: true,
        }),
    )
    .unwrap();
    (root, registry)
}

fn selection_bytes(mode: SelectionMode) -> Vec<u8> {
    serde_json::to_vec_pretty(&Selection {
        version: crate::selection::SELECTION_VERSION,
        mode,
        rules: vec![],
    })
    .unwrap()
}

fn journal_with_components(
    root: &Path,
    request_id: &str,
    components: Vec<JournalComponent>,
) -> PathBuf {
    let path = journal_path(&journal_root(root), request_id);
    save_journal(
        &AtomicFileWriter::new(),
        &path,
        &MutationJournal {
            version: JOURNAL_VERSION,
            request_id: request_id.into(),
            serial: "RAW-A".into(),
            original_revisions: Revisions {
                selection: 0,
                settings: 0,
                subscriptions: 0,
            },
            components,
        },
    )
    .unwrap();
    path
}

#[test]
fn original_revisions_restore_exact_original_bytes() {
    let (root, registry) = fixture("restore-original");
    let live = crate::device_state::device_selection_path_in(&root, "RAW-A").unwrap();
    let original = selection_bytes(SelectionMode::All);
    let target = selection_bytes(SelectionMode::Include);
    AtomicFileWriter::new().write(&live, &target).unwrap();
    let journal = journal_with_components(
        &root,
        "restore-original",
        vec![JournalComponent {
            kind: ConfigComponentKind::Selection,
            live_path: relative_to(&root, &live).unwrap(),
            original_contents: Some(original.clone()),
            target_contents: target,
        }],
    );

    recover_pending(&registry, &root).unwrap();

    assert_eq!(std::fs::read(live).unwrap(), original);
    assert!(!journal.exists());
}

#[test]
fn original_revisions_reject_unexpected_bytes_or_absence() {
    for (label, actual) in [
        (
            "unexpected-bytes",
            Some(selection_bytes(SelectionMode::Exclude)),
        ),
        ("unexpected-absence", None),
    ] {
        let (root, registry) = fixture(label);
        let live = crate::device_state::device_selection_path_in(&root, "RAW-A").unwrap();
        let original = selection_bytes(SelectionMode::All);
        let target = selection_bytes(SelectionMode::Include);
        if let Some(actual) = actual {
            AtomicFileWriter::new().write(&live, &actual).unwrap();
        }
        let journal = journal_with_components(
            &root,
            label,
            vec![JournalComponent {
                kind: ConfigComponentKind::Selection,
                live_path: relative_to(&root, &live).unwrap(),
                original_contents: Some(original),
                target_contents: target,
            }],
        );

        let error = recover_pending(&registry, &root).unwrap_err();

        assert!(error.to_string().contains("differs from journal"));
        assert!(journal.exists());
    }
}

#[test]
fn target_revisions_accept_only_exact_published_target() {
    let (root, mut registry) = fixture("accept-target");
    let live = crate::device_state::device_selection_path_in(&root, "RAW-A").unwrap();
    let original = selection_bytes(SelectionMode::All);
    let target = selection_bytes(SelectionMode::Include);
    AtomicFileWriter::new().write(&live, &target).unwrap();
    let journal = journal_with_components(
        &root,
        "accept-target",
        vec![JournalComponent {
            kind: ConfigComponentKind::Selection,
            live_path: relative_to(&root, &live).unwrap(),
            original_contents: Some(original),
            target_contents: target.clone(),
        }],
    );
    registry
        .advance_config_revisions("RAW-A", true, false, false)
        .unwrap();

    recover_pending(&registry, &root).unwrap();

    assert_eq!(std::fs::read(live).unwrap(), target);
    assert!(!journal.exists());
}

#[test]
fn mixed_revisions_fail_recovery_and_retain_journal() {
    let (root, mut registry) = fixture("reject-mixed");
    let selection_live = crate::device_state::device_selection_path_in(&root, "RAW-A").unwrap();
    let settings_live = crate::device_state::device_settings_path_in(&root, "RAW-A").unwrap();
    let selection_target = selection_bytes(SelectionMode::Include);
    let settings_original =
        serde_json::to_vec_pretty(&crate::device_config::DeviceSettings::default()).unwrap();
    let settings_target = serde_json::to_vec_pretty(&crate::device_config::DeviceSettings {
        version: crate::device_config::DEVICE_SETTINGS_VERSION,
        auto_sync: false,
        rockbox_compat: true,
        transcode_profile: crate::portable::profile::TranscodeProfile::Aac192,
    })
    .unwrap();
    AtomicFileWriter::new()
        .write(&selection_live, &selection_target)
        .unwrap();
    AtomicFileWriter::new()
        .write(&settings_live, &settings_target)
        .unwrap();
    let journal = journal_with_components(
        &root,
        "reject-mixed",
        vec![
            JournalComponent {
                kind: ConfigComponentKind::Selection,
                live_path: relative_to(&root, &selection_live).unwrap(),
                original_contents: Some(selection_bytes(SelectionMode::All)),
                target_contents: selection_target,
            },
            JournalComponent {
                kind: ConfigComponentKind::Settings,
                live_path: relative_to(&root, &settings_live).unwrap(),
                original_contents: Some(settings_original),
                target_contents: settings_target,
            },
        ],
    );
    registry
        .advance_config_revisions("RAW-A", true, false, false)
        .unwrap();

    let error = recover_pending(&registry, &root).unwrap_err();

    assert!(error.to_string().contains("mixed or unexpected revisions"));
    assert!(journal.exists());
}
