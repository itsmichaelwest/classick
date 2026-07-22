using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

public sealed record ApplyReviewCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("no_delete")] bool NoDelete) : WireCommand, ISessionRoutedMessage;

public sealed record DryRunReviewCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand, ISessionRoutedMessage;

public sealed record QuitReviewCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand, ISessionRoutedMessage;

public sealed record PromptDecisionCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("prompt_id")] ulong PromptId,
    [property: JsonRequired, JsonPropertyName("choice")] uint Choice) : WireCommand, ISessionRoutedMessage;

public sealed record FormDecisionCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("prompt_id")] ulong PromptId,
    [property: JsonRequired, JsonPropertyName("value")] string? Value) : WireCommand, ISessionRoutedMessage;

public sealed record WireCancelSyncCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand, ISessionRoutedMessage;

public sealed record PauseSyncCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand, ISessionRoutedMessage;
