using System.Text.Json;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonConverter(typeof(DeviceIdJsonConverter))]
public sealed record DeviceId
{
    private DeviceId(string value) => Value = value;

    public string Value { get; }

    public static DeviceId Parse(string value)
    {
        if (value.Length != 16 || value.Any(character => !Uri.IsHexDigit(character)) ||
            !string.Equals(value, value.ToUpperInvariant(), StringComparison.Ordinal))
        {
            throw new FormatException("device ID must be exactly 16 uppercase hexadecimal characters");
        }

        return new DeviceId(value);
    }

    public override string ToString() => Value;
}

public sealed class DeviceIdJsonConverter : JsonConverter<DeviceId>
{
    public override DeviceId Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        var value = reader.GetString() ?? throw new JsonException("device ID must be a string");
        try
        {
            return DeviceId.Parse(value);
        }
        catch (FormatException exception)
        {
            throw new JsonException(exception.Message, exception);
        }
    }

    public override void Write(Utf8JsonWriter writer, DeviceId value, JsonSerializerOptions options) =>
        writer.WriteStringValue(value.Value);
}

[JsonConverter(typeof(StrictStringEnumConverter<EndpointRole>))]
public enum EndpointRole
{
    [JsonStringEnumMemberName("desktop")] Desktop,
    [JsonStringEnumMemberName("daemon")] Daemon,
    [JsonStringEnumMemberName("worker")] Worker,
}

[JsonConverter(typeof(StrictStringEnumConverter<SelectionMode>))]
public enum SelectionMode
{
    [JsonStringEnumMemberName("all")] All,
    [JsonStringEnumMemberName("include")] Include,
    [JsonStringEnumMemberName("exclude")] Exclude,
}

[JsonPolymorphic(TypeDiscriminatorPropertyName = "kind")]
[JsonDerivedType(typeof(ArtistSelectionRule), "artist")]
[JsonDerivedType(typeof(AlbumSelectionRule), "album")]
[JsonDerivedType(typeof(GenreSelectionRule), "genre")]
public abstract record SelectionRule;

public sealed record ArtistSelectionRule(
    [property: JsonRequired, JsonPropertyName("name")] string Name) : SelectionRule;

public sealed record AlbumSelectionRule(
    [property: JsonRequired, JsonPropertyName("artist")] string Artist,
    [property: JsonRequired, JsonPropertyName("album")] string Album) : SelectionRule;

public sealed record GenreSelectionRule(
    [property: JsonRequired, JsonPropertyName("name")] string Name) : SelectionRule;

public sealed record SelectionValue(
    [property: JsonRequired, JsonPropertyName("schema_version")] uint SchemaVersion,
    [property: JsonRequired, JsonPropertyName("mode")] SelectionMode Mode,
    [property: JsonRequired, JsonPropertyName("rules")] IReadOnlyList<SelectionRule> Rules);

public sealed record SettingsValue(
    [property: JsonRequired, JsonPropertyName("schema_version")] uint SchemaVersion,
    [property: JsonRequired, JsonPropertyName("auto_sync")] bool AutoSync,
    [property: JsonRequired, JsonPropertyName("rockbox_compat")] bool RockboxCompat);

public sealed record SubscriptionsValue(
    [property: JsonRequired, JsonPropertyName("schema_version")] uint SchemaVersion,
    [property: JsonRequired, JsonPropertyName("playlists")] IReadOnlyList<string> Playlists);

[JsonPolymorphic(TypeDiscriminatorPropertyName = "state")]
[JsonDerivedType(typeof(PendingDeviceDelivery), "pending_device")]
[JsonDerivedType(typeof(DeviceCommittedDelivery), "device_committed")]
public abstract record ConfigDelivery;

public sealed record PendingDeviceDelivery(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("last_failure")]
    string? LastFailure = null) : ConfigDelivery;

public sealed record DeviceCommittedDelivery : ConfigDelivery;

public sealed record DeliveredComponent<T>(
    [property: JsonRequired, JsonPropertyName("revision")] ulong Revision,
    [property: JsonRequired, JsonPropertyName("mutation_id")] string MutationId,
    [property: JsonRequired, JsonPropertyName("value")] T Value,
    [property: JsonRequired, JsonPropertyName("delivery")] ConfigDelivery Delivery);
