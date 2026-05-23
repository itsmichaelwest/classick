using System;
using System.Diagnostics;
using IpodSync_UI.Ipc;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace IpodSync_UI.Notifications;

/// <summary>
/// Fires Windows toast notifications via AppNotificationManager when
/// daemon StatusUpdate events report a sync state transition. Filter
/// honors the user's notify_on config (all / errors_only / none).
///
/// The pure <see cref="DecideToast"/> matrix lives in the sibling
/// <c>NotificationDecision.cs</c> partial so plain net10.0 tests can
/// exercise it without WindowsAppSDK references.
/// </summary>
public sealed partial class NotificationService : IDisposable
{
    private readonly DaemonEventRouter _router;
    private readonly Func<string> _getNotifyOn;
    private string _previousState = "idle";
    private bool _registered;

    public NotificationService(DaemonEventRouter router, Func<string> getNotifyOn)
    {
        _router = router;
        _getNotifyOn = getNotifyOn;
    }

    public void Initialize()
    {
        if (!_registered)
        {
            // Packaged WinUI apps get AUMID from manifest automatically.
            // Register is idempotent but logs on duplicate calls.
            try { AppNotificationManager.Default.Register(); _registered = true; }
            catch (Exception e) { Debug.WriteLine($"notify: register failed: {e.Message}"); }
        }
        _router.StatusUpdated += OnStatusUpdated;
    }

    private void OnStatusUpdated(StatusUpdateEvent s)
    {
        var decision = DecideToast(_previousState, s, _getNotifyOn());
        _previousState = s.State;
        if (decision is null) return;
        FireToast(decision);
    }

    private void FireToast(ToastDecision d)
    {
        try
        {
            var builder = new AppNotificationBuilder()
                .AddText(d.Title)
                .AddText(d.Body);
            AppNotificationManager.Default.Show(builder.BuildNotification());
        }
        catch (Exception e)
        {
            Debug.WriteLine($"notify: toast fire failed: {e.Message}");
        }
    }

    public void Dispose()
    {
        _router.StatusUpdated -= OnStatusUpdated;
    }
}
