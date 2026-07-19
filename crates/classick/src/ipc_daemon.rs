//! Daemon-side IPC wire types for the UI ↔ daemon channel (named pipe
//! / Unix socket). Distinct from `src/ipc.rs` (which is the daemon ↔
//! sync-subprocess channel). Same envelope shape: newline-delimited
//! JSON with a snake_case `type` discriminator.
//!
//! Spec §7. Protocol semver: daemon emits hello with
//! `protocol_version = "2.0.0"`. Version 2 makes command correlation and
//! device targeting explicit, adds serial-keyed inventory snapshots, and
//! removes the deprecated singleton selection commands. See
//! `docs/ipc-protocol.md` "Daemon v2.0.0".

use crate::config_file::{DaemonSettings, IpodIdentity};
use crate::daemon::device_storage::StorageInfo;
use crate::daemon::history::HistoryEntry;
use crate::ipc_device::DeviceInventorySnapshot;
use crate::selection::{SelectionMode, SelectionRule};
use serde::{Deserialize, Serialize};

pub const DAEMON_PROTOCOL_VERSION: &str = "2.0.0";

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
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
    },
    ConfigUpdate {
        source: Option<String>,
        daemon: Option<DaemonSettings>,
        ipod: Option<IpodIdentity>,
        config_revision: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
    },
    HistoryUpdate {
        entries: Vec<HistoryEntry>,
        acknowledged_request_id: String,
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
        serial: String,
        acknowledged_request_id: String,
    },
    /// Correlated failure for a command that has no existing error-shaped
    /// canonical reply. It intentionally carries no revision: clients retain
    /// durable intents until a later canonical acknowledgement proves the
    /// requested state was persisted.
    CommandFailed {
        acknowledged_request_id: String,
        error: String,
    },
    /// Forwarded sync-subprocess event. `line` is the raw JSON line
    /// the subprocess emitted on its stdout, unparsed. UI clients
    /// deserialize it as an M1 `IpcEvent`. Wrapping rather than
    /// re-modeling keeps the daemon protocol decoupled from the M1
    /// stdio protocol — bumping M1 doesn't bump daemon-protocol
    /// semver.
    SyncEvent {
        line: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        serial: Option<String>,
        session_id: crate::ipc_device::SessionId,
    },
    #[serde(rename = "device_inventory_snapshot")]
    DeviceInventorySnapshot(DeviceInventorySnapshot),
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
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
    },
    SelectionUpdate {
        mode: SelectionMode,
        rules: Vec<SelectionRule>,
        #[serde(skip_serializing_if = "Option::is_none")]
        serial: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
    },
    /// Reply to preview_selection: hypothetical impact vs the manifest.
    /// bytes are SOURCE sizes (an estimate of on-iPod size — label it "~").
    SelectionPreview {
        selected_tracks: usize,
        selected_bytes: u64,
        adds: usize,
        removes: usize,
        serial: String,
        acknowledged_request_id: String,
    },
    /// Reply to `list_playlists`, and broadcast to all clients after
    /// `save_playlist`/`delete_playlist`. Every playlist in
    /// the store, summarized against the cached library index — sorted by
    /// `slug` for deterministic wire ordering.
    PlaylistsUpdate {
        playlists: Vec<PlaylistSummary>,
        playlist_revision: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
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
        playlist_revision: u64,
        acknowledged_request_id: String,
    },
    /// Reply to `get_device_config`, and broadcast to all clients after
    /// `save_device_config`. One device's resolved selection +
    /// subscriptions + settings.
    DeviceConfigUpdate {
        serial: String,
        selection: SelectionPayload,
        subscriptions: SubscriptionsPayload,
        settings: DeviceSettingsPayload,
        selection_revision: u64,
        settings_revision: u64,
        subscriptions_revision: u64,
        acknowledged_request_id: String,
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
        serial: String,
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
        acknowledged_request_id: String,
    },
    /// Reply to `resolve_tracks` (v1.7.0): concrete source-relative track
    /// paths matched by the given selection rules, expanded against the
    /// cached library index — see `daemon::library::resolve_tracks`. Rules
    /// that match nothing contribute nothing; an absent/never-scanned index
    /// yields an empty list. Both are ordinary replies (`tracks: []`),
    /// never an error. Sorted lexicographically for deterministic ordering.
    ResolvedTracks {
        tracks: Vec<String>,
        acknowledged_request_id: String,
    },
    /// Current reachability of the configured source library. `source_root`
    /// is present only for `available`; mount failures intentionally carry no
    /// backend diagnostics so share paths and credentials cannot leak.
    SourceAvailability {
        state: SourceAvailabilityState,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_root: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        acknowledged_request_id: Option<String>,
    },
}

