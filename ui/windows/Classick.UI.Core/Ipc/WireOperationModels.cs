using System.Text.Json;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonConverter(typeof(StrictStringEnumConverter<SyncMode>))]
public enum SyncMode
{
    [JsonStringEnumMemberName("review")] Review,
    [JsonStringEnumMemberName("auto_apply")] AutoApply,
}

[JsonConverter(typeof(StrictStringEnumConverter<NotifyLevel>))]
public enum NotifyLevel
{
    [JsonStringEnumMemberName("all")] All,
    [JsonStringEnumMemberName("errors_only")] ErrorsOnly,
    [JsonStringEnumMemberName("none")] None,
}

[JsonConverter(typeof(StrictStringEnumConverter<DropSyncBehavior>))]
public enum DropSyncBehavior
{
    [JsonStringEnumMemberName("immediate")] Immediate,
    [JsonStringEnumMemberName("next_sync")] NextSync,
}

public sealed record GlobalSettings(
    [property: JsonRequired, JsonPropertyName("first_sync_mode")] SyncMode FirstSyncMode,
    [property: JsonRequired, JsonPropertyName("subsequent_sync_mode")] SyncMode SubsequentSyncMode,
    [property: JsonRequired, JsonPropertyName("schedule_minutes")] uint ScheduleMinutes,
    [property: JsonRequired, JsonPropertyName("notify_on")] NotifyLevel NotifyOn,
    [property: JsonRequired, JsonPropertyName("drop_sync_behavior")] DropSyncBehavior DropSyncBehavior);

[JsonConverter(typeof(StrictStringEnumConverter<SourceAvailabilityState>))]
public enum SourceAvailabilityState
{
    [JsonStringEnumMemberName("available")] Available,
    [JsonStringEnumMemberName("remounting")] Remounting,
    [JsonStringEnumMemberName("auth_required")] AuthRequired,
    [JsonStringEnumMemberName("unavailable")] Unavailable,
}

[JsonConverter(typeof(StrictStringEnumConverter<SyncTrigger>))]
public enum SyncTrigger
{
    [JsonStringEnumMemberName("manual")] Manual,
    [JsonStringEnumMemberName("scheduled")] Scheduled,
    [JsonStringEnumMemberName("plug_in")] PlugIn,
}

[JsonConverter(typeof(StrictStringEnumConverter<HistoryTrigger>))]
public enum HistoryTrigger
{
    [JsonStringEnumMemberName("manual")] Manual,
    [JsonStringEnumMemberName("scheduled")] Scheduled,
    [JsonStringEnumMemberName("plug_in")] PlugIn,
    [JsonStringEnumMemberName("coalesced")] Coalesced,
}

[JsonConverter(typeof(StrictStringEnumConverter<SyncOperation>))]
public enum SyncOperation
{
    [JsonStringEnumMemberName("sync")] Sync,
    [JsonStringEnumMemberName("backfill_rockbox")] BackfillRockbox,
    [JsonStringEnumMemberName("replace_library")] ReplaceLibrary,
}

[JsonConverter(typeof(StrictStringEnumConverter<SyncRejectReason>))]
public enum SyncRejectReason
{
    [JsonStringEnumMemberName("already_running")] AlreadyRunning,
    [JsonStringEnumMemberName("device_disconnected")] DeviceDisconnected,
    [JsonStringEnumMemberName("not_adopted")] NotAdopted,
    [JsonStringEnumMemberName("needs_apple_initialization")] NeedsAppleInitialization,
    [JsonStringEnumMemberName("invalid_database")] InvalidDatabase,
    [JsonStringEnumMemberName("source_unavailable")] SourceUnavailable,
    [JsonStringEnumMemberName("recovery_required")] RecoveryRequired,
}

[JsonConverter(typeof(StrictStringEnumConverter<SyncOutcome>))]
public enum SyncOutcome
{
    [JsonStringEnumMemberName("ok")] Ok,
    [JsonStringEnumMemberName("error")] Error,
    [JsonStringEnumMemberName("aborted")] Aborted,
    [JsonStringEnumMemberName("cancelled")] Cancelled,
}

