using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

public abstract record WireMessage;

public sealed record WireHello : WireMessage
{
    [JsonPropertyOrder(-100), JsonPropertyName("type")]
    public string Type => "hello";

    [JsonRequired, JsonPropertyName("protocol_version")]
    public required string ProtocolVersion { get; init; }

    [JsonRequired, JsonPropertyName("role")]
    public required EndpointRole Role { get; init; }

    [JsonRequired, JsonPropertyName("software_version")]
    public required string SoftwareVersion { get; init; }

    [JsonRequired, JsonPropertyName("capabilities")]
    public required IReadOnlyList<string> Capabilities { get; init; }
}

public interface ISessionRoutedMessage
{
    DeviceId DeviceId { get; }
    ulong SessionId { get; }
}

public sealed record WireActionPlanSummary(
    [property: JsonRequired, JsonPropertyName("add")] ulong Add,
    [property: JsonRequired, JsonPropertyName("modify")] ulong Modify,
    [property: JsonRequired, JsonPropertyName("metadata_only")] ulong MetadataOnly,
    [property: JsonRequired, JsonPropertyName("remove")] ulong Remove,
    [property: JsonRequired, JsonPropertyName("unchanged")] ulong Unchanged,
    [property: JsonRequired, JsonPropertyName("total_planned")] ulong TotalPlanned);

[JsonConverter(typeof(StrictStringEnumConverter<TrackResult>))]
public enum TrackResult
{
    [JsonStringEnumMemberName("applied")] Applied,
    [JsonStringEnumMemberName("skipped")] Skipped,
}

[JsonConverter(typeof(StrictStringEnumConverter<StopReason>))]
public enum StopReason
{
    [JsonStringEnumMemberName("cancelled")] Cancelled,
    [JsonStringEnumMemberName("paused")] Paused,
}

public sealed record SkippedForSpaceSummary(
    [property: JsonRequired, JsonPropertyName("albums")] ulong Albums,
    [property: JsonRequired, JsonPropertyName("tracks")] ulong Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes);

public sealed record ArtworkSummary(
    [property: JsonRequired, JsonPropertyName("embedded")] ulong Embedded,
    [property: JsonRequired, JsonPropertyName("eligible")] ulong Eligible,
    [property: JsonRequired, JsonPropertyName("failed_sources")] ulong FailedSources);

public sealed record RunHeaderEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("source")] string Source,
    [property: JsonRequired, JsonPropertyName("ipod")] string Ipod,
    [property: JsonRequired, JsonPropertyName("manifest")] string Manifest) : WireEvent, ISessionRoutedMessage;

public sealed record SyncSummaryEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("summary")] WireActionPlanSummary Summary) : WireEvent, ISessionRoutedMessage;

public sealed record ReviewRequestedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("summary")] WireActionPlanSummary Summary,
    [property: JsonRequired, JsonPropertyName("no_delete")] bool NoDelete) : WireEvent, ISessionRoutedMessage;

public sealed record WirePromptEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("prompt_id")] ulong PromptId,
    [property: JsonRequired, JsonPropertyName("message")] string Message,
    [property: JsonRequired, JsonPropertyName("options")] IReadOnlyList<string> Options) : WireEvent, ISessionRoutedMessage;

public sealed record WireFormEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("prompt_id")] ulong PromptId,
    [property: JsonRequired, JsonPropertyName("label")] string Label,
    [property: JsonRequired, JsonPropertyName("initial")] string Initial,
    [property: JsonRequired, JsonPropertyName("hint")] string Hint) : WireEvent, ISessionRoutedMessage;

public sealed record WireTrackStartEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("current")] ulong Current,
    [property: JsonRequired, JsonPropertyName("total")] ulong Total,
    [property: JsonRequired, JsonPropertyName("label")] string Label,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("eta_secs")] ulong? EtaSecs = null) : WireEvent, ISessionRoutedMessage;

public sealed record WireTrackDoneEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("result")] TrackResult Result) : WireEvent, ISessionRoutedMessage;

public sealed record WireFinalizingEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("reason")] StopReason Reason,
    [property: JsonRequired, JsonPropertyName("staged_albums")] ulong StagedAlbums,
    [property: JsonRequired, JsonPropertyName("staged_tracks")] ulong StagedTracks) : WireEvent, ISessionRoutedMessage;

public sealed record SyncCancelledEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId) : WireEvent, ISessionRoutedMessage;

public sealed record SyncPausedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId) : WireEvent, ISessionRoutedMessage;

public sealed record SyncLogEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("message")] string Message) : WireEvent, ISessionRoutedMessage;

public sealed record SyncErrorEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("message")] string Message,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("recovery_hints")]
    IReadOnlyList<string>? RecoveryHints = null) : WireEvent, ISessionRoutedMessage;

public sealed record SyncFinishedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("success")] bool Success,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("skipped_for_space")]
    SkippedForSpaceSummary? SkippedForSpace = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("artwork")]
    ArtworkSummary? Artwork = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("db_restored")]
    bool DbRestored = false) : WireEvent, ISessionRoutedMessage;