impl DaemonEvent {
    pub fn with_acknowledged_request_id(mut self, request_id: Option<String>) -> Self {
        match &mut self {
            Self::StatusUpdate {
                acknowledged_request_id,
                ..
            }
            | Self::ConfigUpdate {
                acknowledged_request_id,
                ..
            }
            | Self::LibraryUpdate {
                acknowledged_request_id,
                ..
            }
            | Self::SelectionUpdate {
                acknowledged_request_id,
                ..
            }
            | Self::PlaylistsUpdate {
                acknowledged_request_id,
                ..
            } => {
                *acknowledged_request_id = request_id;
            }
            _ => {}
        }
        self
    }

    pub fn with_target_serial(mut self, target_serial: Option<String>) -> Self {
        match &mut self {
            Self::SelectionUpdate { serial, .. } => *serial = target_serial,
            _ => {}
        }
        self
    }
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
/// contract (mirrors how `device_config_update` flattens to just
/// `mode`+`rules`).
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStateLabel {
    Idle,
    Syncing,
    Scanning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailabilityState {
    Available,
    Remounting,
    AuthRequired,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    GetStatus {
        request_id: String,
    },
    GetConfig {
        request_id: String,
    },
    SaveConfig {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        daemon: Option<DaemonSettings>,
        #[serde(default)]
        ipod: Option<IpodIdentity>,
        request_id: String,
    },
    /// Clear the persisted iPod identity. Used when the user picks
    /// "Remove this iPod" from settings or the chooser. SaveConfig
    /// can't express "unset" because `ipod: None` is the wire-level
    /// "don't change" sentinel.
    ForgetIpod {
        serial: String,
        request_id: String,
    },
    TriggerSync {
        source: TriggerSource,
        serial: String,
        request_id: String,
    },
    GetHistory {
        #[serde(default = "default_history_limit")]
        limit: usize,
        request_id: String,
    },
    SubscribeDeviceEvents,
    UnsubscribeDeviceEvents,
    /// Request cancellation of the currently-running sync. The daemon
    /// signals the orchestrator task, which writes one Cancel command and
    /// drains finalization progress through `cancelled` and stdout EOF.
    /// History records the distinct `cancelled` outcome. A 120-second
    /// inactivity grace is the emergency kill backstop.
    /// No-op if no sync is in progress.
    CancelSync {
        serial: String,
        request_id: String,
    },
    /// Gracefully pause the running sync — drains in-flight, checkpoints,
    /// → Paused. No-op if idle.
    Pause {
        serial: String,
        request_id: String,
    },
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
        serial: String,
        request_id: String,
    },
    /// Embed tags + cover art into the existing on-iPod library in place so
    /// Rockbox can read it. Spawns a `--backfill-rockbox` subprocess; reports
    /// sync-style progress. No-op if a sync is already running.
    BackfillRockbox {
        serial: String,
        request_id: String,
    },
    /// Erase every track on the iPod, then sync the current selection from
    /// scratch. Spawns a `--replace-library --apply` subprocess; reports
    /// sync-style progress. `--apply` skips the core's interactive
    /// confirmation prompt — the UI does its own typed confirmation before
    /// ever sending this command. No-op if a sync is already running.
    ReplaceLibrary {
        serial: String,
        request_id: String,
    },
    /// Reply: library_update from the cached index (may be never-scanned).
    GetLibrary {
        request_id: String,
    },
    /// Spawn a --scan-library subprocess under the shared sync guard.
    /// No-op (log + drop) if busy or no source configured.
    ScanLibrary {
        request_id: String,
    },
    /// Retry the global source-library mount. `allow_ui` is required so the
    /// daemon never infers permission to open authentication UI.
    RetrySourceMount {
        allow_ui: bool,
        request_id: String,
    },
    /// Pure computation; nothing persists. Reply: selection_preview.
    PreviewSelection {
        mode: crate::selection::SelectionMode,
        rules: Vec<crate::selection::SelectionRule>,
        serial: String,
        request_id: String,
    },
    /// Reply: `playlists_update`, every playlist in the store. Never fails
    /// the arm: a playlist file the store can't parse surfaces as an
    /// `error`-annotated stub entry (see `PlaylistSummary`) rather than
    /// aborting the reply, and a store-open failure (e.g. an unwritable
    /// config dir) degrades to an empty `playlists` list (logged).
    ListPlaylists {
        request_id: String,
    },
    /// Reply: `playlist_detail` for the one matching `slug` — full content
    /// (the manual track list or the smart rule set), for the playlist
    /// editor. Unlike `ListPlaylists`'s fail-open posture, a missing slug,
    /// an unopenable store, or an on-disk file that fails to parse all
    /// reply with `error` set (see `PlaylistDetail`) rather than degrading
    /// to an empty result — the requester needs to distinguish "not found"
    /// from "found, empty".
    GetPlaylist {
        slug: String,
        request_id: String,
    },
    /// Create (absent `playlist.slug`) or replace (present) a playlist.
    /// Persists atomically; broadcasts a fresh `playlists_update` to every
    /// client on success. No direct reply — on a store-open or write
    /// failure (including "couldn't allocate a slug"), the arm logs and
    /// returns without persisting or broadcasting; the client learns
    /// nothing changed only by the absence of a `playlists_update`.
    SavePlaylist {
        playlist: PlaylistPayload,
        request_id: String,
    },
    /// Delete a playlist by slug and remove that slug from every remembered
    /// device's subscriptions in one recoverable host transaction. A missing
    /// playlist is an acknowledged no-op. Successful deletion broadcasts the
    /// changed device configs followed by `playlists_update`; failures log
    /// without a success broadcast.
    DeletePlaylist {
        slug: String,
        request_id: String,
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
        request_id: String,
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
        request_id: String,
    },
    /// Pure computation over the cached library index + this device's
    /// config — no filesystem walk, nothing persists. Reply:
    /// `device_preview`. Never fails the arm: a playlist-store-open
    /// failure degrades to "no playlist extras" (logged), same fail-open
    /// posture as `GetDeviceConfig`.
    PreviewDevice {
        serial: String,
        request_id: String,
    },
    /// v1.7.0: expand artist/album/genre selection rules — the SAME `rules`
    /// wire shape `save_device_config`'s `selection.rules` uses
    /// (`selection::SelectionRule`) — into concrete source-relative track
    /// paths against the cached library index. Exists for clients (e.g. the
    /// macOS "Add Songs" picker) that only see aggregate library data
    /// (artist/album/genre + counts) and can't otherwise turn a rule-based
    /// selection into concrete playlist track entries. Pure computation;
    /// nothing persists. Reply: `resolved_tracks`, sent synchronously
    /// inline (same reply-ordering contract as `preview_device`) rather
    /// than after any async work, so replies arrive in request order.
    /// Never fails the arm: an absent/never-scanned index degrades to an
    /// empty `tracks` list rather than an error.
    ResolveTracks {
        rules: Vec<crate::selection::SelectionRule>,
        request_id: String,
    },
    Shutdown,
}

impl DaemonCommand {
    pub fn target_serial(&self) -> Option<&str> {
        match self {
            Self::ForgetIpod { serial, .. }
            | Self::TriggerSync { serial, .. }
            | Self::CancelSync { serial, .. }
            | Self::Pause { serial, .. }
            | Self::DecidePrompt { serial, .. }
            | Self::BackfillRockbox { serial, .. }
            | Self::ReplaceLibrary { serial, .. }
            | Self::PreviewSelection { serial, .. }
            | Self::GetDeviceConfig { serial, .. }
            | Self::SaveDeviceConfig { serial, .. }
            | Self::PreviewDevice { serial, .. } => Some(serial),
            _ => None,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::GetStatus { request_id }
            | Self::GetConfig { request_id }
            | Self::SaveConfig { request_id, .. }
            | Self::ForgetIpod { request_id, .. }
            | Self::TriggerSync { request_id, .. }
            | Self::GetHistory { request_id, .. }
            | Self::CancelSync { request_id, .. }
            | Self::Pause { request_id, .. }
            | Self::DecidePrompt { request_id, .. }
            | Self::BackfillRockbox { request_id, .. }
            | Self::ReplaceLibrary { request_id, .. }
            | Self::GetLibrary { request_id }
            | Self::ScanLibrary { request_id }
            | Self::RetrySourceMount { request_id, .. }
            | Self::PreviewSelection { request_id, .. }
            | Self::ListPlaylists { request_id }
            | Self::GetPlaylist { request_id, .. }
            | Self::SavePlaylist { request_id, .. }
            | Self::DeletePlaylist { request_id, .. }
            | Self::GetDeviceConfig { request_id, .. }
            | Self::SaveDeviceConfig { request_id, .. }
            | Self::PreviewDevice { request_id, .. }
            | Self::ResolveTracks { request_id, .. } => Some(request_id.as_str()),
            Self::SubscribeDeviceEvents | Self::UnsubscribeDeviceEvents | Self::Shutdown => None,
        }
    }
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

fn default_history_limit() -> usize {
    10
}

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
        assert!(json.contains(r#""protocol_version":"2.0.0""#));
    }

    #[test]
    fn protocol_version_is_2_0_0_constant() {
        assert_eq!(DAEMON_PROTOCOL_VERSION, "2.0.0");
    }

    #[test]
    fn source_availability_available_serializes_with_correlated_root() {
        let event = DaemonEvent::SourceAvailability {
            state: SourceAvailabilityState::Available,
            source_root: Some("/Volumes/data-1/media/music".into()),
            acknowledged_request_id: Some("req-mount".into()),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "source_availability",
                "state": "available",
                "source_root": "/Volumes/data-1/media/music",
                "acknowledged_request_id": "req-mount"
            })
        );
    }

