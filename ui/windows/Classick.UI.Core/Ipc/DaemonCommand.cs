using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

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
[JsonDerivedType(typeof(BackfillRockboxCommand), "backfill_rockbox")]
[JsonDerivedType(typeof(ReplaceLibraryCommand), "replace_library")]
[JsonDerivedType(typeof(GetLibraryCommand), "get_library")]
[JsonDerivedType(typeof(ScanLibraryCommand), "scan_library")]
[JsonDerivedType(typeof(RetrySourceMountCommand), "retry_source_mount")]
[JsonDerivedType(typeof(PreviewSelectionCommand), "preview_selection")]
[JsonDerivedType(typeof(ListPlaylistsCommand), "list_playlists")]
[JsonDerivedType(typeof(GetPlaylistCommand), "get_playlist")]
[JsonDerivedType(typeof(SavePlaylistCommand), "save_playlist")]
[JsonDerivedType(typeof(DeletePlaylistCommand), "delete_playlist")]
[JsonDerivedType(typeof(GetDeviceConfigCommand), "get_device_config")]
[JsonDerivedType(typeof(SaveDeviceConfigCommand), "save_device_config")]
[JsonDerivedType(typeof(PreviewDeviceCommand), "preview_device")]
[JsonDerivedType(typeof(ResolveTracksCommand), "resolve_tracks")]
[JsonDerivedType(typeof(ShutdownCommand), "shutdown")]
public abstract record DaemonCommand;

public sealed record GetStatusCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record GetConfigCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record SaveConfigCommand(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("source")] string? Source,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("daemon")] DaemonSettings? Daemon,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("ipod")] IpodIdentity? Ipod,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

/// <summary>Clear the persisted iPod identity. SaveConfig can't
/// express this because its <c>Ipod</c> field treats null as "leave
/// alone".</summary>
public sealed record ForgetIpodCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record TriggerSyncCommand(
    [property: JsonPropertyName("source")] string Source,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record GetHistoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("limit")] int? Limit = null
) : DaemonCommand;

public sealed record SubscribeDeviceEventsCommand : DaemonCommand;
public sealed record UnsubscribeDeviceEventsCommand : DaemonCommand;
public sealed record CancelSyncCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record PauseCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

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
    [property: JsonPropertyName("choice")] int Choice,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record BackfillRockboxCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record ReplaceLibraryCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record GetLibraryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record ScanLibraryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record RetrySourceMountCommand(
    [property: JsonRequired, JsonPropertyName("allow_ui")] bool AllowUi,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record PreviewSelectionCommand(
    [property: JsonPropertyName("mode")] SelectionMode Mode,
    [property: JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules,
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record ListPlaylistsCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record GetPlaylistCommand(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record SavePlaylistCommand(
    [property: JsonRequired, JsonPropertyName("playlist")] PlaylistPayload Playlist,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record DeletePlaylistCommand(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record GetDeviceConfigCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record SaveDeviceConfigCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("selection")] SelectionState? Selection,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("subscriptions")] Subscriptions? Subscriptions,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("settings")] DeviceSettings? Settings,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record PreviewDeviceCommand(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record ResolveTracksCommand(
    [property: JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId
) : DaemonCommand;

public sealed record ShutdownCommand : DaemonCommand;
