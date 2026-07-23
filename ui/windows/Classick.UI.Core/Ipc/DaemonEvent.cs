using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(GlobalConfigEvent), "global_config")]
[JsonDerivedType(typeof(WireSourceAvailabilityEvent), "source_availability")]
[JsonDerivedType(typeof(DeviceInventoryEvent), "device_inventory")]
[JsonDerivedType(typeof(InventorySubscriptionChangedEvent), "inventory_subscription_changed")]
[JsonDerivedType(typeof(DeviceConfigEvent), "device_config")]
[JsonDerivedType(typeof(ConfigMutationFailedEvent), "config_mutation_failed")]
[JsonDerivedType(typeof(DeviceForgottenEvent), "device_forgotten")]
[JsonDerivedType(typeof(SyncAcceptedEvent), "sync_accepted")]
[JsonDerivedType(typeof(WireSyncRejectedEvent), "sync_rejected")]
[JsonDerivedType(typeof(HistoryEvent), "history")]
[JsonDerivedType(typeof(LibraryEvent), "library")]
[JsonDerivedType(typeof(LibraryScanStartedEvent), "library_scan_started")]
[JsonDerivedType(typeof(LibraryScanProgressEvent), "library_scan_progress")]
[JsonDerivedType(typeof(LibraryScanFinishedEvent), "library_scan_finished")]
[JsonDerivedType(typeof(SelectionPreviewEvent), "selection_preview")]
[JsonDerivedType(typeof(DevicePreviewEvent), "device_preview")]
[JsonDerivedType(typeof(ResolvedTracksEvent), "resolved_tracks")]
[JsonDerivedType(typeof(PlaylistsEvent), "playlists")]
[JsonDerivedType(typeof(PlaylistDetailEvent), "playlist_detail")]
[JsonDerivedType(typeof(PlaylistSavedEvent), "playlist_saved")]
[JsonDerivedType(typeof(DeviceSelectionAddedEvent), "device_selection_added")]
[JsonDerivedType(typeof(PlaylistSelectionAppendedEvent), "playlist_selection_appended")]
[JsonDerivedType(typeof(LibraryMutationRejectedEvent), "library_mutation_rejected")]
[JsonDerivedType(typeof(DaemonShutdownStartedEvent), "daemon_shutdown_started")]
[JsonDerivedType(typeof(RunHeaderEvent), "run_header")]
[JsonDerivedType(typeof(SyncSummaryEvent), "sync_summary")]
[JsonDerivedType(typeof(ReviewRequestedEvent), "review_requested")]
[JsonDerivedType(typeof(WirePromptEvent), "prompt")]
[JsonDerivedType(typeof(WireFormEvent), "form")]
[JsonDerivedType(typeof(WireTrackStartEvent), "track_start")]
[JsonDerivedType(typeof(WireTrackDoneEvent), "track_done")]
[JsonDerivedType(typeof(WireFinalizingEvent), "finalizing")]
[JsonDerivedType(typeof(SyncCancelledEvent), "sync_cancelled")]
[JsonDerivedType(typeof(SyncPausedEvent), "sync_paused")]
[JsonDerivedType(typeof(SyncLogEvent), "sync_log")]
[JsonDerivedType(typeof(SyncErrorEvent), "sync_error")]
[JsonDerivedType(typeof(SyncFinishedEvent), "sync_finished")]
[JsonDerivedType(typeof(CommandFailedEvent), "command_failed")]
public abstract record WireEvent : WireMessage;

public sealed record GlobalConfigEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("source_root")] string? SourceRoot,
    [property: JsonRequired, JsonPropertyName("settings")] GlobalSettings Settings) : WireEvent;

public sealed record WireSourceAvailabilityEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("state")] SourceAvailabilityState State,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("source_root")] string? SourceRoot) : WireEvent;

public sealed record DeviceInventoryEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("devices")] IReadOnlyList<IdentifiedDeviceSnapshot> Devices,
    [property: JsonRequired, JsonPropertyName("unidentified")] IReadOnlyList<UnidentifiedDeviceSnapshot> Unidentified) : WireEvent;

public sealed record InventorySubscriptionChangedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("subscribed")] bool Subscribed) : WireEvent;

public sealed record DeviceConfigEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("selection")] DeliveredComponent<SelectionValue> Selection,
    [property: JsonRequired, JsonPropertyName("settings")] DeliveredComponent<SettingsValue> Settings,
    [property: JsonRequired, JsonPropertyName("subscriptions")] DeliveredComponent<SubscriptionsValue> Subscriptions) : WireEvent;

[JsonConverter(typeof(StrictStringEnumConverter<ConfigComponent>))]
public enum ConfigComponent
{
    [JsonStringEnumMemberName("selection")] Selection,
    [JsonStringEnumMemberName("settings")] Settings,
    [JsonStringEnumMemberName("subscriptions")] Subscriptions,
}

