using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

[JsonConverter(typeof(StrictStringEnumConverter<DeviceReadiness>))]
public enum DeviceReadiness
{
    [JsonStringEnumMemberName("ready")] Ready,
    [JsonStringEnumMemberName("needs_apple_initialization")] NeedsAppleInitialization,
    [JsonStringEnumMemberName("invalid_database")] InvalidDatabase,
    [JsonStringEnumMemberName("identity_unavailable")] IdentityUnavailable,
}

[JsonConverter(typeof(StrictStringEnumConverter<DevicePhase>))]
public enum DevicePhase
{
    [JsonStringEnumMemberName("disconnected")] Disconnected,
    [JsonStringEnumMemberName("unconfigured")] Unconfigured,
    [JsonStringEnumMemberName("idle")] Idle,
    [JsonStringEnumMemberName("syncing")] Syncing,
    [JsonStringEnumMemberName("paused")] Paused,
    [JsonStringEnumMemberName("error")] Error,
}

[JsonConverter(typeof(StrictStringEnumConverter<ProfileStatus>))]
public enum ProfileStatus
{
    [JsonStringEnumMemberName("not_adopted")] NotAdopted,
    [JsonStringEnumMemberName("pending_adoption")] PendingAdoption,
    [JsonStringEnumMemberName("adopted")] Adopted,
    [JsonStringEnumMemberName("invalid")] Invalid,
    [JsonStringEnumMemberName("recovery_required")] RecoveryRequired,
}

[JsonConverter(typeof(StrictStringEnumConverter<FactSource>))]
public enum FactSource
{
    [JsonStringEnumMemberName("reported")] Reported,
    [JsonStringEnumMemberName("decoded")] Decoded,
    [JsonStringEnumMemberName("inferred")] Inferred,
}

[JsonConverter(typeof(StrictStringEnumConverter<FactConfidence>))]
public enum FactConfidence
{
    [JsonStringEnumMemberName("certain")] Certain,
    [JsonStringEnumMemberName("heuristic")] Heuristic,
}

[JsonConverter(typeof(StrictStringEnumConverter<IpodFamily>))]
public enum IpodFamily
{
    [JsonStringEnumMemberName("ipod")] Ipod,
    [JsonStringEnumMemberName("classic")] Classic,
    [JsonStringEnumMemberName("nano")] Nano,
    [JsonStringEnumMemberName("mini")] Mini,
    [JsonStringEnumMemberName("shuffle")] Shuffle,
    [JsonStringEnumMemberName("photo")] Photo,
    [JsonStringEnumMemberName("video")] Video,
    [JsonStringEnumMemberName("touch")] Touch,
}

[JsonConverter(typeof(StrictStringEnumConverter<IpodColour>))]
public enum IpodColour
{
    [JsonStringEnumMemberName("silver")] Silver,
    [JsonStringEnumMemberName("black")] Black,
    [JsonStringEnumMemberName("white")] White,
    [JsonStringEnumMemberName("blue")] Blue,
    [JsonStringEnumMemberName("green")] Green,
    [JsonStringEnumMemberName("pink")] Pink,
    [JsonStringEnumMemberName("red")] Red,
    [JsonStringEnumMemberName("yellow")] Yellow,
    [JsonStringEnumMemberName("purple")] Purple,
    [JsonStringEnumMemberName("orange")] Orange,
    [JsonStringEnumMemberName("gold")] Gold,
    [JsonStringEnumMemberName("stainless_steel")] StainlessSteel,
}

public sealed record HardwareFact<T>(
    [property: JsonRequired, JsonPropertyName("value")] T Value,
    [property: JsonRequired, JsonPropertyName("source")] FactSource Source,
    [property: JsonRequired, JsonPropertyName("confidence")] FactConfidence Confidence);

public sealed record HardwareFacts(
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("family")] HardwareFact<IpodFamily>? Family = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("generation")] HardwareFact<string>? Generation = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("model_code")] HardwareFact<string>? ModelCode = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("colour")] HardwareFact<IpodColour>? Colour = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("firmware")] HardwareFact<string>? Firmware = null,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("capacity_bytes")] HardwareFact<ulong>? CapacityBytes = null);

[JsonConverter(typeof(StrictStringEnumConverter<StorageFreshness>))]
public enum StorageFreshness
{
    [JsonStringEnumMemberName("live")] Live,
    [JsonStringEnumMemberName("cached")] Cached,
}

public sealed record StorageSnapshot(
    [property: JsonRequired, JsonPropertyName("total_bytes")] ulong TotalBytes,
    [property: JsonRequired, JsonPropertyName("free_bytes")] ulong FreeBytes,
    [property: JsonRequired, JsonPropertyName("freshness")] StorageFreshness Freshness);

public sealed record IdentifiedDeviceSnapshot(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("name")] string? Name,
    [property: JsonRequired, JsonPropertyName("readiness")] DeviceReadiness Readiness,
    [property: JsonRequired, JsonPropertyName("hardware")] HardwareFacts Hardware,
    [property: JsonRequired, JsonPropertyName("profile_status")] ProfileStatus ProfileStatus,
    [property: JsonRequired, JsonPropertyName("connected")] bool Connected,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("mount_path")] string? MountPath,
    [property: JsonRequired, JsonPropertyName("phase")] DevicePhase Phase,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("session_id")] ulong? SessionId,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("storage")] StorageSnapshot? Storage,
    [property: JsonRequired, JsonPropertyName("synced_count")] ulong SyncedCount,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("library_count")] ulong? LibraryCount,
    [property: JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull), JsonPropertyName("last_terminal_error")] string? LastTerminalError);

public sealed record UnidentifiedDeviceSnapshot(
    [property: JsonRequired, JsonPropertyName("observation_id")] ulong ObservationId,
    [property: JsonRequired, JsonPropertyName("readiness")] DeviceReadiness Readiness,
    [property: JsonRequired, JsonPropertyName("hardware")] HardwareFacts Hardware);

public sealed record DeviceConfigSnapshot(
    [property: JsonRequired, JsonPropertyName("device_id")] DeviceId DeviceId,
    [property: JsonRequired, JsonPropertyName("selection")] DeliveredComponent<SelectionValue> Selection,
    [property: JsonRequired, JsonPropertyName("settings")] DeliveredComponent<SettingsValue> Settings,
    [property: JsonRequired, JsonPropertyName("subscriptions")] DeliveredComponent<SubscriptionsValue> Subscriptions);