    #[test]
    fn source_availability_non_available_states_omit_root_and_ack() {
        let event = DaemonEvent::SourceAvailability {
            state: SourceAvailabilityState::AuthRequired,
            source_root: None,
            acknowledged_request_id: None,
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "source_availability",
                "state": "auth_required"
            })
        );
    }

    #[test]
    fn retry_source_mount_requires_allow_ui_and_request_id() {
        let command: DaemonCommand = serde_json::from_value(serde_json::json!({
            "type": "retry_source_mount",
            "allow_ui": true,
            "request_id": "req-mount"
        }))
        .unwrap();

        assert!(matches!(
            command,
            DaemonCommand::RetrySourceMount {
                allow_ui: true,
                request_id
            } if request_id == "req-mount"
        ));
        assert!(serde_json::from_value::<DaemonCommand>(serde_json::json!({
            "type": "retry_source_mount",
            "request_id": "req-mount"
        }))
        .is_err());
        assert!(serde_json::from_value::<DaemonCommand>(serde_json::json!({
            "type": "retry_source_mount",
            "allow_ui": true
        }))
        .is_err());
    }

    #[test]
    fn v2_library_and_preview_commands_require_correlation() {
        assert!(serde_json::from_str::<DaemonCommand>(r#"{"type":"get_library"}"#).is_err());
        assert!(serde_json::from_str::<DaemonCommand>(r#"{"type":"scan_library"}"#).is_err());
        let preview: DaemonCommand = serde_json::from_str(
            r#"{"type":"preview_selection","mode":"exclude","rules":[],"serial":"A","request_id":"req"}"#).unwrap();
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
                    name: "Drukqs".into(),
                    genre: Some("IDM".into()),
                    tracks: 30,
                    bytes: 900,
                }],
            }],
            genres: vec![LibraryGenre {
                name: "IDM".into(),
                tracks: 30,
                bytes: 900,
            }],
            total_tracks: 30,
            total_bytes: 900,
            acknowledged_request_id: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"library_update""#));
        assert!(json.contains(r#""scanned_at_unix_secs":42"#));
        assert!(json.contains(r#""albums""#));
    }

    #[test]
    fn library_update_never_scanned_serializes_null_timestamp() {
        let evt = DaemonEvent::LibraryUpdate {
            source_root: None,
            scanned_at_unix_secs: None,
            artists: vec![],
            genres: vec![],
            total_tracks: 0,
            total_bytes: 0,
            acknowledged_request_id: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(
            json.contains(r#""scanned_at_unix_secs":null"#),
            "null (not omitted) — the UI branches on it for the never-scanned state"
        );
    }

    #[test]
    fn selection_update_and_preview_serialize() {
        let upd = DaemonEvent::SelectionUpdate {
            mode: crate::selection::SelectionMode::Exclude,
            rules: vec![crate::selection::SelectionRule::Genre {
                name: "Podcast".into(),
            }],
            serial: None,
            acknowledged_request_id: None,
        };
        let json = serde_json::to_string(&upd).unwrap();
        assert!(json.contains(r#""type":"selection_update""#));
        assert!(json.contains(r#""mode":"exclude""#));

        let prev = DaemonEvent::SelectionPreview {
            selected_tracks: 2340,
            selected_bytes: 14_200_000_000,
            adds: 120,
            removes: 214,
            serial: "serial-a".into(),
            acknowledged_request_id: "request-a".into(),
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
            serde_json::from_str(r#"{"type":"get_status","request_id":"request-a"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::GetStatus { .. }));
    }

    #[test]
    fn decodes_pause_command() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"pause","serial":"serial-a","request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::Pause { .. }));
    }

    #[test]
    fn save_config_with_partial_payload_deserializes() {
        let json = r#"{"type":"save_config","ipod":{"serial":"X","model_label":"iPod 7G"},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveConfig {
                source,
                daemon,
                ipod,
                ..
            } => {
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
        let json = r#"{"type":"backfill_rockbox","serial":"serial-a","request_id":"request-a"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(json).unwrap(),
            DaemonCommand::BackfillRockbox { .. }
        ));
    }

    #[test]
    fn replace_library_deserializes() {
        let json = r#"{"type":"replace_library","serial":"serial-a","request_id":"request-a"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(json).unwrap(),
            DaemonCommand::ReplaceLibrary { .. }
        ));
    }

    /// `replace_library`'s busy/no-device guards (runtime.rs) reply with
    /// these two `SyncRejected` variants — unlike `TriggerSync`'s NotConfigured
    /// path, `replace_library` never sends that third reason. Locks the wire
    /// shape those replies depend on.
    #[test]
    fn sync_rejected_serializes_already_syncing_and_no_ipod() {
        let already_syncing = DaemonEvent::SyncRejected {
            reason: SyncRejectReason::AlreadySyncing,
            serial: "serial-a".into(),
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&already_syncing).unwrap();
        assert!(json.contains(r#""type":"sync_rejected""#));
        assert!(
            json.contains(r#""reason":"already_syncing""#),
            "got: {json}"
        );

        let no_ipod = DaemonEvent::SyncRejected {
            reason: SyncRejectReason::NoIpod,
            serial: "serial-a".into(),
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&no_ipod).unwrap();
        assert!(json.contains(r#""reason":"no_ipod""#), "got: {json}");
    }

    #[test]
    fn trigger_sync_round_trips() {
        let json = r#"{"type":"trigger_sync","source":"manual","serial":"serial-a","request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        assert!(matches!(
            cmd,
            DaemonCommand::TriggerSync {
                source: TriggerSource::Manual,
                ..
            }
        ));
    }

    #[test]
    fn sync_event_serializes_with_line_field() {
        let evt = DaemonEvent::SyncEvent {
            line: r#"{"type":"track_done"}"#.to_string(),
            serial: None,
            session_id: 1,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""type":"sync_event""#));
        assert!(
            json.contains(r#""line":"{\"type\":\"track_done\"}""#),
            "got: {json}"
        );
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
            serde_json::from_str(r#"{"type":"list_playlists","request_id":"request-a"}"#).unwrap();
        assert!(matches!(cmd, DaemonCommand::ListPlaylists { .. }));
    }

    #[test]
    fn get_playlist_deserializes() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"get_playlist","slug":"gym","request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::GetPlaylist { slug, .. } if slug == "gym"));
    }

    #[test]
    fn delete_playlist_deserializes() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"delete_playlist","slug":"gym","request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::DeletePlaylist { slug, .. } if slug == "gym"));
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
            playlist_revision: 7,
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":["Artist/Album/01.flac","B/02.flac"],"playlist_revision":7,"acknowledged_request_id":"request-a"}"#,
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
            playlist_revision: 8,
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"recent-idm","name":"Recent IDM","kind":"smart","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0},"playlist_revision":8,"acknowledged_request_id":"request-a"}"#,
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
            playlist_revision: 8,
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlist_detail","slug":"ghost","error":"no such playlist","playlist_revision":8,"acknowledged_request_id":"request-a"}"#,
            "got: {json}"
        );
    }

    #[test]
    fn save_playlist_manual_without_slug_deserializes_as_create() {
        let json = r#"{"type":"save_playlist","playlist":
            {"kind":"manual","name":"Gym","tracks":["Artist/Album/01.flac","B/02.flac"]},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist {
                playlist: PlaylistPayload::Manual { slug, name, tracks },
                ..
            } => {
                assert_eq!(slug, None, "absent slug means create");
                assert_eq!(name, "Gym");
                assert_eq!(
                    tracks,
                    vec!["Artist/Album/01.flac".to_string(), "B/02.flac".to_string()]
                );
            }
            _ => panic!("expected SavePlaylist(Manual)"),
        }
    }

    #[test]
    fn save_playlist_manual_with_slug_deserializes_as_edit() {
        let json = r#"{"type":"save_playlist","playlist":
            {"kind":"manual","slug":"gym","name":"Gym Mix","tracks":[]},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist {
                playlist: PlaylistPayload::Manual { slug, name, tracks },
                ..
            } => {
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
                    {"field":"genre","op":"is","value":"IDM"}]}},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SavePlaylist {
                playlist: PlaylistPayload::Smart { slug, name, rules },
                ..
            } => {
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
                    slug: "gym".into(),
                    name: "Gym".into(),
                    kind: PlaylistKind::Manual,
                    tracks: 12,
                    bytes: 900,
                    error: None,
                },
                PlaylistSummary {
                    slug: "broken".into(),
                    name: "broken".into(),
                    kind: PlaylistKind::Smart,
                    tracks: 0,
                    bytes: 0,
                    error: Some("parse failed".into()),
                },
            ],
            playlist_revision: 9,
            acknowledged_request_id: Some("request-a".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"playlists_update","playlists":[{"slug":"gym","name":"Gym","kind":"manual","tracks":12,"bytes":900},{"slug":"broken","name":"broken","kind":"smart","tracks":0,"bytes":0,"error":"parse failed"}],"playlist_revision":9,"acknowledged_request_id":"request-a"}"#,
            "got: {json}"
        );
    }

    // --- v1.6.0: per-device config -----------------------------------

    #[test]
    fn get_device_config_deserializes() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"get_device_config","serial":"0xABC","request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::GetDeviceConfig { serial, .. } if serial == "0xABC"));
    }

    #[test]
    fn save_device_config_partial_payload_deserializes() {
        // Only `settings` provided — `selection`/`subscriptions` are the
        // wire-level "don't change" sentinel, same convention as `save_config`.
        let json = r#"{"type":"save_device_config","serial":"0xABC",
            "settings":{"auto_sync":false,"rockbox_compat":true},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveDeviceConfig {
                serial,
                selection,
                subscriptions,
                settings,
                ..
            } => {
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
            "settings":{"auto_sync":true,"rockbox_compat":false},"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::SaveDeviceConfig {
                serial,
                selection,
                subscriptions,
                settings,
                ..
            } => {
                assert_eq!(serial, "0xABC");
                let selection = selection.expect("selection present");
                assert_eq!(selection.mode, crate::selection::SelectionMode::Include);
                assert_eq!(selection.rules.len(), 1);
                assert_eq!(
                    subscriptions.expect("subscriptions present").playlists,
                    vec!["gym", "chill"]
                );
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
            subscriptions: SubscriptionsPayload {
                playlists: vec!["gym".into()],
            },
            settings: DeviceSettingsPayload {
                auto_sync: true,
                rockbox_compat: false,
            },
            selection_revision: 3,
            settings_revision: 4,
            subscriptions_revision: 5,
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"device_config_update","serial":"0xABC","selection":{"mode":"include","rules":[]},"subscriptions":{"playlists":["gym"]},"settings":{"auto_sync":true,"rockbox_compat":false},"selection_revision":3,"settings_revision":4,"subscriptions_revision":5,"acknowledged_request_id":"request-a"}"#
        );
    }

    #[test]
    fn command_failed_serializes_without_a_revision() {
        let evt = DaemonEvent::CommandFailed {
            acknowledged_request_id: "request-a".into(),
            error: "persist failed".into(),
        };

        assert_eq!(
            serde_json::to_string(&evt).unwrap(),
            r#"{"type":"command_failed","acknowledged_request_id":"request-a","error":"persist failed"}"#
        );
    }

    // --- v1.6.0: device preview ---------------------------------------

    #[test]
    fn preview_device_deserializes() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"preview_device","serial":"0xABC","request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::PreviewDevice { serial, .. } if serial == "0xABC"));
    }

    #[test]
    fn device_preview_serializes_projected_free_as_value_or_null() {
        let connected = DaemonEvent::DevicePreview {
            serial: "serial-a".into(),
            selected_tracks: 100,
            selected_bytes: 5_000_000_000,
            playlist_extra_tracks: 3,
            playlist_extra_bytes: 90_000_000,
            projected_free_bytes: Some(1_200_000_000),
            unresolved_subscriptions: vec![],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&connected).unwrap();
        assert!(json.contains(r#""type":"device_preview""#));
        assert!(json.contains(r#""projected_free_bytes":1200000000"#));

        let disconnected = DaemonEvent::DevicePreview {
            serial: "serial-a".into(),
            selected_tracks: 100,
            selected_bytes: 5_000_000_000,
            playlist_extra_tracks: 3,
            playlist_extra_bytes: 90_000_000,
            projected_free_bytes: None,
            unresolved_subscriptions: vec![],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&disconnected).unwrap();
        assert!(
            json.contains(r#""projected_free_bytes":null"#),
            "null (not omitted) — mirrors library_update's never-scanned convention"
        );
    }

    #[test]
    fn device_preview_unresolved_subscriptions_present_or_omitted() {
        let with_unresolved = DaemonEvent::DevicePreview {
            serial: "serial-a".into(),
            selected_tracks: 1,
            selected_bytes: 1,
            playlist_extra_tracks: 0,
            playlist_extra_bytes: 0,
            projected_free_bytes: None,
            unresolved_subscriptions: vec!["ghost".into(), "gym".into()],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&with_unresolved).unwrap();
        assert!(json.contains(r#""unresolved_subscriptions":["ghost","gym"]"#));

        let none_unresolved = DaemonEvent::DevicePreview {
            serial: "serial-a".into(),
            selected_tracks: 1,
            selected_bytes: 1,
            playlist_extra_tracks: 0,
            playlist_extra_bytes: 0,
            projected_free_bytes: None,
            unresolved_subscriptions: vec![],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&none_unresolved).unwrap();
        assert!(
            !json.contains("unresolved_subscriptions"),
            "omitted entirely when empty, not serialized as []"
        );
    }

    // --- v1.7.0: resolve_tracks -----------------------------------------

    #[test]
    fn resolve_tracks_command_deserializes_reusing_selection_rule_shape() {
        // Doc-literal payload from docs/ipc-protocol.md "Daemon v1.7.0" —
        // `rules` reuses the exact same wire shape as
        // `save_device_config`'s `selection.rules`.
        let json = r#"{"type":"resolve_tracks","rules":[
            {"kind":"artist","name":"Boards of Canada"},
            {"kind":"album","artist":"Aphex Twin","album":"Drukqs"},
            {"kind":"genre","name":"Ambient"}],"request_id":"request-a"}"#;
        let cmd: DaemonCommand = serde_json::from_str(json).unwrap();
        match cmd {
            DaemonCommand::ResolveTracks { rules, .. } => {
                assert_eq!(rules.len(), 3);
                assert_eq!(
                    rules[0],
                    SelectionRule::Artist {
                        name: "Boards of Canada".into()
                    }
                );
                assert_eq!(
                    rules[1],
                    SelectionRule::Album {
                        artist: "Aphex Twin".into(),
                        album: "Drukqs".into(),
                    }
                );
                assert_eq!(
                    rules[2],
                    SelectionRule::Genre {
                        name: "Ambient".into()
                    }
                );
            }
            _ => panic!("expected ResolveTracks"),
        }
    }

    #[test]
    fn resolve_tracks_command_with_empty_rules_deserializes() {
        let cmd: DaemonCommand = serde_json::from_str(
            r#"{"type":"resolve_tracks","rules":[],"request_id":"request-a"}"#,
        )
        .unwrap();
        assert!(matches!(cmd, DaemonCommand::ResolveTracks { rules, .. } if rules.is_empty()));
    }

    #[test]
    fn resolved_tracks_event_serializes_doc_literal_shape() {
        let evt = DaemonEvent::ResolvedTracks {
            tracks: vec!["Artist/Album/01.flac".into(), "Artist/Album/02.flac".into()],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"resolved_tracks","tracks":["Artist/Album/01.flac","Artist/Album/02.flac"],"acknowledged_request_id":"request-a"}"#,
            "got: {json}"
        );
    }

    #[test]
    fn resolved_tracks_event_empty_tracks_is_a_valid_reply_not_an_error() {
        let evt = DaemonEvent::ResolvedTracks {
            tracks: vec![],
            acknowledged_request_id: "request-a".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert_eq!(
            json,
            r#"{"type":"resolved_tracks","tracks":[],"acknowledged_request_id":"request-a"}"#
        );
    }

    // --- v2.0.0: serial-keyed inventory + request correlation ---------

    #[test]
    fn protocol_version_is_2_0_0() {
        assert_eq!(DAEMON_PROTOCOL_VERSION, "2.0.0");
    }

    #[test]
    fn save_config_rejects_legacy_and_accepts_exact_v2_json() {
        let legacy = r#"{"type":"save_config","source":"/music"}"#;
        assert!(serde_json::from_str::<DaemonCommand>(legacy).is_err());

        let correlated = r#"{"type":"save_config","request_id":"req-config","source":"/music"}"#;
        match serde_json::from_str::<DaemonCommand>(correlated).unwrap() {
            DaemonCommand::SaveConfig {
                source, request_id, ..
            } => {
                assert_eq!(source.as_deref(), Some("/music"));
                assert_eq!(request_id, "req-config");
            }
            _ => panic!("expected SaveConfig"),
        }
    }

    #[test]
    fn targeted_commands_reject_legacy_and_accept_exact_v2_json() {
        let legacy = r#"{"type":"trigger_sync","source":"manual"}"#;
        assert!(serde_json::from_str::<DaemonCommand>(legacy).is_err());

        let new =
            r#"{"type":"trigger_sync","source":"manual","serial":"RAW-A","request_id":"req-sync"}"#;
        match serde_json::from_str::<DaemonCommand>(new).unwrap() {
            DaemonCommand::TriggerSync {
                serial, request_id, ..
            } => {
                assert_eq!(serial, "RAW-A");
                assert_eq!(request_id, "req-sync");
            }
            _ => panic!("expected TriggerSync"),
        }
    }

    #[test]
    fn every_device_mutation_deserializes_with_serial_and_request_id() {
        let fixtures = [
            r#"{"type":"forget_ipod","serial":"RAW-A","request_id":"req-1"}"#,
            r#"{"type":"trigger_sync","source":"manual","serial":"RAW-A","request_id":"req-2"}"#,
            r#"{"type":"cancel_sync","serial":"RAW-A","request_id":"req-3"}"#,
            r#"{"type":"pause","serial":"RAW-A","request_id":"req-4"}"#,
            r#"{"type":"decide_prompt","id":7,"choice":1,"serial":"RAW-A","request_id":"req-5"}"#,
            r#"{"type":"backfill_rockbox","serial":"RAW-A","request_id":"req-6"}"#,
            r#"{"type":"replace_library","serial":"RAW-A","request_id":"req-7"}"#,
            r#"{"type":"save_device_config","serial":"RAW-A","request_id":"req-8","settings":{"auto_sync":true,"rockbox_compat":false}}"#,
        ];

        for fixture in fixtures {
            let command: DaemonCommand = serde_json::from_str(fixture).unwrap();
            assert_eq!(command.target_serial(), Some("RAW-A"), "fixture: {fixture}");
            assert!(command.request_id().is_some(), "fixture: {fixture}");
        }
    }

    #[test]
    fn deprecated_singleton_selection_commands_are_rejected() {
        assert!(serde_json::from_str::<DaemonCommand>(r#"{"type":"get_selection"}"#).is_err());
        assert!(serde_json::from_str::<DaemonCommand>(
            r#"{"type":"save_selection","mode":"all","rules":[]}"#,
        )
        .is_err());
    }

    #[test]
    fn device_inventory_snapshot_round_trips_two_devices() {
        use crate::ipc_device::{
            DeviceIdentitySnapshot, DeviceInventorySnapshot, DevicePhaseLabel, DeviceSnapshot,
        };

        let device = |serial: &str, configured: bool, connected: bool, phase| DeviceSnapshot {
            identity: DeviceIdentitySnapshot {
                serial: serial.into(),
                model_label: "iPod Classic".into(),
                name: Some(format!("Device {serial}")),
            },
            configured,
            connected,
            mount: connected.then(|| format!("/Volumes/{serial}")),
            phase,
            session_id: None,
            storage: None,
            synced_count: 12,
            library_count: Some(20),
            latest_successful_sync: None,
            latest_attempt: None,
            last_terminal_error: None,
            selection_revision: 3,
            settings_revision: 4,
            subscriptions_revision: 5,
        };
        let snapshot = DeviceInventorySnapshot {
            revision: 9,
            devices: vec![
                device("RAW-A", true, true, DevicePhaseLabel::Idle),
                device("raw-B", false, true, DevicePhaseLabel::Unconfigured),
            ],
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: DeviceInventorySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
        assert!(json.contains(r#""serial":"RAW-A""#));
        assert!(
            json.contains(r#""serial":"raw-B""#),
            "raw wire identity must be preserved"
        );
    }

    #[test]
    fn correlated_replies_echo_request_serial_and_session() {
        let config = DaemonEvent::ConfigUpdate {
            source: Some("/music".into()),
            daemon: None,
            ipod: None,
            config_revision: 4,
            acknowledged_request_id: Some("req-config".into()),
        };
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"type":"config_update","source":"/music","daemon":null,"ipod":null,"config_revision":4,"acknowledged_request_id":"req-config"}"#,
        );

        let sync = DaemonEvent::SyncEvent {
            line: r#"{"type":"track_done"}"#.into(),
            serial: Some("RAW-A".into()),
            session_id: 42,
        };
        assert_eq!(
            serde_json::to_string(&sync).unwrap(),
            r#"{"type":"sync_event","line":"{\"type\":\"track_done\"}","serial":"RAW-A","session_id":42}"#,
        );

        let rejected = DaemonEvent::SyncRejected {
            reason: SyncRejectReason::AlreadySyncing,
            serial: "RAW-A".into(),
            acknowledged_request_id: "req-sync".into(),
        };
        assert_eq!(
            serde_json::to_string(&rejected).unwrap(),
            r#"{"type":"sync_rejected","reason":"already_syncing","serial":"RAW-A","acknowledged_request_id":"req-sync"}"#,
        );
    }
}
