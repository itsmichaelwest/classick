using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

/// <summary>
/// Base type for events emitted by the Rust core on stdout in --ipc-mode.
/// Wire format: newline-delimited JSON, snake_case "type" discriminator.
/// See docs/ipc-protocol.md §4 for the authoritative schema.
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
[JsonDerivedType(typeof(LogEvent), "log")]
[JsonDerivedType(typeof(ErrorEvent), "error")]
[JsonDerivedType(typeof(FinishEvent), "finish")]
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
    [property: JsonPropertyName("label")] string Label
) : IpcEvent;

/// <summary>Per-track operation end. See §4.8.</summary>
public sealed record TrackDoneEvent : IpcEvent;

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
    [property: JsonPropertyName("success")] bool Success
) : IpcEvent;
