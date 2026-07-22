using System.Text.Json;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public class IpcWireFormatTests
{
    private static string VectorRoot => Path.Combine(AppContext.BaseDirectory, "WireV3");

    [Fact]
    public void SharedPositiveCollections_DecodeAndRoundTripExactly()
    {
        using var manifest = ReadJson("manifest.json");
        foreach (var groupName in new[] { "progress", "device", "operations" })
        {
            foreach (var collection in manifest.RootElement.GetProperty(groupName).GetProperty("positive_collections").EnumerateArray())
            {
                var stream = ParseStream(collection.GetProperty("stream").GetString()!);
                foreach (var line in File.ReadLines(Path.Combine(VectorRoot, collection.GetProperty("path").GetString()!)))
                {
                    var decoded = Assert.IsType<KnownWireMessage>(WireCodec.DecodeAdmittedMessage(line, stream));
                    Assert.Equal(line, WireCodec.Encode(decoded.Message));
                }
            }
        }
    }

    [Fact]
    public void SharedNegativeVectors_AreRejectedByTheirAdmittedStream()
    {
        using var manifest = ReadJson("manifest.json");
        foreach (var groupName in new[] { "progress", "device", "operations" })
        {
            foreach (var vector in manifest.RootElement.GetProperty(groupName).GetProperty("negative_vectors").EnumerateArray())
            {
                var json = File.ReadAllText(Path.Combine(VectorRoot, vector.GetProperty("path").GetString()!));
                var stream = ParseStream(vector.GetProperty("stream").GetString()!);
                var route = ReadRoute(vector);
                var pending = ReadPendingInteraction(vector);
                Assert.ThrowsAny<Exception>(() => WireCodec.DecodeAdmittedMessage(json, stream, route, pending));
            }
        }
    }

    [Fact]
    public void SharedHelloVectors_EnforceDecodeAndAdmissionExpectations()
    {
        using var manifest = ReadJson("manifest.json");
        foreach (var vector in manifest.RootElement.GetProperty("vectors").EnumerateArray())
        {
            var expectation = vector.GetProperty("expectation").GetString();
            var json = File.ReadAllText(Path.Combine(VectorRoot, vector.GetProperty("path").GetString()!));
            switch (expectation)
            {
                case "valid_hello":
                case "canonicalize_hello":
                    var hello = WireCodec.DecodeInitialHello(json);
                    if (vector.TryGetProperty("expected_role", out var expectedRole))
                    {
                        var capabilities = vector.TryGetProperty("required_capabilities", out var required)
                            ? required.EnumerateArray().Select(item => item.GetString()!).ToArray()
                            : [];
                        WireCodec.ValidatePeerHello(hello, ParseRole(expectedRole.GetString()!), capabilities);
                    }
                    Assert.Equal(hello.Capabilities.Order(StringComparer.Ordinal), hello.Capabilities);
                    break;
                case "admission_failure":
                    var admitted = WireCodec.DecodeInitialHello(json);
                    Assert.Throws<InvalidOperationException>(() =>
                        WireCodec.ValidatePeerHello(admitted, ParseRole(vector.GetProperty("expected_role").GetString()!)));
                    break;
                case "decode_failure":
                    Assert.Throws<JsonException>(() => WireCodec.DecodeInitialHello(json));
                    break;
                case "ignored_desktop_event":
                    var ignored = WireCodec.DecodeAdmittedMessage(json, WireStream.DesktopReceivingDaemonEvents);
                    Assert.IsType<IgnoredUnknownEvent>(ignored);
                    break;
                default:
                    throw new InvalidOperationException($"unknown vector expectation {expectation}");
            }
        }
    }

    [Theory]
    [InlineData("000a27002138b0a8")]
    [InlineData("0x000A27002138B0A8")]
    [InlineData("000A27002138B0A")]
    public void DeviceId_Parse_RejectsNonCanonicalWireValues(string value)
    {
        Assert.Throws<FormatException>(() => DeviceId.Parse(value));
    }

    [Fact]
    public void UnknownDesktopEvent_IsIgnoredWithoutInventingKnownState()
    {
        var result = WireCodec.DecodeAdmittedMessage(
            """{"type":"future_device_phase","phase":"teleporting"}""",
            WireStream.DesktopReceivingDaemonEvents);

        var ignored = Assert.IsType<IgnoredUnknownEvent>(result);
        Assert.Equal("future_device_phase", ignored.MessageType);
    }

    [Fact]
    public void NoPositiveVectorContainsNestedSyncJson()
    {
        foreach (var file in Directory.EnumerateFiles(VectorRoot, "*.ndjson", SearchOption.AllDirectories))
        {
            foreach (var line in File.ReadLines(file))
            {
                using var document = JsonDocument.Parse(line);
                Assert.False(document.RootElement.TryGetProperty("line", out _));
                Assert.NotEqual("sync_event", document.RootElement.GetProperty("type").GetString());
            }
        }
    }

    [Fact]
    public void EncodeHello_CanonicalizesCapabilityOrder()
    {
        var hello = new WireHello
        {
            ProtocolVersion = "3.0.0",
            Role = EndpointRole.Daemon,
            SoftwareVersion = "1.0.0",
            Capabilities = ["typed_sync_progress", "device_inventory", "portable_profile"],
        };

        Assert.Equal(
            """{"type":"hello","protocol_version":"3.0.0","role":"daemon","software_version":"1.0.0","capabilities":["device_inventory","portable_profile","typed_sync_progress"]}""",
            WireCodec.Encode(hello));
    }

    [Theory]
    [InlineData("""{"type":"get_inventory","request_id":null}""")]
    [InlineData("""{"type":"source_availability","state":0}""")]
    [InlineData("""{"type":"source_availability","state":"AVAILABLE","source_root":"/Music"}""")]
    [InlineData("""{"type":"device_inventory","revision":1,"devices":null,"unidentified":[]}""")]
    public void Decode_RejectsNullOrNonCanonicalRoutingAndEnumFields(string json)
    {
        var stream = json.Contains("get_inventory", StringComparison.Ordinal)
            ? WireStream.DaemonReceivingDesktopCommands
            : WireStream.DesktopReceivingDaemonEvents;

        Assert.ThrowsAny<Exception>(() => WireCodec.DecodeAdmittedMessage(json, stream));
    }

    [Theory]
    [InlineData("""{"type":"library_scan_started","session_id":43}""")]
    [InlineData("""{"type":"library_scan_progress","session_id":43,"files_scanned":2,"tracks_indexed":1}""")]
    [InlineData("""{"type":"library_scan_finished","session_id":43,"success":true}""")]
    public void UnsolicitedLibraryScanEvents_OmitRequestId(string json)
    {
        var decoded = Assert.IsType<KnownWireMessage>(WireCodec.DecodeAdmittedMessage(
            json,
            WireStream.DesktopReceivingDaemonEvents));

        Assert.Equal(json, WireCodec.Encode(decoded.Message));
    }

    private static JsonDocument ReadJson(string relativePath) =>
        JsonDocument.Parse(File.ReadAllText(Path.Combine(VectorRoot, relativePath)));

    private static WireStream ParseStream(string value) => value switch
    {
        "desktop_to_daemon_commands" => WireStream.DaemonReceivingDesktopCommands,
        "daemon_to_desktop_events" => WireStream.DesktopReceivingDaemonEvents,
        "worker_to_daemon_events" => WireStream.DaemonReceivingWorkerEvents,
        "daemon_to_worker_commands" => WireStream.WorkerReceivingDaemonCommands,
        _ => throw new InvalidOperationException($"unknown vector stream {value}"),
    };

    private static EndpointRole ParseRole(string value) => value switch
    {
        "desktop" => EndpointRole.Desktop,
        "daemon" => EndpointRole.Daemon,
        "worker" => EndpointRole.Worker,
        _ => throw new InvalidOperationException($"unknown role {value}"),
    };

    private static OwnedSessionRoute? ReadRoute(JsonElement vector)
    {
        if (!vector.TryGetProperty("expected_device_id", out var deviceId)) return null;
        return new OwnedSessionRoute(
            DeviceId.Parse(deviceId.GetString()!),
            vector.GetProperty("expected_session_id").GetUInt64());
    }

    private static PendingWorkerInteraction? ReadPendingInteraction(JsonElement vector)
    {
        if (!vector.TryGetProperty("prompt_id", out var promptId)) return null;
        return new PendingWorkerInteraction.Prompt(
            promptId.GetUInt64(),
            vector.GetProperty("option_count").GetUInt32());
    }
}
