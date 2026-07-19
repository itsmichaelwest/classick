using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

/// <summary>
/// Base type for events emitted by the Rust core on stdout in --ipc-mode.
/// Wire format: newline-delimited JSON, snake_case "type" discriminator.
/// See docs/ipc/subprocess.md for the authoritative schema.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(HelloEvent), "hello")]
[JsonDerivedType(typeof(HeaderEvent), "header")]
[JsonDerivedType(typeof(SummaryEvent), "summary")]
[JsonDerivedType(typeof(ReviewEvent), "review")]
[JsonDerivedType(typeof(PromptEvent), "prompt")]
[JsonDerivedType(typeof(FormEvent), "form")]
[JsonDerivedType(typeof(TrackStartEvent), "track_start")]
[JsonDerivedType(typeof(TrackDoneEvent), "track_done")]
[JsonDerivedType(typeof(FinalizingEvent), "finalizing")]
[JsonDerivedType(typeof(CancelledEvent), "cancelled")]
[JsonDerivedType(typeof(LogEvent), "log")]
[JsonDerivedType(typeof(ErrorEvent), "error")]
[JsonDerivedType(typeof(FinishEvent), "finish")]
[JsonDerivedType(typeof(PausedEvent), "paused")]
public abstract record IpcEvent;

/// <summary>Handshake; first event emitted after spawn. See §4.1.</summary>
public sealed record HelloEvent(
    [property: JsonPropertyName("protocol_version")] string ProtocolVersion,
    [property: JsonPropertyName("core_version")] string CoreVersion
) : IpcEvent;

/// <summary>Resolved paths for display. See §4.2.</summary>
public sealed record HeaderEvent(
    [property: JsonPropertyName("source")] string Source,
    [property: JsonPropertyName("ipod")] string Ipod,
    [property: JsonPropertyName("manifest")] string Manifest
) : IpcEvent;

/// <summary>Action plan counts. See §4.3.</summary>
public sealed record SummaryEvent(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("metadata_only")] int MetadataOnly,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged,
    [property: JsonPropertyName("total_planned")] int TotalPlanned
) : IpcEvent;

/// <summary>Nested action-plan summary used by <see cref="ReviewEvent"/>.</summary>
public sealed record ActionPlanSummary(
    [property: JsonPropertyName("add")] int Add,
    [property: JsonPropertyName("modify")] int Modify,
    [property: JsonPropertyName("metadata_only")] int MetadataOnly,
    [property: JsonPropertyName("remove")] int Remove,
    [property: JsonPropertyName("unchanged")] int Unchanged
);

/// <summary>Request a review decision from the user. See §4.4.</summary>
public sealed record ReviewEvent(
    [property: JsonPropertyName("summary")] ActionPlanSummary Summary,
    [property: JsonPropertyName("no_delete")] bool NoDelete
) : IpcEvent;

/// <summary>Modal multi-choice prompt. See §4.5.</summary>
public sealed record PromptEvent(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("message")] string Message,
    [property: JsonPropertyName("options")] IReadOnlyList<string> Options
) : IpcEvent;

/// <summary>Modal text-input prompt. See §4.6.</summary>
public sealed record FormEvent(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("label")] string Label,
    [property: JsonPropertyName("initial")] string Initial,
    [property: JsonPropertyName("hint")] string Hint
) : IpcEvent;

/// <summary>Per-track operation begin. See §4.7.</summary>
public sealed record TrackStartEvent(
    [property: JsonPropertyName("current")] int Current,
    [property: JsonPropertyName("total")] int Total,
    [property: JsonPropertyName("label")] string Label,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("eta_secs")] ulong? EtaSecs = null
) : IpcEvent;

/// <summary>Per-track operation end. See §4.8.</summary>
public sealed record TrackDoneEvent(
    [property: JsonRequired, JsonPropertyName("result")] TrackResult Result
) : IpcEvent;

[JsonConverter(typeof(JsonStringEnumConverter<TrackResult>))]
public enum TrackResult
{
    [JsonStringEnumMemberName("applied")]
    Applied,
    [JsonStringEnumMemberName("skipped")]
    Skipped,
}

[JsonConverter(typeof(JsonStringEnumConverter<FinalizationReason>))]
public enum FinalizationReason
{
    [JsonStringEnumMemberName("cancelled")]
    Cancelled,
    [JsonStringEnumMemberName("paused")]
    Paused,
}

public sealed record FinalizingEvent(
    [property: JsonRequired, JsonPropertyName("reason")] FinalizationReason Reason,
    [property: JsonRequired, JsonPropertyName("staged_albums")] int StagedAlbums,
    [property: JsonRequired, JsonPropertyName("staged_tracks")] int StagedTracks
) : IpcEvent;

public sealed record CancelledEvent : IpcEvent;

/// <summary>Graceful sync checkpoint reached after a pause request.</summary>
public sealed record PausedEvent : IpcEvent;

/// <summary>Informational log line. See §4.9.</summary>
public sealed record LogEvent(
    [property: JsonPropertyName("message")] string Message
) : IpcEvent;

/// <summary>Non-fatal or fatal error. See §4.10.</summary>
public sealed record ErrorEvent(
    [property: JsonPropertyName("message")] string Message,
    [property: JsonPropertyName("recovery_hints")] IReadOnlyList<string>? RecoveryHints = null
) : IpcEvent;

/// <summary>Final event of a run. See §4.11.</summary>
public sealed record FinishEvent(
    [property: JsonRequired, JsonPropertyName("success")] bool Success,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("skipped_for_space")] SkippedForSpaceSummary? SkippedForSpace = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("artwork")] ArtworkSummary? Artwork = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("db_restored")] bool DbRestored = false
) : IpcEvent;

public sealed record SkippedForSpaceSummary(
    [property: JsonRequired, JsonPropertyName("albums")] int Albums,
    [property: JsonRequired, JsonPropertyName("tracks")] int Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes
);

public sealed record ArtworkSummary(
    [property: JsonRequired, JsonPropertyName("embedded")] int Embedded,
    [property: JsonRequired, JsonPropertyName("eligible")] int Eligible,
    [property: JsonRequired, JsonPropertyName("failed_sources")] int FailedSources
);
