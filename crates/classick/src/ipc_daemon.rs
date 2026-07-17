//! Daemon-side IPC wire types for the UI ↔ daemon channel (named pipe
//! / Unix socket). Distinct from `src/ipc.rs` (which is the daemon ↔
//! sync-subprocess channel). Same envelope shape: newline-delimited
//! JSON, snake_case "type" discriminator, additive.
//!
//! Spec §7. Protocol semver: daemon emits hello with
//! `protocol_version = "1.6.0"` since this extends M1's "1.0.0". v1.6.0
//! adds playlist CRUD (`list_playlists`/`get_playlist`/`save_playlist`/
//! `delete_playlist` + `playlists_update`), per-device config
//! (`get_device_config`/`save_device_config` + `device_config_update`),
//! and a pure device-sync-footprint estimate (`preview_device` +
//! `device_preview`) — see `docs/ipc-protocol.md` "Daemon v1.6.0".

use crate::config_file::{DaemonSettings, IpodIdentity};
use crate::daemon::device_storage::StorageInfo;
use crate::daemon::history::HistoryEntry;
use crate::selection::{SelectionMode, SelectionRule};
use serde::{Deserialize, Serialize};

pub const DAEMON_PROTOCOL_VERSION: &str = "1.6.0";

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
    /// Reply to `list_playlists`/`get_playlist`, and broadcast to all
    /// clients after `save_playlist`/`delete_playlist`. Every playlist in
    /// the store, summarized against the cached library index — sorted by
    /// `slug` for deterministic wire ordering. `get_playlist` reuses this
    /// same event with `playlists` filtered to 0 or 1 entries rather than
    /// introducing a separate single-playlist event type.
    PlaylistsUpdate {
        playlists: Vec<PlaylistSummary>,
    },
    /// Reply to `get_playlist`: the one playlist's full content — unlike
    /// `PlaylistsUpdate`'s per-list summary (track count only), this is
    /// what the playlist editor needs to actually render/edit a playlist.
    /// `name`/`kind` are `Some` together with the matching content field
    /// (`tracks` for manual, `rules` for smart) on success; on failure
    /// (unopenable store, no playlist at `slug`, or an on-disk file that
    /// failed to parse) `name`/`kind`/`tracks`/`rules` are all `None` and
    /// `error` is set instead — so the requester can tell "not found" from
    /// "found, empty" (e.g. a manual playlist with zero tracks).
    PlaylistDetail {
        slug: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<PlaylistKind>,
        /// Source-relative track paths, in order. `Some` only when
        /// `kind` is `manual`.
        #[serde(skip_serializing_if = "Option::is_none")]
        tracks: Option<Vec<String>>,
        /// `Some` only when `kind` is `smart` — exactly as
        /// `playlist_rules::SmartRules` serializes.
        #[serde(skip_serializing_if = "Option::is_none")]
        rules: Option<crate::playlist_rules::SmartRules>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to `get_device_config`, and broadcast to all clients after
    /// `save_device_config`. One device's resolved selection +
    /// subscriptions + settings.
    DeviceConfigUpdate {
        serial: String,
        selection: SelectionPayload,
        subscriptions: SubscriptionsPayload,
        settings: DeviceSettingsPayload,
    },
    /// Reply to `preview_device`: a pure, index-based estimate of what this
    /// device's sync would look like — no filesystem walk. `selected_*` is
    /// the scope selection; `playlist_extra_*` is subscribed-playlist
    /// members NOT already in scope (the union's out-of-scope delta, same
    /// idea as `sync_set::compute`'s union but sized from the cached index
    /// rather than a live walk). `projected_free_bytes` is `None` whenever
    /// this device isn't the one currently connected (no live `StorageInfo`
    /// to project from) — see `daemon::library::compute_device_preview`
    /// for exactly what "projected" assumes.
    DevicePreview {
        selected_tracks: usize,
        selected_bytes: u64,
        playlist_extra_tracks: usize,
        playlist_extra_bytes: u64,
        projected_free_bytes: Option<u64>,
        /// Slugs from this device's subscriptions that `compute_device_preview`
        /// could not resolve against the cached index (unknown slug, or a
        /// playlist-store load error) — the same set that's silently folded
        /// into "no extra" in the `playlist_extra_*` totals above, surfaced so
        /// the UI can flag a dangling subscription instead of just under-
        /// counting bytes. Sorted for deterministic wire ordering. Omitted
        /// entirely (not `[]`) when every subscription resolved.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        unresolved_subscriptions: Vec<String>,
    },
}

