using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(GetGlobalConfigCommand), "get_global_config")]
[JsonDerivedType(typeof(SetSourceLocationCommand), "set_source_location")]
[JsonDerivedType(typeof(SetGlobalSettingsCommand), "set_global_settings")]
[JsonDerivedType(typeof(GetInventoryCommand), "get_inventory")]
[JsonDerivedType(typeof(SubscribeInventoryCommand), "subscribe_inventory")]
[JsonDerivedType(typeof(UnsubscribeInventoryCommand), "unsubscribe_inventory")]
[JsonDerivedType(typeof(AdoptDeviceCommand), "adopt_device")]
[JsonDerivedType(typeof(ForgetDeviceCommand), "forget_device")]
[JsonDerivedType(typeof(GetDeviceConfigCommand), "get_device_config")]
[JsonDerivedType(typeof(SetSelectionCommand), "set_selection")]
[JsonDerivedType(typeof(SetSettingsCommand), "set_settings")]
[JsonDerivedType(typeof(SetSubscriptionsCommand), "set_subscriptions")]
[JsonDerivedType(typeof(WireTriggerSyncCommand), "trigger_sync")]
[JsonDerivedType(typeof(BackfillRockboxCommand), "backfill_rockbox")]
[JsonDerivedType(typeof(ReplaceLibraryCommand), "replace_library")]
[JsonDerivedType(typeof(WireGetHistoryCommand), "get_history")]
[JsonDerivedType(typeof(GetLibraryCommand), "get_library")]
[JsonDerivedType(typeof(ScanLibraryCommand), "scan_library")]
[JsonDerivedType(typeof(WireRetrySourceMountCommand), "retry_source_mount")]
[JsonDerivedType(typeof(PreviewSelectionCommand), "preview_selection")]
[JsonDerivedType(typeof(PreviewDeviceCommand), "preview_device")]
[JsonDerivedType(typeof(ResolveTracksCommand), "resolve_tracks")]
[JsonDerivedType(typeof(AddSelectionToDeviceCommand), "add_selection_to_device")]
[JsonDerivedType(typeof(ListPlaylistsCommand), "list_playlists")]
[JsonDerivedType(typeof(GetPlaylistCommand), "get_playlist")]
[JsonDerivedType(typeof(SavePlaylistCommand), "save_playlist")]
[JsonDerivedType(typeof(DeletePlaylistCommand), "delete_playlist")]
[JsonDerivedType(typeof(AppendSelectionToPlaylistCommand), "append_selection_to_playlist")]
[JsonDerivedType(typeof(WireShutdownCommand), "shutdown")]
[JsonDerivedType(typeof(ApplyReviewCommand), "apply_review")]
[JsonDerivedType(typeof(DryRunReviewCommand), "dry_run_review")]
[JsonDerivedType(typeof(QuitReviewCommand), "quit_review")]
[JsonDerivedType(typeof(PromptDecisionCommand), "prompt_decision")]
[JsonDerivedType(typeof(FormDecisionCommand), "form_decision")]
[JsonDerivedType(typeof(WireCancelSyncCommand), "cancel_sync")]
[JsonDerivedType(typeof(PauseSyncCommand), "pause_sync")]
public abstract record WireCommand : WireMessage;

public sealed record GetGlobalConfigCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record SetSourceLocationCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("source_root")] string? SourceRoot) : WireCommand;

public sealed record SetGlobalSettingsCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("settings")] GlobalSettings Settings) : WireCommand;

public sealed record GetInventoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record SubscribeInventoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record UnsubscribeInventoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record AdoptDeviceCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("selection_mutation_id")] string SelectionMutationId,
    [property: JsonRequired, JsonPropertyName("selection")] SelectionValue Selection,
    [property: JsonRequired, JsonPropertyName("settings_mutation_id")] string SettingsMutationId,
    [property: JsonRequired, JsonPropertyName("settings")] SettingsValue Settings,
    [property: JsonRequired, JsonPropertyName("subscriptions_mutation_id")] string SubscriptionsMutationId,
    [property: JsonRequired, JsonPropertyName("subscriptions")] SubscriptionsValue Subscriptions) : WireCommand;

public sealed record ForgetDeviceCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record GetDeviceConfigCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record SetSelectionCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("selection")] SelectionValue Selection) : WireCommand;

public sealed record SetSettingsCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("settings")] SettingsValue Settings) : WireCommand;

public sealed record SetSubscriptionsCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("subscriptions")] SubscriptionsValue Subscriptions) : WireCommand;

public sealed record WireTriggerSyncCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("trigger")] SyncTrigger Trigger) : WireCommand;

public sealed record BackfillRockboxCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record ReplaceLibraryCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record WireGetHistoryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("limit")] uint Limit) : WireCommand;

public sealed record GetLibraryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record ScanLibraryCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record WireRetrySourceMountCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("allow_ui")] bool AllowUi) : WireCommand;

public sealed record PreviewSelectionCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("selection")] SelectionValue Selection) : WireCommand;

public sealed record PreviewDeviceCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record ResolveTracksCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules) : WireCommand;

public sealed record AddSelectionToDeviceCommand(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules) : WireCommand;

public sealed record ListPlaylistsCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;

public sealed record GetPlaylistCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("slug")] string Slug) : WireCommand;

public sealed record SavePlaylistCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("playlist")] Playlist Playlist) : WireCommand;

public sealed record DeletePlaylistCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("slug")] string Slug) : WireCommand;

public sealed record AppendSelectionToPlaylistCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules) : WireCommand;

public sealed record WireShutdownCommand(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireCommand;
