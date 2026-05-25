using System.Text.Json.Serialization;

namespace IpodSync_UI.Ipc;

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
[JsonDerivedType(typeof(DecidePromptCommand), "decide_prompt")]
[JsonDerivedType(typeof(ShutdownCommand), "shutdown")]
public abstract record DaemonCommand;

public sealed record GetStatusCommand : DaemonCommand;
public sealed record GetConfigCommand : DaemonCommand;

public sealed record SaveConfigCommand(
    [property: JsonPropertyName("source")] string? Source = null,
    [property: JsonPropertyName("daemon")] DaemonSettings? Daemon = null,
    [property: JsonPropertyName("ipod")] IpodIdentity? Ipod = null
) : DaemonCommand;

/// <summary>Clear the persisted iPod identity. SaveConfig can't
/// express this because its <c>Ipod</c> field treats null as "leave
/// alone".</summary>
public sealed record ForgetIpodCommand : DaemonCommand;

public sealed record TriggerSyncCommand(
    [property: JsonPropertyName("source")] string Source  // "manual" | "scheduled" | "plug_in"
) : DaemonCommand;

public sealed record GetHistoryCommand(
    [property: JsonPropertyName("limit")] int Limit = 10
) : DaemonCommand;

public sealed record SubscribeDeviceEventsCommand : DaemonCommand;
public sealed record UnsubscribeDeviceEventsCommand : DaemonCommand;
public sealed record CancelSyncCommand : DaemonCommand;

/// <summary>
/// Reply to a <see cref="PromptEvent"/> the daemon ferried from the
/// running sync subprocess. The daemon forwards <c>{"type":
/// "prompt_decision","id":Id,"choice":Choice}</c> to the subprocess
/// stdin so the apply loop's <c>await_prompt</c> returns and the
/// sync proceeds. Without this command the popover would have no way
/// to answer daemon-relayed prompts (source-change safeguard,
/// per-track retry/skip/abort, etc.) and the sync would block
/// indefinitely.
/// </summary>
public sealed record DecidePromptCommand(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("choice")] int Choice
) : DaemonCommand;

public sealed record ShutdownCommand : DaemonCommand;
