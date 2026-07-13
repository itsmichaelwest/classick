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
public abstract record DaemonEvent;

// TODO(windows): pause/resume + X-of-Y not yet wired on Windows.
// Daemon protocol bumped to 1.2.0: `status_update` gained a required
// `synced_count` (usize) and an optional `library_count` (usize, omitted
// when unknown) — see docs/ipc-protocol.md "Daemon v1.2.0". StatusUpdateEvent
// below doesn't decode either field yet, so "X of Y synced" can't be
// rendered on Windows. There's also no terminal `paused` SyncEvent case to
// distinguish a graceful pause from a normal completion or abort. Mirror
// the Rust daemon (crates/classick/src/ipc_daemon.rs::DaemonEvent::StatusUpdate)
// and the macOS client (ui/macos/Sources/Classick/Ipc/WireModels.swift,
// `StatusInfo.syncedCount`/`libraryCount`, `SyncEvent.paused`) plus a
// Pause/Resume affordance in the tray UI. Can't build/verify Windows in
// this environment — see docs/ipc-protocol.md.
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
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod
) : DaemonEvent;

public sealed record HistoryUpdateEvent(
    [property: JsonPropertyName("entries")] IReadOnlyList<HistoryEntry> Entries
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
    [property: JsonPropertyName("reason")] string Reason
) : DaemonEvent;

public sealed record SyncEventEnvelope(
    [property: JsonPropertyName("line")] string Line
) : DaemonEvent;

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
    [property: JsonPropertyName("summary")] SyncSummary? Summary
);

public sealed record SyncSummary(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged,
    [property: JsonPropertyName("skipped")] int Skipped
);
