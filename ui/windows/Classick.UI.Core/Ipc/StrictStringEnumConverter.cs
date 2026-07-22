using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

public sealed class StrictStringEnumConverter<T> : JsonConverter<T> where T : struct, Enum
{
    private static readonly IReadOnlyDictionary<string, T> ValuesByName = Enum.GetValues<T>()
        .ToDictionary(ReadWireName, value => value, StringComparer.Ordinal);

    private static readonly IReadOnlyDictionary<T, string> NamesByValue = ValuesByName
        .ToDictionary(pair => pair.Value, pair => pair.Key);

    public override T Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType != JsonTokenType.String ||
            reader.GetString() is not { } name ||
            !ValuesByName.TryGetValue(name, out var value))
        {
            throw new JsonException($"invalid {typeof(T).Name} wire value");
        }

        return value;
    }

    public override void Write(Utf8JsonWriter writer, T value, JsonSerializerOptions options)
    {
        if (!NamesByValue.TryGetValue(value, out var name))
        {
            throw new JsonException($"invalid {typeof(T).Name} value");
        }

        writer.WriteStringValue(name);
    }

    private static string ReadWireName(T value)
    {
        var member = typeof(T).GetMember(value.ToString(), BindingFlags.Public | BindingFlags.Static).Single();
        return member.GetCustomAttribute<JsonStringEnumMemberNameAttribute>()?.Name ?? member.Name;
    }
}