public sealed record HistorySummary(
    [property: JsonRequired, JsonPropertyName("add")] ulong Add,
    [property: JsonRequired, JsonPropertyName("modify")] ulong Modify,
    [property: JsonRequired, JsonPropertyName("metadata_only")] ulong MetadataOnly,
    [property: JsonRequired, JsonPropertyName("remove")] ulong Remove,
    [property: JsonRequired, JsonPropertyName("unchanged")] ulong Unchanged,
    [property: JsonRequired, JsonPropertyName("skipped")] ulong Skipped,
    [property: JsonRequired, JsonPropertyName("skipped_for_space_tracks")] ulong SkippedForSpaceTracks,
    [property: JsonRequired, JsonPropertyName("skipped_for_space_bytes")] ulong SkippedForSpaceBytes,
    [property: JsonRequired, JsonPropertyName("artwork_failed_sources")] ulong ArtworkFailedSources);

public sealed record WireHistoryEntry(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("session_id")] ulong? SessionId,
    [property: JsonRequired, JsonPropertyName("timestamp")] string Timestamp,
    [property: JsonRequired, JsonPropertyName("duration_secs")] ulong DurationSecs,
    [property: JsonRequired, JsonPropertyName("trigger")] HistoryTrigger Trigger,
    [property: JsonRequired, JsonPropertyName("operation")] SyncOperation Operation,
    [property: JsonRequired, JsonPropertyName("outcome")] SyncOutcome Outcome,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("error_message")] string? ErrorMessage = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("summary")] HistorySummary? Summary = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault), JsonPropertyName("db_restored")] bool DbRestored = false);

public sealed record LibraryAlbum(
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("genre")] string? Genre,
    [property: JsonRequired, JsonPropertyName("tracks")] ulong Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes,
    [property: JsonRequired, JsonPropertyName("duration_ms")] ulong DurationMs = 0);

public sealed record LibraryArtist(
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("albums")] IReadOnlyList<LibraryAlbum> Albums);

public sealed record LibraryGenre(
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("tracks")] ulong Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes,
    [property: JsonRequired, JsonPropertyName("duration_ms")] ulong DurationMs = 0);

[JsonConverter(typeof(StrictStringEnumConverter<PlaylistKind>))]
public enum PlaylistKind
{
    [JsonStringEnumMemberName("manual")] Manual,
    [JsonStringEnumMemberName("smart")] Smart,
}

[JsonConverter(typeof(StrictStringEnumConverter<SmartMatching>))]
public enum SmartMatching
{
    [JsonStringEnumMemberName("all")] All,
    [JsonStringEnumMemberName("any")] Any,
}

[JsonConverter(typeof(StrictStringEnumConverter<SmartField>))]
public enum SmartField
{
    [JsonStringEnumMemberName("artist")] Artist,
    [JsonStringEnumMemberName("album")] Album,
    [JsonStringEnumMemberName("genre")] Genre,
    [JsonStringEnumMemberName("year")] Year,
}

[JsonConverter(typeof(StrictStringEnumConverter<SmartOperation>))]
public enum SmartOperation
{
    [JsonStringEnumMemberName("is")] Is,
    [JsonStringEnumMemberName("contains")] Contains,
    [JsonStringEnumMemberName("gte")] Gte,
    [JsonStringEnumMemberName("lte")] Lte,
}

[JsonConverter(typeof(StrictStringEnumConverter<SmartOrder>))]
public enum SmartOrder
{
    [JsonStringEnumMemberName("recently_modified")] RecentlyModified,
    [JsonStringEnumMemberName("random_stable")] RandomStable,
    [JsonStringEnumMemberName("alpha")] Alpha,
}

public sealed record SmartRule(
    [property: JsonRequired, JsonPropertyName("field")] SmartField Field,
    [property: JsonRequired, JsonPropertyName("op")] SmartOperation Operation,
    [property: JsonRequired, JsonPropertyName("value")] string Value);

[JsonConverter(typeof(SmartLimitJsonConverter))]
public abstract record SmartLimit;

public sealed record TrackSmartLimit(ulong Tracks) : SmartLimit;
public sealed record ByteSmartLimit(ulong Bytes) : SmartLimit;

public sealed class SmartLimitJsonConverter : JsonConverter<SmartLimit>
{
    public override SmartLimit Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var root = document.RootElement;
        if (root.ValueKind != JsonValueKind.Object || root.EnumerateObject().Count() != 1)
        {
            throw new JsonException("smart limit must contain one limit kind");
        }
        if (root.TryGetProperty("tracks", out var tracks)) return new TrackSmartLimit(tracks.GetUInt64());
        if (root.TryGetProperty("bytes", out var bytes)) return new ByteSmartLimit(bytes.GetUInt64());
        throw new JsonException("unknown smart limit kind");
    }

    public override void Write(Utf8JsonWriter writer, SmartLimit value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        switch (value)
        {
            case TrackSmartLimit tracks: writer.WriteNumber("tracks", tracks.Tracks); break;
            case ByteSmartLimit bytes: writer.WriteNumber("bytes", bytes.Bytes); break;
            default: throw new JsonException("unknown smart limit");
        }
        writer.WriteEndObject();
    }
}

