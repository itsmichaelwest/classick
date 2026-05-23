//! Daemon-side IPC wire types for the UI ↔ daemon channel (named pipe
//! / Unix socket). Distinct from `src/ipc.rs` (which is the daemon ↔
//! sync-subprocess channel). Same envelope shape: newline-delimited
//! JSON, snake_case "type" discriminator, additive.
//!
//! Spec §7. Protocol semver: daemon emits hello with
//! `protocol_version = "1.1.0"` since this extends M1's "1.0.0".

use crate::config_file::{DaemonSettings, IpodIdentity};
use crate::daemon::history::HistoryEntry;
use serde::{Deserialize, Serialize};

pub const DAEMON_PROTOCOL_VERSION: &str = "1.1.0";

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
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStateLabel {
    Idle,
    Syncing,
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
        assert!(json.contains(r#""protocol_version":"1.1.0""#));
    }

    #[test]
    fn get_status_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"get_status"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetStatus));
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
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"device_connected""#));
        assert!(json.contains(r#""drive":"G:\\""#));
    }
}
