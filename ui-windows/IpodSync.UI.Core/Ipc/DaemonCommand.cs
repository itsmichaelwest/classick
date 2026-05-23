using System.Text.Json.Serialization;

namespace IpodSync_UI.Ipc;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(GetStatusCommand), "get_status")]
[JsonDerivedType(typeof(GetConfigCommand), "get_config")]
[JsonDerivedType(typeof(SaveConfigCommand), "save_config")]
[JsonDerivedType(typeof(TriggerSyncCommand), "trigger_sync")]
[JsonDerivedType(typeof(GetHistoryCommand), "get_history")]
[JsonDerivedType(typeof(SubscribeDeviceEventsCommand), "subscribe_device_events")]
[JsonDerivedType(typeof(UnsubscribeDeviceEventsCommand), "unsubscribe_device_events")]
[JsonDerivedType(typeof(ShutdownCommand), "shutdown")]
public abstract record DaemonCommand;

public sealed record GetStatusCommand : DaemonCommand;
public sealed record GetConfigCommand : DaemonCommand;

public sealed record SaveConfigCommand(
    [property: JsonPropertyName("source")] string? Source = null,
    [property: JsonPropertyName("daemon")] DaemonSettings? Daemon = null,
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod = null
) : DaemonCommand;

public sealed record TriggerSyncCommand(
    [property: JsonPropertyName("source")] string Source  // "manual" | "scheduled" | "plug_in"
) : DaemonCommand;

public sealed record GetHistoryCommand(
    [property: JsonPropertyName("limit")] int Limit = 10
) : DaemonCommand;

public sealed record SubscribeDeviceEventsCommand : DaemonCommand;
public sealed record UnsubscribeDeviceEventsCommand : DaemonCommand;
public sealed record ShutdownCommand : DaemonCommand;
