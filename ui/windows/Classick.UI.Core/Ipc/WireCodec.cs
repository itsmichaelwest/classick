using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace Classick_UI.Ipc;

public enum WireStream
{
    DesktopReceivingDaemonEvents,
    DaemonReceivingDesktopCommands,
    DaemonReceivingWorkerEvents,
    WorkerReceivingDaemonCommands,
}

public sealed record OwnedSessionRoute(DeviceId DeviceId, ulong SessionId);

public abstract record PendingWorkerInteraction
{
    public sealed record None : PendingWorkerInteraction;
    public sealed record Review : PendingWorkerInteraction;
    public sealed record Prompt(ulong PromptId, uint OptionCount) : PendingWorkerInteraction;
    public sealed record Form(ulong PromptId) : PendingWorkerInteraction;
}

public abstract record WireDecodeResult;
public sealed record KnownWireMessage(WireMessage Message) : WireDecodeResult;
public sealed record IgnoredUnknownEvent(string MessageType) : WireDecodeResult;

public static partial class WireCodec
{
    public const string ProtocolVersion = "3.0.0";

    public static readonly IReadOnlyList<string> RequiredDaemonCapabilities =
        ["device_inventory", "portable_profile", "typed_sync_progress"];

    private static readonly JsonSerializerOptions SerializerOptions = new()
    {
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        RespectNullableAnnotations = true,
    };

    private static readonly HashSet<string> CommandTypes =
    [
        "get_global_config", "set_source_location", "set_global_settings", "get_inventory",
        "subscribe_inventory", "unsubscribe_inventory", "adopt_device", "forget_device",
        "get_device_config", "set_selection", "set_settings", "set_subscriptions", "trigger_sync",
        "backfill_rockbox", "replace_library", "get_history", "get_library", "scan_library",
        "retry_source_mount", "preview_selection", "preview_device", "resolve_tracks",
        "add_selection_to_device", "list_playlists", "get_playlist", "save_playlist",
        "delete_playlist", "append_selection_to_playlist", "shutdown", "apply_review",
        "dry_run_review", "quit_review", "prompt_decision", "form_decision", "cancel_sync",
        "pause_sync",
    ];

    private static readonly HashSet<string> EventTypes =
    [
        "global_config", "source_availability", "device_inventory", "inventory_subscription_changed",
        "device_config", "config_mutation_failed", "device_forgotten", "sync_accepted",
        "sync_rejected", "history", "library", "library_scan_started", "library_scan_progress",
        "library_scan_finished", "selection_preview", "device_preview", "resolved_tracks",
        "playlists", "playlist_detail", "playlist_saved", "device_selection_added",
        "playlist_selection_appended", "library_mutation_rejected", "daemon_shutdown_started",
        "run_header", "sync_summary", "review_requested", "prompt", "form", "track_start",
        "track_done", "finalizing", "sync_cancelled", "sync_paused", "sync_log", "sync_error",
        "sync_finished", "command_failed",
    ];

    private static readonly HashSet<string> WorkerEventTypes =
    [
        "run_header", "sync_summary", "review_requested", "prompt", "form", "track_start",
        "track_done", "finalizing", "sync_cancelled", "sync_paused", "sync_log", "sync_error",
        "sync_finished",
    ];

    public static WireHello DecodeInitialHello(string json)
    {
        using var document = ParseObject(json);
        if (ReadMessageType(document.RootElement) != "hello")
        {
            throw new JsonException("first wire message must be hello");
        }

        var hello = JsonSerializer.Deserialize<WireHello>(json, SerializerOptions) ??
            throw new JsonException("hello payload was empty");
        ValidateHello(hello);
        return hello with { Capabilities = hello.Capabilities.Order(StringComparer.Ordinal).ToArray() };
    }

    public static void ValidatePeerHello(
        WireHello hello,
        EndpointRole expectedRole,
        IEnumerable<string>? requiredCapabilities = null)
    {
        ValidateHello(hello);
        if (ReadSemVerMajor(hello.ProtocolVersion) != ReadSemVerMajor(ProtocolVersion))
        {
            throw new InvalidOperationException($"incompatible wire protocol {hello.ProtocolVersion}");
        }
        if (hello.Role != expectedRole)
        {
            throw new InvalidOperationException($"unexpected peer role {hello.Role}");
        }
        foreach (var capability in requiredCapabilities ?? [])
        {
            if (!hello.Capabilities.Contains(capability, StringComparer.Ordinal))
            {
                throw new InvalidOperationException($"peer does not advertise {capability}");
            }
        }
    }

    public static WireDecodeResult DecodeAdmittedMessage(
        string json,
        WireStream stream,
        OwnedSessionRoute? ownedSession = null,
        PendingWorkerInteraction? pendingInteraction = null)
    {
        using var document = ParseObject(json);
        var root = document.RootElement;
        var messageType = ReadMessageType(root);
        if (messageType == "hello")
        {
            throw new JsonException("hello is only valid as the first wire message");
        }

        var isCommand = CommandTypes.Contains(messageType);
        var isEvent = EventTypes.Contains(messageType);
        if (!isCommand && !isEvent)
        {
            if (stream == WireStream.DesktopReceivingDaemonEvents)
            {
                return new IgnoredUnknownEvent(messageType);
            }
            throw new JsonException($"unknown {messageType} message");
        }

        if (isCommand && root.TryGetProperty("observation_id", out _))
        {
            throw new JsonException("observation ID is not accepted by wire commands");
        }
        ValidateRawRouting(root);

        WireMessage message = stream switch
        {
            WireStream.DesktopReceivingDaemonEvents when isEvent => DeserializeEvent(json),
            WireStream.DaemonReceivingDesktopCommands when isCommand => DeserializeCommand(json),
            WireStream.DaemonReceivingWorkerEvents when isEvent && WorkerEventTypes.Contains(messageType) =>
                DeserializeEvent(json),
            WireStream.WorkerReceivingDaemonCommands when isCommand => DeserializeCommand(json),
            _ => throw new JsonException($"{messageType} is not valid on this wire stream"),
        };

        WireValidation.Validate(message);
        if (stream == WireStream.DaemonReceivingWorkerEvents)
        {
            ValidateOwnedRoute(message, ownedSession);
        }
        else if (stream == WireStream.WorkerReceivingDaemonCommands)
        {
            ValidateWorkerCommand(message, ownedSession, pendingInteraction);
        }

        return new KnownWireMessage(message);
    }