/// `manual` or `smart`, mirroring `Playlist`'s two variants on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistKind {
    Manual,
    Smart,
}

/// One entry on `playlists_update`. `tracks`/`bytes` are computed against
/// the cached library index (never a filesystem walk) — see
/// `daemon::library::build_playlist_summaries`. `error` is set (and
/// `tracks`/`bytes` are `0`) for a playlist FILE the store failed to parse;
/// it still surfaces here (named from its filename) rather than silently
/// vanishing from the list, so a corrupt playlist is something the user can
/// see and delete.
#[derive(Debug, Clone, Serialize)]
pub struct PlaylistSummary {
    pub slug: String,
    pub name: String,
    pub kind: PlaylistKind,
    pub tracks: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Wire shape of a selection nested under `save_device_config` /
/// `device_config_update` — `mode` + `rules` only. Deliberately distinct
/// from the on-disk `selection::Selection` type: that type's `version`
/// field is a file-format implementation detail, not part of the wire
/// contract (mirrors how `selection_update`/`save_selection` already
/// flatten to just `mode`+`rules`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionPayload {
    #[serde(default)]
    pub mode: SelectionMode,
    #[serde(default)]
    pub rules: Vec<SelectionRule>,
}

/// Wire shape of subscriptions nested under `save_device_config` /
/// `device_config_update` — the subscribed playlist slugs only (no
/// `version`, same rationale as `SelectionPayload`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionsPayload {
    #[serde(default)]
    pub playlists: Vec<String>,
}

/// Wire shape of per-device settings nested under `save_device_config` /
/// `device_config_update` (no `version`, same rationale as
/// `SelectionPayload`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSettingsPayload {
    #[serde(default = "default_true")]
    pub auto_sync: bool,
    #[serde(default)]
    pub rockbox_compat: bool,
}

