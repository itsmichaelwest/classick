//! Daemon-side IPC wire types for the UI ↔ daemon channel (named pipe
//! / Unix socket). Distinct from `src/ipc.rs` (which is the daemon ↔
//! sync-subprocess channel). Same envelope shape: newline-delimited
//! JSON, snake_case "type" discriminator, additive.
//!
//! Spec §7. Protocol semver: daemon emits hello with
//! `protocol_version = "1.5.0"` since this extends M1's "1.0.0".

use crate::config_file::{DaemonSettings, IpodIdentity};
use crate::daemon::device_storage::StorageInfo;
use crate::daemon::history::HistoryEntry;
use crate::selection::{SelectionMode, SelectionRule};
use serde::{Deserialize, Serialize};

pub const DAEMON_PROTOCOL_VERSION: &str = "1.5.0";

/// Events from daemon → UI clients (in addition to forwarded sync-
/// subprocess events from `src/ipc.rs`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    Hello {
        protocol_version: String,
        core_version: String,
    },
    StatusUpdate {
        state: DaemonStateLabel,
        configured: bool,
        ipod_connected: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_sync: Option<HistoryEntry>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_scheduled_unix_secs: Option<u64>,
        /// Free + total bytes on the iPod's drive. `None` when no iPod
        /// is connected, or when the drive query failed (treat absence
        /// as "no info yet" on the UI). Always `None` on non-Windows
        /// platforms until a native `statvfs`/`statfs` impl lands.
        #[serde(skip_serializing_if = "Option::is_none")]
        storage: Option<StorageInfo>,
        /// Tracks currently on the iPod per the manifest (X in "X of Y
        /// synced"). Always available — a fresh manifest read.
        synced_count: usize,
        /// Source-library track count (Y). `None` until known — the
        /// daemon doesn't walk the source on every status tick; this is
        /// populated from the most recent sync's action-plan diff (which
        /// already walks the source) and cached from there.
        #[serde(skip_serializing_if = "Option::is_none")]
        library_count: Option<usize>,
    },
    ConfigUpdate {
        source: Option<String>,
        daemon: Option<DaemonSettings>,
        ipod: Option<IpodIdentity>,
    },
    HistoryUpdate {
        entries: Vec<HistoryEntry>,
    },
    DeviceConnected {
        serial: String,
        model_label: String,
        drive: String,
        /// iPod's user-set name from the iTunesDB master-playlist name
        /// (e.g. "Michael's iPod"). May be `None` if the daemon hasn't
        /// finished reading the DB yet, or the DB read failed.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    DeviceDisconnected {
        serial: String,
    },
    SyncRejected {
        reason: SyncRejectReason,
    },
    /// Forwarded sync-subprocess event. `line` is the raw JSON line
    /// the subprocess emitted on its stdout, unparsed. UI clients
    /// deserialize it as an M1 `IpcEvent`. Wrapping rather than
    /// re-modeling keeps the daemon protocol decoupled from the M1
    /// stdio protocol — bumping M1 doesn't bump daemon-protocol
    /// semver.
    SyncEvent {
        line: String,
    },
    /// Aggregated library index for the Choose Music browser. Never
    /// per-track. `scanned_at_unix_secs: None` (serialized null) = never
    /// scanned — the UI shows its scan-prompt empty state.
    LibraryUpdate {
        source_root: Option<String>,
        scanned_at_unix_secs: Option<u64>,
        artists: Vec<LibraryArtist>,
        genres: Vec<LibraryGenre>,
        total_tracks: usize,
        total_bytes: u64,
    },
    SelectionUpdate {
        mode: SelectionMode,
        rules: Vec<SelectionRule>,
    },
    /// Reply to preview_selection: hypothetical impact vs the manifest.
    /// bytes are SOURCE sizes (an estimate of on-iPod size — label it "~").
    SelectionPreview {
        selected_tracks: usize,
        selected_bytes: u64,
        adds: usize,
        removes: usize,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryAlbum {
    pub name: String,
    /// Display-only: most common genre among the album's tracks; None on
    /// tie/absence. Genre RULES always match per-track (see spec).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    pub tracks: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryArtist {
    pub name: String,
    pub albums: Vec<LibraryAlbum>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryGenre {
    pub name: String,
    pub tracks: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStateLabel {
    Idle,
    Syncing,
    Scanning,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejectReason {
    AlreadySyncing,
    NoIpod,
    NotConfigured,
    TooManyFailures,
}

/// Commands from UI → daemon.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    GetStatus,
    GetConfig,
    SaveConfig {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        daemon: Option<DaemonSettings>,
        #[serde(default)]
        ipod: Option<IpodIdentity>,
    },
    /// Clear the persisted iPod identity. Used when the user picks
    /// "Remove this iPod" from settings or the chooser. SaveConfig
    /// can't express "unset" because `ipod: None` is the wire-level
    /// "don't change" sentinel.
    ForgetIpod,
    TriggerSync {
        source: TriggerSource,
    },
    GetHistory {
        #[serde(default = "default_history_limit")]
        limit: usize,
    },
    SubscribeDeviceEvents,
    UnsubscribeDeviceEvents,
    /// Request cancellation of the currently-running sync. The daemon
    /// signals the orchestrator task, which writes a Cancel command
    /// to the subprocess stdin and force-kills after a 5s grace.
    /// History entry records outcome=Aborted with reason "user_cancelled".
    /// No-op if no sync is in progress.
    CancelSync,
    /// Gracefully pause the running sync — drains in-flight, checkpoints,
    /// → Paused. No-op if idle.
    Pause,
    /// Forward a user's reply to a `PromptEvent` from the sync
    /// subprocess. The daemon writes `{"type":"prompt_decision",
    /// "id":<id>,"choice":<choice>}` to the subprocess stdin. Without
    /// this command, daemon-relayed prompts (source-change safeguard,
    /// per-track retry/skip/abort) block the sync indefinitely
    /// because the popover UI has no other way to answer.
    /// No-op if no sync is in progress.
    DecidePrompt {
        id: u64,
        choice: i32,
    },
    /// Embed tags + cover art into the existing on-iPod library in place so
    /// Rockbox can read it. Spawns a `--backfill-rockbox` subprocess; reports
    /// sync-style progress. No-op if a sync is already running.
    BackfillRockbox,
    /// Reply: library_update from the cached index (may be never-scanned).
    GetLibrary,
    /// Spawn a --scan-library subprocess under the shared sync guard.
    /// No-op (log + drop) if busy or no source configured.
    ScanLibrary,
    GetSelection,
    SaveSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
    /// Pure computation; nothing persists. Reply: selection_preview.
    PreviewSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    Manual,
    Scheduled,
    PlugIn,
}

fn default_history_limit() -> usize { 10 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_serializes_with_protocol_version() {
        let event = DaemonEvent::Hello {
            protocol_version: DAEMON_PROTOCOL_VERSION.to_string(),
            core_version: "0.0.1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"hello""#));
        assert!(json.contains(r#""protocol_version":"1.5.0""#));
    }

    #[test]
    fn protocol_version_is_1_5_0() {
        assert_eq!(DAEMON_PROTOCOL_VERSION, "1.5.0");
    }

    #[test]
    fn new_selection_commands_deserialize() {
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"get_library"}"#).unwrap(),
            DaemonCommand::GetLibrary));
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"scan_library"}"#).unwrap(),
            DaemonCommand::ScanLibrary));
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(r#"{"type":"get_selection"}"#).unwrap(),
            DaemonCommand::GetSelection));

        let save: DaemonCommand = serde_json::from_str(
            r#"{"type":"save_selection","mode":"include","rules":[
                {"kind":"artist","name":"Boards of Canada"},
                {"kind":"album","artist":"Aphex Twin","album":"Drukqs"},
                {"kind":"genre","name":"Ambient"}]}"#).unwrap();
        match save {
            DaemonCommand::SaveSelection { mode, rules } => {
                assert_eq!(mode, crate::selection::SelectionMode::Include);
                assert_eq!(rules.len(), 3);
            }
            _ => panic!("expected SaveSelection"),
        }

        let preview: DaemonCommand = serde_json::from_str(
            r#"{"type":"preview_selection","mode":"exclude","rules":[]}"#).unwrap();
        assert!(matches!(preview, DaemonCommand::PreviewSelection { .. }));
    }

    #[test]
    fn library_update_serializes_aggregated_shape() {
        let evt = DaemonEvent::LibraryUpdate {
            source_root: Some("/music".into()),
            scanned_at_unix_secs: Some(42),
            artists: vec![LibraryArtist {
                name: "Aphex Twin".into(),
                albums: vec![LibraryAlbum {
                    name: "Drukqs".into(), genre: Some("IDM".into()), tracks: 30, bytes: 900,
                }],
            }],
            genres: vec![LibraryGenre { name: "IDM".into(), tracks: 30, bytes: 900 }],
            total_tracks: 30,
            total_bytes: 900,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"library_update""#));
        assert!(json.contains(r#""scanned_at_unix_secs":42"#));
        assert!(json.contains(r#""albums""#));
    }

    #[test]
    fn library_update_never_scanned_serializes_null_timestamp() {
        let evt = DaemonEvent::LibraryUpdate {
            source_root: None, scanned_at_unix_secs: None,
            artists: vec![], genres: vec![], total_tracks: 0, total_bytes: 0,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""scanned_at_unix_secs":null"#),
            "null (not omitted) — the UI branches on it for the never-scanned state");
    }

    #[test]
    fn selection_update_and_preview_serialize() {
        let upd = DaemonEvent::SelectionUpdate {
            mode: crate::selection::SelectionMode::Exclude,
            rules: vec![crate::selection::SelectionRule::Genre { name: "Podcast".into() }],
        };
        let json = serde_json::to_string(&upd).unwrap();
        assert!(json.contains(r#""type":"selection_update""#));
        assert!(json.contains(r#""mode":"exclude""#));

        let prev = DaemonEvent::SelectionPreview {
            selected_tracks: 2340, selected_bytes: 14_200_000_000,
            adds: 120, removes: 214,
        };
        let json = serde_json::to_string(&prev).unwrap();
        assert!(json.contains(r#""type":"selection_preview""#));
        assert!(json.contains(r#""removes":214"#));
    }

    #[test]
    fn scanning_state_label_serializes() {
        let s = serde_json::to_string(&DaemonStateLabel::Scanning).unwrap();
        assert_eq!(s, r#""scanning""#);
    }

    #[test]
    fn get_status_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"get_status"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetStatus));
    }

    #[test]
    fn decodes_pause_command() {
        let cmd: DaemonCommand = serde_json::from_str(r#"{"type":"pause"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::Pause));
    }

    #[test]
    fn save_config_with_partial_payload_deserializes() {
        let json = r#"{"type":"save_config","ipod":{"serial":"X","model_label":"iPod 7G"}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveConfig { source, daemon, ipod } => {
                assert!(source.is_none());
                assert!(daemon.is_none());
                let ipod = ipod.expect("ipod present");
                assert_eq!(ipod.serial, "X");
                assert_eq!(ipod.model_label, "iPod 7G");
            }
            _ => panic!("expected SaveConfig"),
        }
    }

    #[test]
    fn backfill_rockbox_deserializes() {
        let json = r#"{"type":"backfill_rockbox"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(json).unwrap(),
            DaemonCommand::BackfillRockbox
        ));
    }

    #[test]
    fn trigger_sync_round_trips() {
        let json = r#"{"type":"trigger_sync","source":"manual"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, DaemonCommand::TriggerSync { source: TriggerSource::Manual }));
    }

    #[test]
    fn sync_event_serializes_with_line_field() {
        let evt = DaemonEvent::SyncEvent {
            line: r#"{"type":"track_done"}"#.to_string(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"sync_event""#));
        assert!(json.contains(r#""line":"{\"type\":\"track_done\"}""#),
                "got: {json}");
    }

    #[test]
    fn device_connected_event_serializes_with_required_fields() {
        let event = DaemonEvent::DeviceConnected {
            serial: "0xABC".to_string(),
            model_label: "iPod 7G".to_string(),
            drive: "G:\\".to_string(),
            name: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"device_connected""#));
        assert!(json.contains(r#""drive":"G:\\""#));
    }
}