public sealed record SmartRules(
    [property: JsonRequired, JsonPropertyName("version")] uint Version,
    [property: JsonRequired, JsonPropertyName("matching")] SmartMatching Matching,
    [property: JsonRequired, JsonPropertyName("rules")] IReadOnlyList<SmartRule> Rules,
    [property: JsonRequired, JsonPropertyName("limit")] SmartLimit? Limit,
    [property: JsonRequired, JsonPropertyName("order")] SmartOrder Order,
    [property: JsonRequired, JsonPropertyName("seed")] ulong Seed);

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(ManualPlaylist), "manual")]
[JsonDerivedType(typeof(SmartPlaylist), "smart")]
public abstract record Playlist;

public sealed record ManualPlaylist(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("slug")] string? Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("tracks")] IReadOnlyList<string> Tracks) : Playlist;

public sealed record SmartPlaylist(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("slug")] string? Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("rules")] SmartRules Rules) : Playlist;

public sealed record PlaylistSummary(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("kind")] PlaylistKind Kind,
    [property: JsonRequired, JsonPropertyName("tracks")] ulong Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("error")] string? Error = null,
    [property: JsonRequired, JsonPropertyName("duration_ms")] ulong DurationMs = 0);

[JsonPolymorphic(TypeDiscriminatorPropertyName = "state")]
[JsonDerivedType(typeof(FoundPlaylistDetail), "found")]
[JsonDerivedType(typeof(UnavailablePlaylistDetail), "unavailable")]
public abstract record PlaylistDetailResult;

public sealed record FoundPlaylistDetail(
    [property: JsonRequired, JsonPropertyName("playlist")] Playlist Playlist) : PlaylistDetailResult;

public sealed record UnavailablePlaylistDetail(
    [property: JsonRequired, JsonPropertyName("message")] string Message) : PlaylistDetailResult;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(DeviceSelectionMutationTarget), "device_selection")]
[JsonDerivedType(typeof(ManualPlaylistMutationTarget), "manual_playlist")]
public abstract record LibraryMutationTarget;

public sealed record DeviceSelectionMutationTarget(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId) : LibraryMutationTarget;

public sealed record ManualPlaylistMutationTarget(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug) : LibraryMutationTarget;

[JsonConverter(typeof(DropSyncDispositionJsonConverter))]
public abstract record DropSyncDisposition;
public sealed record StartedSyncDisposition(ulong SessionId) : DropSyncDisposition;
public sealed record NextSyncDisposition : DropSyncDisposition;
public sealed record AlreadyPresentDisposition : DropSyncDisposition;

public sealed class DropSyncDispositionJsonConverter : JsonConverter<DropSyncDisposition>
{
    public override DropSyncDisposition Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType == JsonTokenType.String)
        {
            return reader.GetString() switch
            {
                "next_sync" => new NextSyncDisposition(),
                "already_present" => new AlreadyPresentDisposition(),
                _ => throw new JsonException("unknown drop sync disposition"),
            };
        }

        using var document = JsonDocument.ParseValue(ref reader);
        if (document.RootElement.TryGetProperty("started", out var started) &&
            started.TryGetProperty("session_id", out var sessionId))
        {
            return new StartedSyncDisposition(sessionId.GetUInt64());
        }
        throw new JsonException("unknown drop sync disposition");
    }

    public override void Write(Utf8JsonWriter writer, DropSyncDisposition value, JsonSerializerOptions options)
    {
        switch (value)
        {
            case NextSyncDisposition: writer.WriteStringValue("next_sync"); return;
            case AlreadyPresentDisposition: writer.WriteStringValue("already_present"); return;
            case StartedSyncDisposition started:
                writer.WriteStartObject();
                writer.WritePropertyName("started");
                writer.WriteStartObject();
                writer.WriteNumber("session_id", started.SessionId);
                writer.WriteEndObject();
                writer.WriteEndObject();
                return;
            default: throw new JsonException("unknown drop sync disposition");
        }
    }
}
