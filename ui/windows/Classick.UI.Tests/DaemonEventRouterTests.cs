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
        var received = new List<WireEvent>();
        router.EventReceived += received.Add;
        router.Route(Inventory(1, 42));

        router.Route(new WireTrackDoneEvent(Device, 41, TrackResult.Applied));
        router.Route(new WireTrackDoneEvent(Device, 42, TrackResult.Applied));

        Assert.Collection(
            received,
            item => Assert.IsType<DeviceInventoryEvent>(item),
            item => Assert.IsType<WireTrackDoneEvent>(item));
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
        var received = new List<WireEvent>();
        router.EventReceived += received.Add;

        router.Route(new SyncAcceptedEvent(
            Device,
            51,
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8803",
            SyncOperation.Sync));
        router.Route(new SyncLogEvent(Device, 51, "Starting"));

        Assert.Contains(received, item => item is SyncLogEvent { SessionId: 51 });
    }

    [Fact]
    public void Route_StaleInventory_DoesNotReplaceCurrentSession()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<WireEvent>();
        router.EventReceived += received.Add;
        router.Route(Inventory(2, 62));
        router.Route(Inventory(1, 61));

        router.Route(new SyncLogEvent(Device, 61, "stale"));
        router.Route(new SyncLogEvent(Device, 62, "current"));

        Assert.Contains(received, item => item is SyncLogEvent { Message: "current" });
        Assert.DoesNotContain(received, item => item is SyncLogEvent { Message: "stale" });
    }

    [Fact]
    public void Route_PausedInventoryRetainsRouteUntilFinishedThenRejectsLateProgress()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        var received = new List<WireEvent>();
        router.EventReceived += received.Add;
        router.Route(Inventory(1, 72));
        router.Route(new SyncPausedEvent(Device, 72));
        router.Route(IdleInventory(2));

        router.Route(new SyncFinishedEvent(Device, 72, true));
        router.Route(new SyncLogEvent(Device, 72, "late"));

        Assert.Contains(received, item => item is SyncPausedEvent);
        Assert.Contains(received, item => item is SyncFinishedEvent);
        Assert.DoesNotContain(received, item => item is SyncLogEvent { Message: "late" });
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
    public async Task StopWaitsForTypedReader()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        using var router = new DaemonEventRouter(channel.Reader);
        GlobalConfigEvent? received = null;
        var completed = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        router.EventReceived += wireEvent =>
        {
            if (wireEvent is not GlobalConfigEvent config) return;
            received = config;
            completed.SetResult();
        };
        router.Start();

        await channel.Writer.WriteAsync(new GlobalConfigEvent(
            null,
            4,
            null,
            new GlobalSettings(SyncMode.Review, SyncMode.AutoApply, 0, NotifyLevel.All, DropSyncBehavior.Immediate)));
        await completed.Task.WaitAsync(TimeSpan.FromSeconds(2));
        await router.StopAsync();

        Assert.NotNull(received);
        Assert.Equal((ulong)4, received.Revision);
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
