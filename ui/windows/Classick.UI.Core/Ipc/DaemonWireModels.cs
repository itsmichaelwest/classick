using System.Text.Json;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonConverter(typeof(JsonStringEnumConverter<SelectionMode>))]
public enum SelectionMode
{
    [JsonStringEnumMemberName("all")]
    All,
    [JsonStringEnumMemberName("include")]
    Include,
    [JsonStringEnumMemberName("exclude")]
    Exclude,
}

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(ArtistSelectionRule), "artist")]
[JsonDerivedType(typeof(AlbumSelectionRule), "album")]
[JsonDerivedType(typeof(GenreSelectionRule), "genre")]
public abstract record SelectionRule;

public sealed record ArtistSelectionRule(
    [property: JsonRequired, JsonPropertyName("name")] string Name
) : SelectionRule;

public sealed record AlbumSelectionRule(
    [property: JsonRequired, JsonPropertyName("artist")] string Artist,
    [property: JsonRequired, JsonPropertyName("album")] string Album
) : SelectionRule;

public sealed record GenreSelectionRule(
    [property: JsonRequired, JsonPropertyName("name")] string Name
) : SelectionRule;

public sealed record SelectionState(
    [property: JsonPropertyName("mode")] SelectionMode Mode,
    [property: JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules
);

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(ManualPlaylistPayload), "manual")]
[JsonDerivedType(typeof(SmartPlaylistPayload), "smart")]
public abstract record PlaylistPayload;

public sealed record ManualPlaylistPayload(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("slug")] string? Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("tracks")] IReadOnlyList<string> Tracks
) : PlaylistPayload;

public sealed record SmartPlaylistPayload(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("slug")] string? Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("rules")] SmartRules Rules
) : PlaylistPayload;

[JsonConverter(typeof(JsonStringEnumConverter<SmartMatching>))]
public enum SmartMatching
{
    [JsonStringEnumMemberName("all")]
    All,
    [JsonStringEnumMemberName("any")]
    Any,
}

[JsonConverter(typeof(JsonStringEnumConverter<SmartField>))]
public enum SmartField
{
    [JsonStringEnumMemberName("artist")]
    Artist,
    [JsonStringEnumMemberName("album")]
    Album,
    [JsonStringEnumMemberName("genre")]
    Genre,
    [JsonStringEnumMemberName("year")]
    Year,
}

[JsonConverter(typeof(JsonStringEnumConverter<SmartOperation>))]
public enum SmartOperation
{
    [JsonStringEnumMemberName("is")]
    Is,
    [JsonStringEnumMemberName("contains")]
    Contains,
    [JsonStringEnumMemberName("gte")]
    Gte,
    [JsonStringEnumMemberName("lte")]
    Lte,
}

[JsonConverter(typeof(JsonStringEnumConverter<SmartOrder>))]
public enum SmartOrder
{
    [JsonStringEnumMemberName("recently_modified")]
    RecentlyModified,
    [JsonStringEnumMemberName("random_stable")]
    RandomStable,
    [JsonStringEnumMemberName("alpha")]
    Alpha,
}

public sealed record SmartRule(
    [property: JsonRequired, JsonPropertyName("field")] SmartField Field,
    [property: JsonRequired, JsonPropertyName("op")] SmartOperation Operation,
    [property: JsonRequired, JsonPropertyName("value")] string Value
);

public sealed record SmartRules(
    [property: JsonPropertyName("version")] uint Version,
    [property: JsonPropertyName("matching")] SmartMatching Matching,
    [property: JsonPropertyName("rules")] IReadOnlyList<SmartRule> Rules,
    [property: JsonPropertyName("limit")] JsonElement? Limit,
    [property: JsonPropertyName("order")] SmartOrder Order,
    [property: JsonPropertyName("seed")] ulong Seed
);

[JsonConverter(typeof(JsonStringEnumConverter<PlaylistKind>))]
public enum PlaylistKind
{
    [JsonStringEnumMemberName("manual")]
    Manual,
    [JsonStringEnumMemberName("smart")]
    Smart,
}

public sealed record PlaylistSummary(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("kind")] PlaylistKind Kind,
    [property: JsonRequired, JsonPropertyName("tracks")] int Tracks,
    [property: JsonRequired, JsonPropertyName("bytes")] ulong Bytes,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("error")] string? Error = null
);

public sealed record Subscriptions(
    [property: JsonPropertyName("playlists")] IReadOnlyList<string> Playlists
);

public sealed record DeviceSettings(
    [property: JsonPropertyName("auto_sync")] bool AutoSync,
    [property: JsonPropertyName("rockbox_compat")] bool RockboxCompat
);

[JsonConverter(typeof(JsonStringEnumConverter<DropSyncBehavior>))]
public enum DropSyncBehavior
{
    [JsonStringEnumMemberName("immediate")]
    Immediate,
    [JsonStringEnumMemberName("next_sync")]
    NextSync,
}

[JsonConverter(typeof(JsonStringEnumConverter<DropDelivery>))]
public enum DropDelivery
{
    [JsonStringEnumMemberName("added_and_syncing")]
    AddedAndSyncing,
    [JsonStringEnumMemberName("added_for_next_sync")]
    AddedForNextSync,
    [JsonStringEnumMemberName("already_present")]
    AlreadyPresent,
}

public sealed record ManualPlaylist(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug,
    [property: JsonRequired, JsonPropertyName("name")] string Name,
    [property: JsonRequired, JsonPropertyName("tracks")] IReadOnlyList<string> Tracks
);

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(DeviceSelectionMutationTarget), "device_selection")]
[JsonDerivedType(typeof(ManualPlaylistMutationTarget), "manual_playlist")]
public abstract record LibraryMutationTarget;

public sealed record DeviceSelectionMutationTarget(
    [property: JsonRequired, JsonPropertyName("serial")] string Serial
) : LibraryMutationTarget;

public sealed record ManualPlaylistMutationTarget(
    [property: JsonRequired, JsonPropertyName("slug")] string Slug
) : LibraryMutationTarget;
