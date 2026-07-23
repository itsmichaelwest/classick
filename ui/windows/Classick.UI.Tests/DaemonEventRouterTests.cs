using System.Threading.Channels;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public class DaemonEventRouterTests
{
    private static readonly DeviceId Device = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void Route_DirectTypedProgress_RequiresCurrentInventorySession()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<RoutedSyncEvent>();
        router.SyncEventReceived += received.Add;
        router.Route(Inventory(1, 42));

        router.Route(new WireTrackDoneEvent(Device, 41, TrackResult.Applied));
        router.Route(new WireTrackDoneEvent(Device, 42, TrackResult.Applied));

        var routed = Assert.Single(received);
        Assert.Equal(Device, routed.DeviceId);
        Assert.Equal((ulong)42, routed.SessionId);
        Assert.IsType<WireTrackDoneEvent>(routed.Event);
    }

    [Fact]
    public void Route_StaleProgress_IsNotPublishedOnGeneralEventStream()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<WireEvent>();
        router.EventReceived += received.Add;
        router.Route(Inventory(1, 42));

        router.Route(new SyncLogEvent(Device, 41, "stale"));

        Assert.DoesNotContain(received, item => item is SyncLogEvent);
    }

    [Fact]
    public void Route_SyncAcceptedEstablishesSessionBeforeNextInventory()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        RoutedSyncEvent? received = null;
        router.SyncEventReceived += progress => received = progress;

        router.Route(new SyncAcceptedEvent(
            Device,
            51,
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8803",
            SyncOperation.Sync));
        router.Route(new SyncLogEvent(Device, 51, "Starting"));

        Assert.NotNull(received);
        Assert.Equal((ulong)51, received.SessionId);
    }

    [Fact]
    public void Route_StaleInventory_DoesNotReplaceCurrentSession()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<RoutedSyncEvent>();
        router.SyncEventReceived += received.Add;
        router.Route(Inventory(2, 62));
        router.Route(Inventory(1, 61));

        router.Route(new SyncLogEvent(Device, 61, "stale"));
        router.Route(new SyncLogEvent(Device, 62, "current"));

        var routed = Assert.Single(received);
        Assert.Equal("current", Assert.IsType<SyncLogEvent>(routed.Event).Message);
    }

    [Fact]
    public void Route_PausedInventoryRetainsRouteUntilFinishedThenRejectsLateProgress()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<RoutedSyncEvent>();
        router.SyncEventReceived += received.Add;
        router.Route(Inventory(1, 72));
        router.Route(new SyncPausedEvent(Device, 72));
        router.Route(IdleInventory(2));

        router.Route(new SyncFinishedEvent(Device, 72, true));
        router.Route(new SyncLogEvent(Device, 72, "late"));

        Assert.Collection(
            received,
            item => Assert.IsType<SyncPausedEvent>(item.Event),
            item => Assert.IsType<SyncFinishedEvent>(item.Event));
    }

    [Fact]
    public async Task Start_RoutesChannelEventsInOrder()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<Type>();
        var completed = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        router.EventReceived += wireEvent =>
        {
            received.Add(wireEvent.GetType());
            if (received.Count == 3) completed.SetResult();
        };
        router.Start();

        await channel.Writer.WriteAsync(Inventory(1, 70));
        await channel.Writer.WriteAsync(new SyncLogEvent(Device, 70, "one"));
        await channel.Writer.WriteAsync(new WireTrackDoneEvent(Device, 70, TrackResult.Applied));
        await completed.Task.WaitAsync(TimeSpan.FromSeconds(2));

        Assert.Equal(
            [typeof(DeviceInventoryEvent), typeof(SyncLogEvent), typeof(WireTrackDoneEvent)],
            received);
    }

    [Fact]
    public async Task Start_RoutesLegacyAppEventsAndStopWaitsForReader()
    {
        var channel = Channel.CreateUnbounded<object>();
        using var router = new DaemonEventRouter(channel.Reader);
        ConfigUpdateEvent? received = null;
        var completed = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        router.ConfigUpdated += config =>
        {
            received = config;
            completed.SetResult();
        };
        router.Start();

        await channel.Writer.WriteAsync(new ConfigUpdateEvent(null, null, null, 4));
        await completed.Task.WaitAsync(TimeSpan.FromSeconds(2));
        await router.StopAsync();

        Assert.NotNull(received);
        Assert.Equal((ulong)4, received.ConfigRevision);
    }

    private static DeviceInventoryEvent Inventory(ulong revision, ulong sessionId) =>
        new(
            null,
            revision,
            [new IdentifiedDeviceSnapshot(
                Device,
                "iPod",
                DeviceReadiness.Ready,
                new HardwareFacts(),
                ProfileStatus.Adopted,
                true,
                "/Volumes/iPod",
                DevicePhase.Syncing,
                sessionId,
                null,
                0,
                null,
                null)],
            []);

    private static DeviceInventoryEvent IdleInventory(ulong revision) =>
        new(
            null,
            revision,
            [new IdentifiedDeviceSnapshot(
                Device,
                "iPod",
                DeviceReadiness.Ready,
                new HardwareFacts(),
                ProfileStatus.Adopted,
                true,
                "/Volumes/iPod",
                DevicePhase.Idle,
                null,
                null,
                0,
                null,
                null)],
            []);
}
