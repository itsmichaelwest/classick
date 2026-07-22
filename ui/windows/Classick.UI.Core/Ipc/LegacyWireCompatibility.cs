using System.Text.Json;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

// Kept only while the existing Windows reducers migrate to the protocol-3
// models. DaemonClient and DaemonEventRouter do not decode nested legacy JSON.
public interface IpcEvent;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(GetStatusCommand), "get_status")]
[JsonDerivedType(typeof(GetConfigCommand), "get_config")]
[JsonDerivedType(typeof(SaveConfigCommand), "save_config")]
[JsonDerivedType(typeof(ForgetIpodCommand), "forget_ipod")]
[JsonDerivedType(typeof(TriggerSyncCommand), "trigger_sync")]
[JsonDerivedType(typeof(GetHistoryCommand), "get_history")]
[JsonDerivedType(typeof(SubscribeDeviceEventsCommand), "subscribe_device_events")]
[JsonDerivedType(typeof(UnsubscribeDeviceEventsCommand), "unsubscribe_device_events")]
[JsonDerivedType(typeof(CancelSyncCommand), "cancel_sync")]
[JsonDerivedType(typeof(PauseCommand), "pause")]
[JsonDerivedType(typeof(DecidePromptCommand), "decide_prompt")]
[JsonDerivedType(typeof(RetrySourceMountCommand), "retry_source_mount")]
[JsonDerivedType(typeof(ShutdownCommand), "shutdown")]
public abstract record DaemonCommand;

public sealed record GetStatusCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record GetConfigCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record SaveConfigCommand(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("source")] string? Source,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("daemon")] DaemonSettings? Daemon,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("ipod")] IpodIdentity? Ipod,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record ForgetIpodCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record GetHistoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("limit")] int? Limit = null) : DaemonCommand;

public sealed record SubscribeDeviceEventsCommand : DaemonCommand;
public sealed record UnsubscribeDeviceEventsCommand : DaemonCommand;

public sealed record RetrySourceMountCommand(
    [property: JsonRequired, JsonPropertyName("allow_ui")] bool AllowUi,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StatusUpdateEvent), "status_update")]
[JsonDerivedType(typeof(ConfigUpdateEvent), "config_update")]
[JsonDerivedType(typeof(HistoryUpdateEvent), "history_update")]
[JsonDerivedType(typeof(DeviceConnectedEvent), "device_connected")]
[JsonDerivedType(typeof(DeviceDisconnectedEvent), "device_disconnected")]
[JsonDerivedType(typeof(SyncRejectedEvent), "sync_rejected")]
[JsonDerivedType(typeof(SyncEventEnvelope), "sync_event")]
[JsonDerivedType(typeof(DeviceInventorySnapshotEvent), "device_inventory_snapshot")]
[JsonDerivedType(typeof(SourceAvailabilityEvent), "source_availability")]
public abstract record DaemonEvent;

public sealed record SourceAvailabilityEvent : DaemonEvent, IJsonOnDeserialized
{
    public SourceAvailabilityEvent()
    {
    }

    public SourceAvailabilityEvent(
        SourceAvailabilityState state,
        string? sourceRoot = null,
        string? acknowledgedRequestId = null)
    {
        State = state;
        SourceRootWire = sourceRoot is null ? default : JsonSerializer.SerializeToElement(sourceRoot);
        AcknowledgedRequestId = acknowledgedRequestId;
        ValidateShape();
    }

    [JsonRequired, JsonPropertyName("state")]
    public SourceAvailabilityState State { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("source_root")]
    public JsonElement SourceRootWire { get; init; }

    [JsonIgnore]
    public string? SourceRoot => SourceRootWire.ValueKind == JsonValueKind.String
        ? SourceRootWire.GetString()
        : null;

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("acknowledged_request_id")]
    public string? AcknowledgedRequestId { get; init; }

    public void OnDeserialized() => ValidateShape();

    private void ValidateShape()
    {
        if (State == SourceAvailabilityState.Available)
        {
            if (SourceRootWire.ValueKind != JsonValueKind.String)
                throw new JsonException("available source requires a string root");
        }
        else if (SourceRootWire.ValueKind != JsonValueKind.Undefined)
        {
            throw new JsonException("unavailable source must omit its root");
        }
    }
}

public sealed record HeaderEvent(string Source, string Ipod, string Manifest) : IpcEvent;

public sealed record ActionPlanSummary(
    int Add,
    int Modify,
    int MetadataOnly,
    int Remove,
    int Unchanged);

public sealed record SummaryEvent(
    int Add,
    int Modify,
    int MetadataOnly,
    int Remove,
    int Unchanged,
    int TotalPlanned) : IpcEvent;

public sealed record ReviewEvent(ActionPlanSummary Summary, bool NoDelete) : IpcEvent;
public sealed record PromptEvent(ulong Id, string Message, IReadOnlyList<string> Options) : IpcEvent;
public sealed record FormEvent(ulong Id, string Label, string Initial, string Hint) : IpcEvent;
public sealed record TrackStartEvent(int Current, int Total, string Label, ulong? EtaSecs = null) : IpcEvent;
public sealed record TrackDoneEvent(TrackResult Result) : IpcEvent;