/// `save_playlist` command payload, tagged by `kind`. An absent `slug` on
/// either variant means "create a new playlist": the runtime allocates one
/// via `PlaylistStore::unique_slug(name)`. A present `slug` means
/// "create-or-replace at exactly this slug" (the edit path).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaylistPayload {
    Manual {
        #[serde(default)]
        slug: Option<String>,
        name: String,
        /// Source-relative track paths, in order. Not validated here —
        /// `playlist::resolve_manual` is the last line of defense against
        /// an unsafe (absolute or `..`-escaping) entry at resolve time, by
        /// design (see its doc comment); this payload can carry one
        /// through to disk without failing the save.
        #[serde(default)]
        tracks: Vec<String>,
    },
    Smart {
        #[serde(default)]
        slug: Option<String>,
        name: String,
        rules: crate::playlist_rules::SmartRules,
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
    /// Erase every track on the iPod, then sync the current selection from
    /// scratch. Spawns a `--replace-library --apply` subprocess; reports
    /// sync-style progress. `--apply` skips the core's interactive
    /// confirmation prompt — the UI does its own typed confirmation before
    /// ever sending this command. No-op if a sync is already running.
    ReplaceLibrary,
    /// Reply: library_update from the cached index (may be never-scanned).
    GetLibrary,
    /// Spawn a --scan-library subprocess under the shared sync guard.
    /// No-op (log + drop) if busy or no source configured.
    ScanLibrary,
    /// **Deprecated as of v1.6.0**: replies `selection_update` for the
    /// *configured* device's own per-device selection (resolved via
    /// `selection::effective_device_selection_path`, seeded from the
    /// shared `selection.json` the first time it's read) — no longer the
    /// `custom_selection`-gated shared/per-device split
    /// `selection::effective_selection_path` implements. Kept for older UI
    /// clients that don't yet target a device explicitly; prefer
    /// `get_device_config` with an explicit `serial`. If no device is
    /// configured, replies with `mode: "all"`.
    GetSelection,
    /// **Deprecated as of v1.6.0**: same per-device target as
    /// `GetSelection`, and same deprecation rationale — prefer
    /// `save_device_config` with an explicit `serial`. No-op (log + drop,
    /// no reply) if no device is currently configured — unlike the old
    /// shared-file fallback, there's no per-device path to resolve without
    /// a serial.
    SaveSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
    /// Pure computation; nothing persists. Reply: selection_preview.
    PreviewSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
    },
    /// Reply: `playlists_update`, every playlist in the store. Never fails
    /// the arm: a playlist file the store can't parse surfaces as an
    /// `error`-annotated stub entry (see `PlaylistSummary`) rather than
    /// aborting the reply, and a store-open failure (e.g. an unwritable
    /// config dir) degrades to an empty `playlists` list (logged).
    ListPlaylists,
    /// Reply: `playlist_detail` for the one matching `slug` — full content
    /// (the manual track list or the smart rule set), for the playlist
    /// editor. Unlike `ListPlaylists`'s fail-open posture, a missing slug,
    /// an unopenable store, or an on-disk file that fails to parse all
    /// reply with `error` set (see `PlaylistDetail`) rather than degrading
    /// to an empty result — the requester needs to distinguish "not found"
    /// from "found, empty".
    GetPlaylist {
        slug: String,
    },
    /// Create (absent `playlist.slug`) or replace (present) a playlist.
    /// Persists atomically; broadcasts a fresh `playlists_update` to every
    /// client on success. No direct reply — on a store-open or write
    /// failure (including "couldn't allocate a slug"), the arm logs and
    /// returns without persisting or broadcasting; the client learns
    /// nothing changed only by the absence of a `playlists_update`.
    SavePlaylist {
        playlist: PlaylistPayload,
    },
    /// Delete a playlist by slug. No-op (still broadcasts) if the slug
    /// doesn't exist. Broadcasts a fresh `playlists_update` to every
    /// client even on a delete failure (logged) — the broadcast reflects
    /// whatever's actually on disk, whether or not this delete succeeded.
    /// No direct reply.
    DeletePlaylist {
        slug: String,
    },
    /// Reply: `device_config_update` for the given device's serial — its
    /// resolved selection, subscriptions, and settings. Never fails the
    /// arm: an unresolvable path or unreadable file for any one part
    /// degrades to that part's default (see `selection::load_or_all`,
    /// `Subscriptions::load_or_default`, `DeviceSettings::load_or_migrate`)
    /// rather than dropping the reply — even a `serial` the daemon has
    /// never seen before gets a well-formed all-defaults reply.
    GetDeviceConfig {
        serial: String,
    },
    /// Persist the provided parts (each field `None` = "don't change",
    /// same sentinel convention as `save_config`) for the given device's
    /// serial, then broadcast a fresh `device_config_update` to every
    /// client. No direct reply. Each part is saved independently — a
    /// failure to persist one part (logged) doesn't block the others, and
    /// the closing broadcast always fires reflecting whatever did persist.
    /// If `serial` is the currently configured device, also broadcasts a
    /// refreshed `status_update` (the selection change may move "Y" in
    /// "X of Y synced").
    SaveDeviceConfig {
        serial: String,
        #[serde(default)]
        selection: Option<SelectionPayload>,
        #[serde(default)]
        subscriptions: Option<SubscriptionsPayload>,
        #[serde(default)]
        settings: Option<DeviceSettingsPayload>,
    },
    /// Pure computation over the cached library index + this device's
    /// config — no filesystem walk, nothing persists. Reply:
    /// `device_preview`. Never fails the arm: a playlist-store-open
    /// failure degrades to "no playlist extras" (logged), same fail-open
    /// posture as `GetDeviceConfig`.
    PreviewDevice {
        serial: String,
    },
    Shutdown,
}

