using IpodSync_UI.Ipc;
using IpodSync_UI.Notifications;
using Xunit;

public class NotificationServiceTests
{
    private static StatusUpdateEvent Status(string state, string? errorMessage = null)
    {
        var lastSync = errorMessage is null
            ? new HistoryEntry("2026-05-25T10:00:00Z", 5, "plug_in", "ok", null,
                new SyncSummary(1, 0, 0, 0, 0))
            : new HistoryEntry("2026-05-25T10:00:00Z", 5, "plug_in", "error", errorMessage, null);
        return new StatusUpdateEvent(state, true, true, lastSync, null);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_ok_outcome_fires_complete_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Complete, decision!.Kind);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_error_outcome_fires_error_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing",
            newStatus: Status("idle", errorMessage: "Source unreachable"),
            notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Error, decision!.Kind);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_ok_does_not_fire_on_errors_only()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "errors_only");
        Assert.Null(decision);
    }

    [Fact]
    public void DecideToast_idle_after_syncing_with_error_fires_on_errors_only()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing",
            newStatus: Status("idle", errorMessage: "Source unreachable"),
            notifyOn: "errors_only");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Error, decision!.Kind);
    }

    [Fact]
    public void DecideToast_anything_returns_null_when_notify_on_none()
    {
        var decision = NotificationService.DecideToast(
            previousState: "syncing", newStatus: Status("idle"), notifyOn: "none");
        Assert.Null(decision);
    }

    [Fact]
    public void DecideToast_syncing_after_idle_fires_started_on_all()
    {
        var decision = NotificationService.DecideToast(
            previousState: "idle", newStatus: Status("syncing"), notifyOn: "all");
        Assert.NotNull(decision);
        Assert.Equal(ToastKind.Started, decision!.Kind);
    }

    [Fact]
    public void DecideToast_no_transition_returns_null()
    {
        var decision = NotificationService.DecideToast(
            previousState: "idle", newStatus: Status("idle"), notifyOn: "all");
        Assert.Null(decision);
    }
}
