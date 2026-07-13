using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

// TODO(windows): pause/resume + X-of-Y not yet wired on Windows.
// Daemon protocol bumped to 1.2.0 with a new `DaemonCommand::Pause`
// ({"type":"pause"}, no payload, no-op if idle) that forwards a graceful
// pause to the running sync subprocess — see docs/ipc-protocol.md "Daemon
// v1.2.0". This DaemonCommand hierarchy has no PauseCommand yet. There is
// no separate resume command by design: resuming is an ordinary
// TriggerSyncCommand (the sync is diff-based and continues from the last
// checkpoint). Mirror the Rust daemon
// (crates/classick/src/ipc_daemon.rs::DaemonCommand::Pause) and the macOS
// client (ui/macos/Sources/Classick/Ipc/WireModels.swift,
// `DaemonCommand.pause`) plus a Pause/Resume affordance in the tray UI.
// Can't build/verify Windows in this environment — see docs/ipc-protocol.md.

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