fn default_true() -> bool {
    true
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
        assert!(json.contains(r#""protocol_version":"1.6.0""#));
    }

    #[test]
    fn protocol_version_is_1_6_0() {
        assert_eq!(DAEMON_PROTOCOL_VERSION, "1.6.0");
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
    fn replace_library_deserializes() {
        let json = r#"{"type":"replace_library"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(json).unwrap(),
            DaemonCommand::ReplaceLibrary
        ));
    }

    /// `replace_library`'s busy/no-device guards (runtime.rs) reply with
    /// these two `SyncRejected` variants — unlike `TriggerSync`'s NotConfigured
    /// path, `replace_library` never sends that third reason. Locks the wire
    /// shape those replies depend on.
    #[test]
    fn sync_rejected_serializes_already_syncing_and_no_ipod() {
        let already_syncing = DaemonEvent::SyncRejected { reason: SyncRejectReason::AlreadySyncing };
        let json = serde_json::to_string(&already_syncing).unwrap();
        assert!(json.contains(r#""type":"sync_rejected""#));
        assert!(json.contains(r#""reason":"already_syncing""#), "got: {json}");

        let no_ipod = DaemonEvent::SyncRejected { reason: SyncRejectReason::NoIpod };
        let json = serde_json::to_string(&no_ipod).unwrap();
        assert!(json.contains(r#""reason":"no_ipod""#), "got: {json}");
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

    // --- v1.6.0: playlist CRUD -------------------------------------

    #[test]
    fn list_playlists_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"list_playlists"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::ListPlaylists));
    }

    #[test]
    fn get_playlist_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"get_playlist","slug":"gym"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetPlaylist { slug } if slug == "gym"));
    }

    #[test]
    fn delete_playlist_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"delete_playlist","slug":"gym"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::DeletePlaylist { slug } if slug == "gym"));
    }

    #[test]
    fn playlist_detail_manual_serializes_with_tracks_and_no_rules() {
        let evt = DaemonEvent::PlaylistDetail {
            slug: "gym".into(),
            name: Some("Gym".into()),
            kind: Some(PlaylistKind::Manual),
            tracks: Some(vec!["Artist/Album/01.flac".into(), "B/02.flac".into()]),
            rules: None,
            error: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":["Artist/Album/01.flac","B/02.flac"]}"#,
            "got: {json}"
        );
    }

    #[test]
    fn playlist_detail_smart_serializes_with_rules_and_no_tracks() {
        let evt = DaemonEvent::PlaylistDetail {
            slug: "recent-idm".into(),
            name: Some("Recent IDM".into()),
            kind: Some(PlaylistKind::Smart),
            tracks: None,
            rules: Some(crate::playlist_rules::SmartRules {
                version: 1,
                matching: crate::playlist_rules::Match::All,
                rules: vec![crate::playlist_rules::Rule {
                    field: crate::playlist_rules::Field::Genre,
                    op: crate::playlist_rules::Op::Is,
                    value: "IDM".into(),
                }],
                limit: None,
                order: crate::playlist_rules::Order::Alpha,
                seed: 0,
            }),
            error: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"recent-idm","name":"Recent IDM","kind":"smart","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0}}"#,
            "got: {json}"
        );
    }

    #[test]
    fn playlist_detail_error_omits_name_kind_tracks_and_rules() {
        let evt = DaemonEvent::PlaylistDetail {
            slug: "ghost".into(),
            name: None,
            kind: None,
            tracks: None,
            rules: None,
            error: Some("no such playlist".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"ghost","error":"no such playlist"}"#,
            "got: {json}"
        );
    }

    #[test]
    fn save_playlist_manual_without_slug_deserializes_as_create() {
        let json = r#"{"type":"save_playlist","playlist":
            {"kind":"manual","name":"Gym","tracks":["Artist/Album/01.flac","B/02.flac"]}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist { playlist: PlaylistPayload::Manual { slug, name, tracks } } => {
                assert_eq!(slug, None, "absent slug means create");
                assert_eq!(name, "Gym");
                assert_eq!(tracks, vec!["Artist/Album/01.flac".to_string(), "B/02.flac".to_string()]);
            }
            _ => panic!("expected SavePlaylist(Manual)"),
        }
    }

    #[test]
    fn save_playlist_manual_with_slug_deserializes_as_edit() {
        let json = r#"{"type":"save_playlist","playlist":
            {"kind":"manual","slug":"gym","name":"Gym Mix","tracks":[]}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist { playlist: PlaylistPayload::Manual { slug, name, tracks } } => {
                assert_eq!(slug.as_deref(), Some("gym"));
                assert_eq!(name, "Gym Mix");
                assert!(tracks.is_empty());
            }
            _ => panic!("expected SavePlaylist(Manual)"),
        }
    }

    #[test]
    fn save_playlist_smart_deserializes() {
        let json = r#"{"type":"save_playlist","playlist":
            {"kind":"smart","name":"Recent IDM","rules":
                {"version":1,"matching":"all","rules":[
                    {"field":"genre","op":"is","value":"IDM"}]}}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist { playlist: PlaylistPayload::Smart { slug, name, rules } } => {
                assert_eq!(slug, None);
                assert_eq!(name, "Recent IDM");
                assert_eq!(rules.rules.len(), 1);
            }
            _ => panic!("expected SavePlaylist(Smart)"),
        }
    }

    #[test]
    fn playlists_update_serializes_sorted_summaries_with_optional_error() {
        let evt = DaemonEvent::PlaylistsUpdate {
            playlists: vec![
                PlaylistSummary {
                    slug: "gym".into(), name: "Gym".into(), kind: PlaylistKind::Manual,
                    tracks: 12, bytes: 900, error: None,
                },
                PlaylistSummary {
                    slug: "broken".into(), name: "broken".into(), kind: PlaylistKind::Smart,
                    tracks: 0, bytes: 0, error: Some("parse failed".into()),
                },
            ],
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"playlists_update""#));
        assert!(json.contains(r#""kind":"manual""#));
        assert!(json.contains(r#""kind":"smart""#));
        assert!(json.contains(r#""error":"parse failed""#));
        assert!(!json.contains(r#""slug":"gym","name":"Gym","kind":"manual","tracks":12,"bytes":900,"error""#),
            "error must be omitted (not null) when absent");
    }

    // --- v1.6.0: per-device config -----------------------------------

    #[test]
    fn get_device_config_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"get_device_config","serial":"0xABC"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetDeviceConfig { serial } if serial == "0xABC"));
    }

    #[test]
    fn save_device_config_partial_payload_deserializes() {
        // Only `settings` provided — `selection`/`subscriptions` are the
        // wire-level "don't change" sentinel, same convention as `save_config`.
        let json = r#"{"type":"save_device_config","serial":"0xABC",
            "settings":{"auto_sync":false,"rockbox_compat":true}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveDeviceConfig { serial, selection, subscriptions, settings } => {
                assert_eq!(serial, "0xABC");
                assert!(selection.is_none());
                assert!(subscriptions.is_none());
                let settings = settings.expect("settings present");
                assert!(!settings.auto_sync);
                assert!(settings.rockbox_compat);
            }
            _ => panic!("expected SaveDeviceConfig"),
        }
    }

    #[test]
    fn save_device_config_full_payload_deserializes() {
        let json = r#"{"type":"save_device_config","serial":"0xABC",
            "selection":{"mode":"include","rules":[{"kind":"artist","name":"Boards of Canada"}]},
            "subscriptions":{"playlists":["gym","chill"]},
            "settings":{"auto_sync":true,"rockbox_compat":false}}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveDeviceConfig { serial, selection, subscriptions, settings } => {
                assert_eq!(serial, "0xABC");
                let selection = selection.expect("selection present");
                assert_eq!(selection.mode, crate::selection::SelectionMode::Include);
                assert_eq!(selection.rules.len(), 1);
                assert_eq!(subscriptions.expect("subscriptions present").playlists, vec!["gym", "chill"]);
                assert!(settings.expect("settings present").auto_sync);
            }
            _ => panic!("expected SaveDeviceConfig"),
        }
    }

    #[test]
    fn device_config_update_serializes() {
        let evt = DaemonEvent::DeviceConfigUpdate {
            serial: "0xABC".into(),
            selection: SelectionPayload {
                mode: crate::selection::SelectionMode::Include,
                rules: vec![],
            },
            subscriptions: SubscriptionsPayload { playlists: vec!["gym".into()] },
            settings: DeviceSettingsPayload { auto_sync: true, rockbox_compat: false },
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"device_config_update""#));
        assert!(json.contains(r#""serial":"0xABC""#));
        assert!(json.contains(r#""mode":"include""#));
        assert!(json.contains(r#""playlists":["gym"]"#));
        assert!(json.contains(r#""auto_sync":true"#));
    }

    // --- v1.6.0: device preview ---------------------------------------

    #[test]
    fn preview_device_deserializes() {
        let cmd: DaemonCommand =
            serde_json::from_str(r#"{"type":"preview_device","serial":"0xABC"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::PreviewDevice { serial } if serial == "0xABC"));
    }

    #[test]
    fn device_preview_serializes_projected_free_as_value_or_null() {
        let connected = DaemonEvent::DevicePreview {
            selected_tracks: 100, selected_bytes: 5_000_000_000,
            playlist_extra_tracks: 3, playlist_extra_bytes: 90_000_000,
            projected_free_bytes: Some(1_200_000_000),
            unresolved_subscriptions: vec![],
        };
        let json = serde_json::to_string(&connected).unwrap();
        assert!(json.contains(r#""type":"device_preview""#));
        assert!(json.contains(r#""projected_free_bytes":1200000000"#));

        let disconnected = DaemonEvent::DevicePreview {
            selected_tracks: 100, selected_bytes: 5_000_000_000,
            playlist_extra_tracks: 3, playlist_extra_bytes: 90_000_000,
            projected_free_bytes: None,
            unresolved_subscriptions: vec![],
        };
        let json = serde_json::to_string(&disconnected).unwrap();
        assert!(json.contains(r#""projected_free_bytes":null"#),
            "null (not omitted) — mirrors library_update's never-scanned convention");
    }

    #[test]
    fn device_preview_unresolved_subscriptions_present_or_omitted() {
        let with_unresolved = DaemonEvent::DevicePreview {
            selected_tracks: 1, selected_bytes: 1,
            playlist_extra_tracks: 0, playlist_extra_bytes: 0,
            projected_free_bytes: None,
            unresolved_subscriptions: vec!["ghost".into(), "gym".into()],
        };
        let json = serde_json::to_string(&with_unresolved).unwrap();
        assert!(json.contains(r#""unresolved_subscriptions":["ghost","gym"]"#));

        let none_unresolved = DaemonEvent::DevicePreview {
            selected_tracks: 1, selected_bytes: 1,
            playlist_extra_tracks: 0, playlist_extra_bytes: 0,
            projected_free_bytes: None,
            unresolved_subscriptions: vec![],
        };
        let json = serde_json::to_string(&none_unresolved).unwrap();
        assert!(!json.contains("unresolved_subscriptions"),
            "omitted entirely when empty, not serialized as []");
    }
}
