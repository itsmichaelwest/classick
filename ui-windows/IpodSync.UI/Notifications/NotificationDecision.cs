using IpodSync_UI.Ipc;

namespace IpodSync_UI.Notifications;

public enum ToastKind { Started, Complete, Error }

public sealed record ToastDecision(ToastKind Kind, string Title, string Body);

/// <summary>
/// Pure decision logic split out so the test project (plain net10.0,
/// no WindowsAppSDK reference) can exercise the matrix without dragging
/// the AppNotificationManager surface into the test runtime. The full
/// <see cref="NotificationService"/> partial in the sibling file owns
/// the lifecycle, router subscription, and the actual toast emission.
/// </summary>
public sealed partial class NotificationService
{
    /// <summary>
    /// Pure decision function (no AppNotificationManager dependency) so
    /// tests can exercise the matrix without a packaged-app fixture.
    /// </summary>
    public static ToastDecision? DecideToast(
        string previousState, StatusUpdateEvent newStatus, string notifyOn)
    {
        if (notifyOn == "none") return null;
        // Only act on transitions, not repeated broadcasts of the same state.
        if (previousState == newStatus.State) return null;

        // syncing -> idle: completion (ok or error).
        if (previousState == "syncing" && newStatus.State == "idle")
        {
            var outcome = newStatus.LastSync?.Outcome ?? "ok";
            if (outcome == "ok")
            {
                if (notifyOn == "errors_only") return null;
                var summary = newStatus.LastSync?.Summary;
                var body = summary is null
                    ? "Sync complete."
                    : $"Sync complete: +{summary.Add} ~{summary.Modify} -{summary.Remove}"
                      + (summary.Skipped > 0 ? $", {summary.Skipped} skipped" : "");
                return new ToastDecision(ToastKind.Complete, "ipod-sync", body);
            }
            else
            {
                var msg = newStatus.LastSync?.ErrorMessage ?? "Sync failed.";
                return new ToastDecision(ToastKind.Error, "ipod-sync — sync failed", msg);
            }
        }

        // idle -> syncing: starting.
        if (previousState == "idle" && newStatus.State == "syncing")
        {
            if (notifyOn == "errors_only") return null;
            return new ToastDecision(ToastKind.Started, "ipod-sync", "Syncing iPod…");
        }

        return null;
    }
}
