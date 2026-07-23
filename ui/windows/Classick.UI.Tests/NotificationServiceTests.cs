using Classick_UI.Ipc;
using Classick_UI.Notifications;

namespace Classick_UI.Tests;

public sealed class NotificationServiceTests
{
    private static readonly DeviceId Device = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void TypedSessionEvents_IncludeDeviceNameWithoutDisplayingRawId()
    {
        var tracker = new NotificationDecisionTracker();

        var started = tracker.Reduce(Accepted(), _ => "Michael's iPod", "all");
        var completed = tracker.Reduce(Finished(success: true), _ => "Michael's iPod", "all");

        Assert.Equal(ToastKind.Started, started!.Kind);
        Assert.Contains("Michael's iPod", started.Body);
        Assert.Equal(ToastKind.Complete, completed!.Kind);
        Assert.Contains("Michael's iPod", completed.Body);
        Assert.DoesNotContain(Device.Value, started.Body + completed.Body);
    }

    [Fact]
    public void DeviceAndSession_DeduplicateRepeatedTerminalEventsIndependently()
    {
        var tracker = new NotificationDecisionTracker();

        Assert.NotNull(tracker.Reduce(Finished(success: true), _ => "iPod", "all"));
        Assert.Null(tracker.Reduce(Finished(success: true), _ => "iPod", "all"));
        Assert.NotNull(tracker.Reduce(Finished(success: true, sessionId: 8), _ => "iPod", "all"));
    }

    [Fact]
    public void ErrorsOnly_SuppressesStartAndSuccessAndKeepsFailureDetailPrivate()
    {
        var tracker = new NotificationDecisionTracker();

        Assert.Null(tracker.Reduce(Accepted(), _ => "Work iPod", "errors_only"));
        Assert.Null(tracker.Reduce(Finished(success: true), _ => "Work iPod", "errors_only"));
        tracker.Reduce(new SyncErrorEvent(Device, 8, "Source unavailable"), _ => "Work iPod", "errors_only");
        var failed = tracker.Reduce(Finished(success: false, sessionId: 8), _ => "Work iPod", "errors_only");

        Assert.Equal(ToastKind.Error, failed!.Kind);
        Assert.Equal("Work iPod could not be synced. Open Classick for details.", failed.Body);
        Assert.DoesNotContain("Source unavailable", failed.Body);
    }

    [Fact]
    public void RawIdResolverResult_FallsBackToGenericName()
    {
        var tracker = new NotificationDecisionTracker();

        var decision = tracker.Reduce(Accepted(), _ => Device.Value, "all");

        Assert.Equal("Syncing iPod…", decision!.Body);
        Assert.DoesNotContain(Device.Value, decision.Body);
    }

    private static SyncAcceptedEvent Accepted() =>
        new(Device, 7, "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8801", SyncOperation.Sync);

    private static SyncFinishedEvent Finished(bool success, ulong sessionId = 7) =>
        new(Device, sessionId, success);
}
