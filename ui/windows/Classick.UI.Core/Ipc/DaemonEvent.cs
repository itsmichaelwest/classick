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
public abstract record DaemonEvent;

public sealed record StatusUpdateEvent(
    [property: JsonPropertyName("state")] string State,
    [property: JsonPropertyName("configured")] bool Configured,
    [property: JsonPropertyName("ipod_connected")] bool IpodConnected,
    [property: JsonPropertyName("last_sync")] HistoryEntry? LastSync,
    [property: JsonPropertyName("next_scheduled_unix_secs")] long? NextScheduledUnixSecs,
    [property: JsonPropertyName("storage")] StorageInfo? Storage = null
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

public sealed record DaemonSettings(
    [property: JsonPropertyName("enabled")] bool Enabled,
    [property: JsonPropertyName("autostart_with_windows")] bool AutostartWithWindows,
    [property: JsonPropertyName("first_sync_mode")] string FirstSyncMode,
    [property: JsonPropertyName("subsequent_sync_mode")] string SubsequentSyncMode,
    [property: JsonPropertyName("schedule_minutes")] uint ScheduleMinutes,
    [property: JsonPropertyName("notify_on")] string NotifyOn
);

public sealed record IpodIdentity(
    [property: JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("model_label")] string ModelLabel,
    [property: JsonPropertyName("name")] string? Name = null
);

public sealed record HistoryEntry(
    [property: JsonPropertyName("timestamp")] string Timestamp,
    [property: JsonPropertyName("duration_secs")] ulong DurationSecs,
    [property: JsonPropertyName("trigger")] string Trigger,
    [property: JsonPropertyName("outcome")] string Outcome,
    [property: JsonPropertyName("error_message")] string? ErrorMessage,
    [property: JsonPropertyName("summary")] SyncSummary? Summary,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonPropertyName("session_id")] ulong? SessionId = null
);

public sealed record SyncSummary(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged,
    [property: JsonPropertyName("skipped")] int Skipped
);
