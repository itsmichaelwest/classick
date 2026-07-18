using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

/// <summary>
/// Daemon-side events sent over the UI ↔ daemon named pipe. Augments
/// (does not replace) the M1 <see cref="IpcEvent"/> hierarchy — sync-
/// subprocess events (Header, Summary, Review, etc.) are forwarded by
/// the daemon and arrive on the SAME pipe, deserialized via the M1
/// IpcEvent polymorphic table.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StatusUpdateEvent), "status_update")]
[JsonDerivedType(typeof(ConfigUpdateEvent), "config_update")]
[JsonDerivedType(typeof(HistoryUpdateEvent), "history_update")]
[JsonDerivedType(typeof(DeviceConnectedEvent), "device_connected")]
[JsonDerivedType(typeof(DeviceDisconnectedEvent), "device_disconnected")]
[JsonDerivedType(typeof(SyncRejectedEvent), "sync_rejected")]
[JsonDerivedType(typeof(SyncEventEnvelope), "sync_event")]
[JsonDerivedType(typeof(DeviceInventorySnapshotEvent), "device_inventory_snapshot")]
[JsonDerivedType(typeof(LibraryUpdateEvent), "library_update")]
[JsonDerivedType(typeof(SelectionUpdateEvent), "selection_update")]
[JsonDerivedType(typeof(SelectionPreviewEvent), "selection_preview")]
[JsonDerivedType(typeof(PlaylistsUpdateEvent), "playlists_update")]
[JsonDerivedType(typeof(PlaylistDetailEvent), "playlist_detail")]
[JsonDerivedType(typeof(DeviceConfigUpdateEvent), "device_config_update")]
[JsonDerivedType(typeof(DevicePreviewEvent), "device_preview")]
[JsonDerivedType(typeof(ResolvedTracksEvent), "resolved_tracks")]
public abstract record DaemonEvent;

public sealed record StatusUpdateEvent(
    [property: JsonRequired, JsonPropertyName("state")] string State,
    [property: JsonRequired, JsonPropertyName("configured")] bool Configured,
    [property: JsonRequired, JsonPropertyName("ipod_connected")] bool IpodConnected,
    [property: JsonRequired, JsonPropertyName("last_sync")] HistoryEntry? LastSync,
    [property: JsonRequired, JsonPropertyName("next_scheduled_unix_secs")] long? NextScheduledUnixSecs,
    [property: JsonPropertyName("storage")] StorageInfo? Storage,
    [property: JsonRequired, JsonPropertyName("synced_count")] int SyncedCount,
    [property: JsonPropertyName("library_count")] int? LibraryCount,
    [property: JsonPropertyName("acknowledged_request_id")] string? AcknowledgedRequestId
) : DaemonEvent;

public sealed record StorageInfo(
    [property: JsonPropertyName("total_bytes")] ulong TotalBytes,
    [property: JsonPropertyName("free_bytes")] ulong FreeBytes
);

public sealed record ConfigUpdateEvent(
    [property: JsonPropertyName("source")] string? Source,
    [property: JsonPropertyName("daemon")] DaemonSettings? Daemon,
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod,
    [property: JsonRequired, JsonPropertyName("config_revision")] ulong ConfigRevision,
    [property: JsonPropertyName("acknowledged_request_id")] string? AcknowledgedRequestId = null
) : DaemonEvent;

public sealed record HistoryUpdateEvent(
    [property: JsonPropertyName("entries")] IReadOnlyList<HistoryEntry> Entries,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record DeviceConnectedEvent(
    [property: JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("model_label")] string ModelLabel,
    [property: JsonPropertyName("drive")] string Drive,
    [property: JsonPropertyName("name")] string? Name = null
) : DaemonEvent;

public sealed record DeviceDisconnectedEvent(
    [property: JsonPropertyName("serial")] string Serial
) : DaemonEvent;

public sealed record SyncRejectedEvent(
    [property: JsonPropertyName("reason")] string Reason,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record SyncEventEnvelope(
    [property: JsonPropertyName("line")] string Line,
    [property: JsonPropertyName("serial")] string? Serial,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId
) : DaemonEvent;

public sealed record DeviceInventorySnapshotEvent(
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("devices")] IReadOnlyList<DeviceSnapshot> Devices
) : DaemonEvent;

public sealed record DeviceSnapshot(
    [property: JsonRequired, JsonPropertyName("identity")] DeviceIdentitySnapshot Identity,
    [property: JsonRequired, JsonPropertyName("configured")] bool Configured,
    [property: JsonRequired, JsonPropertyName("connected")] bool Connected,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("mount")] string? Mount,
    [property: JsonRequired, JsonPropertyName("phase")] string Phase,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("session_id")] ulong? SessionId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("storage")] StorageInfo? Storage,
    [property: JsonRequired, JsonPropertyName("synced_count")] int SyncedCount,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("library_count")] int? LibraryCount,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("latest_successful_sync")] HistoryEntry? LatestSuccessfulSync,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("latest_attempt")] HistoryEntry? LatestAttempt,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("last_terminal_error")] string? LastTerminalError,
    [property: JsonRequired, JsonPropertyName("selection_revision")] ulong SelectionRevision,
    [property: JsonRequired, JsonPropertyName("settings_revision")] ulong SettingsRevision,
    [property: JsonRequired, JsonPropertyName("subscriptions_revision")] ulong SubscriptionsRevision
);

public sealed record DeviceIdentitySnapshot(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("model_label")] string ModelLabel,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("name")] string? Name = null
);