[JsonConverter(typeof(StrictStringEnumConverter<FinalizationReason>))]
public enum FinalizationReason
{
    [JsonStringEnumMemberName("cancelled")] Cancelled,
    [JsonStringEnumMemberName("paused")] Paused,
}

public sealed record FinalizingEvent(
    FinalizationReason Reason,
    int StagedAlbums,
    int StagedTracks) : IpcEvent;

public sealed record CancelledEvent : IpcEvent;
public sealed record PausedEvent : IpcEvent;
public sealed record LogEvent(string Message) : IpcEvent;
public sealed record ErrorEvent(string Message, IReadOnlyList<string>? RecoveryHints = null) : IpcEvent;
public sealed record FinishEvent(
    bool Success,
    SkippedForSpaceSummary? SkippedForSpace = null,
    ArtworkSummary? Artwork = null,
    bool DbRestored = false) : IpcEvent;

public abstract record ReviewDecisionPayload;
public sealed record ApplyDecision(bool NoDelete) : ReviewDecisionPayload;
public sealed record DryRunDecision : ReviewDecisionPayload;
public sealed record QuitDecision : ReviewDecisionPayload;
public sealed record ReviewDecisionCommand(ReviewDecisionPayload Decision);

public sealed record StorageInfo(ulong TotalBytes, ulong FreeBytes);

public sealed record SyncSummary(
    ulong Add,
    ulong Modify,
    ulong MetadataOnly,
    ulong Remove,
    ulong Unchanged,
    ulong Skipped,
    ulong SkippedForSpaceTracks,
    ulong SkippedForSpaceBytes,
    ulong ArtworkFailedSources);

public sealed record HistoryEntry(
    string Timestamp,
    ulong DurationSecs,
    string Trigger,
    string Outcome,
    string? ErrorMessage,
    SyncSummary? Summary,
    string? Serial = null,
    bool DbRestored = false);

public sealed record StatusUpdateEvent(
    string State,
    bool Configured,
    bool IpodConnected,
    HistoryEntry? LastSync,
    long? NextScheduledUnixSecs,
    StorageInfo? Storage,
    int SyncedCount,
    int? LibraryCount,
    string? AcknowledgedRequestId) : DaemonEvent;

public sealed record HistoryUpdateEvent(
    IReadOnlyList<HistoryEntry> Entries,
    string AcknowledgedRequestId) : DaemonEvent;

public sealed record DaemonSettings(
    bool Enabled,
    bool AutostartWithWindows,
    string FirstSyncMode,
    string SubsequentSyncMode,
    uint ScheduleMinutes,
    string NotifyOn,
    bool RockboxCompat,
    DropSyncBehavior DropSyncBehavior = DropSyncBehavior.Immediate);

public sealed record IpodIdentity(
    string Serial,
    string ModelLabel,
    string? Name,
    bool CustomSelection);

public sealed record ConfigUpdateEvent(
    string? Source,
    DaemonSettings? Daemon,
    IpodIdentity? Ipod,
    ulong ConfigRevision,
    string? AcknowledgedRequestId = null) : DaemonEvent;

public sealed record DeviceConnectedEvent(
    string Serial,
    string ModelLabel,
    string Drive,
    string? Name = null) : DaemonEvent;

public sealed record DeviceDisconnectedEvent(string Serial) : DaemonEvent;

public sealed record SyncRejectedEvent(
    string Reason,
    string Serial,
    string AcknowledgedRequestId) : DaemonEvent;

public sealed record SyncEventEnvelope(
    string Line,
    string? Serial,
    ulong SessionId) : DaemonEvent;

public sealed record DeviceInventorySnapshotEvent(
    ulong Revision,
    IReadOnlyList<DeviceSnapshot> Devices) : DaemonEvent;

public sealed record DeviceIdentitySnapshot(string Serial, string ModelLabel, string? Name = null);

public sealed record DeviceSnapshot(
    DeviceIdentitySnapshot Identity,
    bool Configured,
    bool Connected,
    string? Mount,
    string Phase,
    ulong? SessionId,
    StorageInfo? Storage,
    int SyncedCount,
    int? LibraryCount,
    HistoryEntry? LatestSuccessfulSync,
    HistoryEntry? LatestAttempt,
    string? LastTerminalError,
    ulong SelectionRevision,
    ulong SettingsRevision,
    ulong SubscriptionsRevision);

public sealed record SyncEventContext(ulong SessionId, string? Serial)
{
    public bool IsDeviceSession => !string.IsNullOrWhiteSpace(Serial);
}

public sealed record CancelSyncCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record TriggerSyncCommand(
    [property: JsonPropertyName("source")] string Source,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record PauseCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record DecidePromptCommand(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("choice")] int Choice,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : DaemonCommand;

public sealed record ShutdownCommand : DaemonCommand;