[JsonConverter(typeof(StrictStringEnumConverter<ConfigFailureStage>))]
public enum ConfigFailureStage
{
    [JsonStringEnumMemberName("host_acceptance")] HostAcceptance,
    [JsonStringEnumMemberName("device_delivery")] DeviceDelivery,
}

public sealed record ConfigMutationFailedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("component")] ConfigComponent Component,
    [property: JsonRequired, JsonPropertyName("stage")] ConfigFailureStage Stage,
    [property: JsonRequired, JsonPropertyName("message")] string Message) : WireEvent;

public sealed record DeviceForgottenEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireEvent;

public sealed record SyncAcceptedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("operation")] SyncOperation Operation) : WireEvent, ISessionRoutedMessage;

public sealed record WireSyncRejectedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("operation")] SyncOperation Operation,
    [property: JsonRequired, JsonPropertyName("reason")] SyncRejectReason Reason,
    [property: JsonRequired, JsonPropertyName("message")] string Message) : WireEvent;

public sealed record HistoryEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("entries")] IReadOnlyList<WireHistoryEntry> Entries) : WireEvent;

public sealed record LibraryEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("source_root")] string? SourceRoot,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("scanned_at_unix_secs")] ulong? ScannedAtUnixSecs,
    [property: JsonRequired, JsonPropertyName("artists")] IReadOnlyList<LibraryArtist> Artists,
    [property: JsonRequired, JsonPropertyName("genres")] IReadOnlyList<LibraryGenre> Genres,
    [property: JsonRequired, JsonPropertyName("total_tracks")] ulong TotalTracks,
    [property: JsonRequired, JsonPropertyName("total_bytes")] ulong TotalBytes) : WireEvent;

public sealed record LibraryScanStartedEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId) : WireEvent;

public sealed record LibraryScanProgressEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("files_scanned")] ulong FilesScanned,
    [property: JsonRequired, JsonPropertyName("tracks_indexed")] ulong TracksIndexed) : WireEvent;

public sealed record LibraryScanFinishedEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("session_id")] ulong SessionId,
    [property: JsonRequired, JsonPropertyName("success")] bool Success,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("message")] string? Message = null) : WireEvent;

public sealed record SelectionPreviewEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("selected_tracks")] ulong SelectedTracks,
    [property: JsonRequired, JsonPropertyName("selected_bytes")] ulong SelectedBytes,
    [property: JsonRequired, JsonPropertyName("adds")] ulong Adds,
    [property: JsonRequired, JsonPropertyName("removes")] ulong Removes) : WireEvent;

public sealed record DevicePreviewEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("selected_tracks")] ulong SelectedTracks,
    [property: JsonRequired, JsonPropertyName("selected_bytes")] ulong SelectedBytes,
    [property: JsonRequired, JsonPropertyName("playlist_extra_tracks")] ulong PlaylistExtraTracks,
    [property: JsonRequired, JsonPropertyName("playlist_extra_bytes")] ulong PlaylistExtraBytes,
    [property: JsonRequired, JsonPropertyName("projected_free_bytes")] ulong? ProjectedFreeBytes,
    [property: JsonRequired, JsonPropertyName("unresolved_subscriptions")] IReadOnlyList<string> UnresolvedSubscriptions) : WireEvent;

public sealed record ResolvedTracksEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("tracks")] IReadOnlyList<string> Tracks) : WireEvent;

public sealed record PlaylistsEvent(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("request_id")] string? RequestId,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("playlists")] IReadOnlyList<PlaylistSummary> Playlists) : WireEvent;

public sealed record PlaylistDetailEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("result")] PlaylistDetailResult Result) : WireEvent;

public sealed record PlaylistSavedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("playlist")] Playlist Playlist) : WireEvent;

public sealed record DeviceSelectionAddedEvent(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("matched_tracks")] ulong MatchedTracks,
    [property: JsonRequired, JsonPropertyName("missing_tracks")] ulong MissingTracks,
    [property: JsonRequired, JsonPropertyName("selection_changed")] bool SelectionChanged,
    [property: JsonRequired, JsonPropertyName("selection_revision")] ulong SelectionRevision,
    [property: JsonRequired, JsonPropertyName("selection")] SelectionValue Selection,
    [property: JsonRequired, JsonPropertyName("delivery")] ConfigDelivery Delivery,
    [property: JsonRequired, JsonPropertyName("sync")] DropSyncDisposition Sync) : WireEvent;

public sealed record PlaylistSelectionAppendedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("appended_tracks")] ulong AppendedTracks,
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("playlist")] Playlist Playlist) : WireEvent;

public sealed record LibraryMutationRejectedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("target")] LibraryMutationTarget Target,
    [property: JsonRequired, JsonPropertyName("code")] string Code,
    [property: JsonRequired, JsonPropertyName("message")] string Message) : WireEvent;

public sealed record DaemonShutdownStartedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId) : WireEvent;

public sealed record CommandFailedEvent(
    [property: JsonRequired, JsonPropertyName("request_id")] string RequestId,
    [property: JsonRequired, JsonPropertyName("message")] string Message) : WireEvent;
