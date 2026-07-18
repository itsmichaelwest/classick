using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Classick_UI.Ipc;
using Xunit;

public class DaemonEventRouterTests
{
    [Fact]
    public async Task Routes_status_update_to_typed_subscribers()
    {
        var channel = Channel.CreateUnbounded<object>();
        StatusUpdateEvent? received = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += s => received = s;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);

        Assert.NotNull(received);
        Assert.Equal("idle", received!.State);
        router.Stop();
    }

    [Fact]
    public async Task Multiple_subscribers_all_receive_event()
    {
        var channel = Channel.CreateUnbounded<object>();
        int count1 = 0, count2 = 0;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += _ => count1++;
        router.StatusUpdated += _ => count2++;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);

        Assert.Equal(1, count1);
        Assert.Equal(1, count2);
        router.Stop();
    }

    [Fact]
    public async Task Routes_device_connected_separately_from_status()
    {
        var channel = Channel.CreateUnbounded<object>();
        StatusUpdateEvent? status = null;
        DeviceConnectedEvent? device = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += s => status = s;
        router.DeviceConnected += d => device = d;

        router.Start();
        await channel.Writer.WriteAsync(new DeviceConnectedEvent("0xABC", "iPod 7G", "G:\\"));
        await Task.Delay(50);

        Assert.Null(status);
        Assert.NotNull(device);
        Assert.Equal("0xABC", device!.Serial);
        router.Stop();
    }

    [Fact]
    public async Task Unsubscribe_stops_delivery()
    {
        var channel = Channel.CreateUnbounded<object>();
        int count = 0;
        void Handler(StatusUpdateEvent _) => count++;
        var router = new DaemonEventRouter(channel.Reader);
        router.StatusUpdated += Handler;

        router.Start();
        await channel.Writer.WriteAsync(new StatusUpdateEvent("idle", true, true, null, null));
        await Task.Delay(50);
        Assert.Equal(1, count);

        router.StatusUpdated -= Handler;
        await channel.Writer.WriteAsync(new StatusUpdateEvent("syncing", true, true, null, null));
        await Task.Delay(50);
        Assert.Equal(1, count);  // unchanged
        router.Stop();
    }

    [Fact]
    public async Task Sync_event_is_re_parsed_as_ipc_event_and_routed()
    {
        var channel = Channel.CreateUnbounded<object>();
        IpcEvent? routed = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.IpcEventReceived += e => routed = e;

        router.Start();
        // Wrapped sync subprocess event:
        await channel.Writer.WriteAsync(new SyncEventEnvelope(@"{""type"":""track_done""}", "A", 11));
        await Task.Delay(50);

        Assert.NotNull(routed);
        Assert.IsType<TrackDoneEvent>(routed);
        router.Stop();
    }

    [Fact]
    public async Task Routes_device_inventory_snapshot_to_typed_subscribers()
    {
        var channel = Channel.CreateUnbounded<object>();
        DeviceInventorySnapshotEvent? received = null;
        var router = new DaemonEventRouter(channel.Reader);
        router.DeviceInventorySnapshotReceived += snapshot => received = snapshot;

        router.Start();
        await channel.Writer.WriteAsync(new DeviceInventorySnapshotEvent(
            Revision: 3,
            Devices:
            [
                new DeviceSnapshot(
                    new DeviceIdentitySnapshot("A", "iPod 5G"),
                    Configured: true,
                    Connected: true,
                    Mount: "G:\\",
                    Phase: "idle",
                    SessionId: null,
                    Storage: null,
                    SyncedCount: 1,
                    LibraryCount: 2,
                    LatestSuccessfulSync: null,
                    LatestAttempt: null,
                    LastTerminalError: null,
                    SelectionRevision: 1,
                    SettingsRevision: 1,
                    SubscriptionsRevision: 1)
            ]));
        await Task.Delay(50);

        Assert.NotNull(received);
        Assert.Equal(3UL, received!.Revision);
        Assert.Single(received.Devices);
        Assert.Equal("A", received.Devices[0].Identity.Serial);
        router.Stop();
    }
}