public sealed record LibraryUpdateEvent(
    [property: JsonPropertyName("source_root")] string? SourceRoot,
    [property: JsonPropertyName("scanned_at_unix_secs")] ulong? ScannedAtUnixSecs,
    [property: JsonPropertyName("artists")] IReadOnlyList<LibraryArtist> Artists,
    [property: JsonPropertyName("genres")] IReadOnlyList<LibraryGenre> Genres,
    [property: JsonPropertyName("total_tracks")] int TotalTracks,
    [property: JsonPropertyName("total_bytes")] ulong TotalBytes,
    [property: JsonPropertyName("acknowledged_request_id")] string? AcknowledgedRequestId = null
) : DaemonEvent;

public sealed record LibraryArtist(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("albums")] IReadOnlyList<LibraryAlbum> Albums
);

public sealed record LibraryAlbum(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("genre")] string? Genre,
    [property: JsonPropertyName("tracks")] int Tracks,
    [property: JsonPropertyName("bytes")] ulong Bytes
);

public sealed record LibraryGenre(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("tracks")] int Tracks,
    [property: JsonPropertyName("bytes")] ulong Bytes
);

public sealed record SelectionUpdateEvent(
    [property: JsonPropertyName("mode")] SelectionMode Mode,
    [property: JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules,
    [property: JsonPropertyName("serial")] string? Serial = null,
    [property: JsonPropertyName("acknowledged_request_id")] string? AcknowledgedRequestId = null
) : DaemonEvent;

public sealed record SelectionPreviewEvent(
    [property: JsonRequired, JsonPropertyName("selected_tracks")] int SelectedTracks,
    [property: JsonRequired, JsonPropertyName("selected_bytes")] ulong SelectedBytes,
    [property: JsonRequired, JsonPropertyName("adds")] int Adds,
    [property: JsonRequired, JsonPropertyName("removes")] int Removes,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record PlaylistsUpdateEvent(
    [property: JsonPropertyName("playlists")] IReadOnlyList<PlaylistSummary> Playlists,
    [property: JsonPropertyName("acknowledged_request_id")] string? AcknowledgedRequestId = null
) : DaemonEvent;

public sealed record PlaylistDetailEvent(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("name")] string? Name,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("kind")] PlaylistKind? Kind,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("tracks")] IReadOnlyList<string>? Tracks,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("rules")] SmartRules? Rules,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("error")] string? Error,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record DeviceConfigUpdateEvent(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("selection")] SelectionState Selection,
    [property: JsonRequired, JsonPropertyName("subscriptions")] Subscriptions Subscriptions,
    [property: JsonRequired, JsonPropertyName("settings")] DeviceSettings Settings,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record DevicePreviewEvent(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("selected_tracks")] int SelectedTracks,
    [property: JsonRequired, JsonPropertyName("selected_bytes")] ulong SelectedBytes,
    [property: JsonRequired, JsonPropertyName("playlist_extra_tracks")] int PlaylistExtraTracks,
    [property: JsonRequired, JsonPropertyName("playlist_extra_bytes")] ulong PlaylistExtraBytes,
    [property: JsonPropertyName("projected_free_bytes")] ulong? ProjectedFreeBytes,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("unresolved_subscriptions")] IReadOnlyList<string>? UnresolvedSubscriptions,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record ResolvedTracksEvent(
    [property: JsonRequired, JsonPropertyName("tracks")] IReadOnlyList<string> Tracks,
    [property: JsonRequired, JsonPropertyName("acknowledged_request_id")] string AcknowledgedRequestId
) : DaemonEvent;

public sealed record DaemonSettings(
    [property: JsonPropertyName("enabled")] bool Enabled,
    [property: JsonPropertyName("autostart_with_windows")] bool AutostartWithWindows,
    [property: JsonPropertyName("first_sync_mode")] string FirstSyncMode,
    [property: JsonPropertyName("subsequent_sync_mode")] string SubsequentSyncMode,
    [property: JsonPropertyName("schedule_minutes")] uint ScheduleMinutes,
    [property: JsonPropertyName("notify_on")] string NotifyOn,
    [property: JsonPropertyName("rockbox_compat")] bool RockboxCompat
);

public sealed record IpodIdentity(
    [property: JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("model_label")] string ModelLabel,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("name")] string? Name,
    [property: JsonPropertyName("custom_selection")] bool CustomSelection
);

public sealed record HistoryEntry(
    [property: JsonPropertyName("timestamp")] string Timestamp,
    [property: JsonPropertyName("duration_secs")] ulong DurationSecs,
    [property: JsonPropertyName("trigger")] string Trigger,
    [property: JsonPropertyName("outcome")] string Outcome,
    [property: JsonPropertyName("error_message")] string? ErrorMessage,
    [property: JsonPropertyName("summary")] SyncSummary? Summary,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("session_id")] ulong? SessionId = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("db_restored")] bool DbRestored = false
);

public sealed record SyncSummary(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged,
    [property: JsonPropertyName("skipped")] int Skipped,
    [property: JsonRequired, JsonPropertyName("metadata_only")] int MetadataOnly,
    [property: JsonRequired, JsonPropertyName("skipped_for_space_tracks")] int SkippedForSpaceTracks,
    [property: JsonRequired, JsonPropertyName("skipped_for_space_bytes")] ulong SkippedForSpaceBytes,
    [property: JsonRequired, JsonPropertyName("artwork_failed_sources")] int ArtworkFailedSources
);