    public static string Encode(WireMessage message)
    {
        ArgumentNullException.ThrowIfNull(message);
        WireValidation.Validate(message);
        return message switch
        {
            WireHello hello => JsonSerializer.Serialize(
                hello with { Capabilities = hello.Capabilities.Order(StringComparer.Ordinal).ToArray() },
                SerializerOptions),
            WireCommand command => JsonSerializer.Serialize<WireCommand>(command, SerializerOptions),
            WireEvent wireEvent => JsonSerializer.Serialize<WireEvent>(wireEvent, SerializerOptions),
            _ => throw new JsonException($"unknown wire message {message.GetType().Name}"),
        };
    }

    private static WireCommand DeserializeCommand(string json) =>
        JsonSerializer.Deserialize<WireCommand>(json, SerializerOptions) ?? throw new JsonException("empty wire command");

    private static WireEvent DeserializeEvent(string json) =>
        JsonSerializer.Deserialize<WireEvent>(json, SerializerOptions) ?? throw new JsonException("empty wire event");

    private static JsonDocument ParseObject(string json)
    {
        var document = JsonDocument.Parse(json);
        if (document.RootElement.ValueKind != JsonValueKind.Object)
        {
            document.Dispose();
            throw new JsonException("wire message must be a JSON object");
        }
        return document;
    }

    private static string ReadMessageType(JsonElement root)
    {
        if (!root.TryGetProperty("type", out var element) || element.ValueKind != JsonValueKind.String ||
            string.IsNullOrEmpty(element.GetString()))
        {
            throw new JsonException("wire message requires a non-empty string type");
        }
        return element.GetString()!;
    }

    private static void ValidateRawRouting(JsonElement root)
    {
        foreach (var name in new[] { "request_id", "mutation_id", "selection_mutation_id", "settings_mutation_id", "subscriptions_mutation_id" })
        {
            if (root.TryGetProperty(name, out var value))
            {
                ValidateUuid(value.GetString(), name);
            }
        }
    }

    private static void ValidateOwnedRoute(WireMessage message, OwnedSessionRoute? expected)
    {
        if (expected is null || message is not ISessionRoutedMessage routed ||
            routed.DeviceId != expected.DeviceId || routed.SessionId != expected.SessionId)
        {
            throw new JsonException("message does not match the owned worker session");
        }
    }

    private static void ValidateWorkerCommand(
        WireMessage message,
        OwnedSessionRoute? expected,
        PendingWorkerInteraction? pending)
    {
        ValidateOwnedRoute(message, expected);
        var accepted = (message, pending) switch
        {
            (ApplyReviewCommand or DryRunReviewCommand or QuitReviewCommand, PendingWorkerInteraction.Review) => true,
            (PromptDecisionCommand command, PendingWorkerInteraction.Prompt interaction) =>
                command.PromptId == interaction.PromptId && command.Choice < interaction.OptionCount,
            (FormDecisionCommand command, PendingWorkerInteraction.Form interaction) =>
                command.PromptId == interaction.PromptId,
            (WireCancelSyncCommand or PauseSyncCommand, _) => true,
            _ => false,
        };
        if (!accepted)
        {
            throw new JsonException("command does not match the worker's pending interaction");
        }
    }

    internal static void ValidateHello(WireHello hello)
    {
        ReadSemVerMajor(hello.ProtocolVersion);
        ReadSemVerMajor(hello.SoftwareVersion);
        var capabilities = new HashSet<string>(StringComparer.Ordinal);
        foreach (var capability in hello.Capabilities)
        {
            if (!CapabilityRegex().IsMatch(capability) || !capabilities.Add(capability))
            {
                throw new JsonException("hello contains an invalid or duplicate capability");
            }
        }
    }

    private static int ReadSemVerMajor(string value)
    {
        var match = SemVerRegex().Match(value);
        if (!match.Success)
        {
            throw new JsonException("version is not semantic versioning");
        }
        return int.Parse(match.Groups[1].Value, System.Globalization.CultureInfo.InvariantCulture);
    }

    internal static void ValidateUuid(string? value, string label)
    {
        if (value is null || !UuidRegex().IsMatch(value) || value == "00000000-0000-0000-0000-000000000000")
        {
            throw new JsonException($"{label} must be a non-nil lowercase UUID");
        }
    }

    [GeneratedRegex("^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:-((?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(?:\\.(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$")]
    private static partial Regex SemVerRegex();

    [GeneratedRegex("^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$")]
    private static partial Regex CapabilityRegex();

    [GeneratedRegex("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")]
    private static partial Regex UuidRegex();
}
