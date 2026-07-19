#[path = "support/playlist_audit_fixture.rs"]
mod playlist_audit_fixture;

use clap::Parser;
use classick::cli::Cli;
use classick::ipod::playlist_audit::InternalCategoryVisibility;
use classick::playlist_audit_command;
use playlist_audit_fixture::AuditFixture;
use std::process::Command;

#[test]
fn audit_json_is_deterministic_and_complete_device_tree_is_unchanged() {
    let fixture = AuditFixture::new();
    let before = fixture.tree_digest();

    let first = playlist_audit_command::run_at(&fixture.mount, &fixture.serial).unwrap();
    let second = playlist_audit_command::run_at(&fixture.mount, &fixture.serial).unwrap();

    assert_eq!(
        serde_json::to_string_pretty(&first).unwrap(),
        serde_json::to_string_pretty(&second).unwrap()
    );
    assert_eq!(fixture.tree_digest(), before);
    assert_eq!(first.playlists.len(), 4);
    assert_eq!(
        first.internal_mhsd5_categories,
        InternalCategoryVisibility::UnsupportedByVendoredLibgpod
    );
}

#[test]
fn audit_flag_conflicts_with_every_mutating_one_shot() {
    for other in [
        "--apply",
        "--dry-run",
        "--rebuild-manifest",
        "--backfill-rockbox",
        "--scan-library",
        "--restore-db-backup",
        "--replace-library",
        "--verify-artwork",
    ] {
        assert!(
            Cli::try_parse_from(["classick", "--audit-playlists", other]).is_err(),
            "accepted {other}"
        );
    }
}

#[test]
fn audit_flag_parses_without_a_source_library() {
    let cli =
        Cli::try_parse_from(["classick", "--audit-playlists", "--ipod", "/tmp/ipod"]).unwrap();
    assert!(cli.audit_playlists);
    assert!(cli.source.is_none());
}

#[test]
fn plain_command_emits_only_pretty_json_without_source_configuration() {
    let fixture = AuditFixture::new();
    let before = fixture.tree_digest();
    let home = fixture.mount.with_extension("audit-home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_classick"))
        .args([
            "--audit-playlists",
            "--no-tui",
            "--ipod",
            fixture.mount.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("appdata"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: classick::ipod::playlist_audit::PlaylistAudit =
        serde_json::from_str(&stdout).expect("plain stdout must contain exactly one JSON value");
    assert_eq!(parsed.playlists.len(), 4);
    assert!(stdout.contains("\n  \"playlists\""));
    assert_eq!(fixture.tree_digest(), before);
}
