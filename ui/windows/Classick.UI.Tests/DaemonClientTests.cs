using Classick_UI.Ipc;
using System.Text.Json;

namespace Classick_UI.Tests;

public class DaemonClientTests
{
    [Theory]
    [InlineData("3.0.0", true)]
    [InlineData("3.8.2", true)]
    [InlineData("2.99.0", false)]
    [InlineData("03.0.0", false)]
    [InlineData("not-a-version", false)]
    public void IsProtocolVersionSupported_UsesSemanticMajorCompatibility(string version, bool expected)
    {
        Assert.Equal(expected, DaemonClient.IsProtocolVersionSupported(version));
    }

    [Fact]
    public async Task ReadAdmittedEventsAsync_PreservesInputOrderAndIgnoresAdditiveUnknownEvents()
    {
        var input = string.Join('\n',
            """{"type":"library_scan_started","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808","session_id":43}""",
            """{"type":"future_daemon_event","value":1}""",
            """{"type":"library_scan_progress","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808","session_id":43,"files_scanned":2,"tracks_indexed":1}""",
            """{"type":"library_scan_finished","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808","session_id":43,"success":true}""");
        using var reader = new StringReader(input);

        var events = new List<WireEvent>();
        await foreach (var wireEvent in DaemonClient.ReadAdmittedEventsAsync(reader))
        {
            events.Add(wireEvent);
        }

        Assert.Collection(events,
            item => Assert.IsType<LibraryScanStartedEvent>(item),
            item => Assert.IsType<LibraryScanProgressEvent>(item),
            item => Assert.IsType<LibraryScanFinishedEvent>(item));
    }

    [Fact]
    public void ValidatePeerHello_RejectsMissingRequiredCapability()
    {
        var hello = new WireHello
        {
            ProtocolVersion = "3.0.0",
            Role = EndpointRole.Daemon,
            SoftwareVersion = "1.2.3",
            Capabilities = ["device_inventory", "portable_profile"],
        };

        Assert.Throws<InvalidOperationException>(() =>
            WireCodec.ValidatePeerHello(hello, EndpointRole.Daemon, WireCodec.RequiredDaemonCapabilities));
    }

    [Fact]
    public void WorkerProgress_ForWrongOwnedSession_IsRejected()
    {
        var expected = new OwnedSessionRoute(DeviceId.Parse("000A27002138B0A8"), 42);
        var json = """{"type":"track_done","device_id":"000A27002138B0A9","session_id":42,"result":"applied"}""";

        Assert.ThrowsAny<Exception>(() => WireCodec.DecodeAdmittedMessage(
            json,
            WireStream.DaemonReceivingWorkerEvents,
            expected));
    }

    [Fact]
    public void Encode_RejectsNonCanonicalRoutingCreatedByClientCode()
    {
        var command = new GetInventoryCommand("not-a-request-id");

        Assert.ThrowsAny<Exception>(() => WireCodec.Encode(command));
    }

    [Fact]
    public void Encode_RejectsNullDeviceIdInjectedByClientCode()
    {
        var command = new ForgetDeviceCommand(
            null!,
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8764");

        Assert.ThrowsAny<Exception>(() => WireCodec.Encode(command));
    }

    [Fact]
    public void LegacyCommandSurface_StillSerializesForPendingUiMigration()
    {
        DaemonCommand command = new GetHistoryCommand(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808",
            10);

        Assert.Equal(
            """{"type":"get_history","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808","limit":10}""",
            JsonSerializer.Serialize<DaemonCommand>(command));
    }
}
