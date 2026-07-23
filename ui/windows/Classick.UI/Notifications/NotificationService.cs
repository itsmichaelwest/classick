using System;
using System.Diagnostics;
using Classick_UI.Ipc;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace Classick_UI.Notifications;

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
    private readonly Func<DeviceId, string> _resolveDeviceName;
    private readonly NotificationDecisionTracker _decisions = new();
    private bool _registered;
    private bool _subscribed;

    public NotificationService(
        DaemonEventRouter router,
        Func<string> getNotifyOn,
        Func<DeviceId, string> resolveDeviceName)
    {
        _router = router;
        _getNotifyOn = getNotifyOn;
        _resolveDeviceName = resolveDeviceName;
    }

    public void Initialize()
    {
        if (!_registered)
        {
            // Packaged WinUI apps get AUMID from manifest automatically.
            // Unpackaged Debug builds throw "No COM servers are registered
            // for this app" — caught here so the app still starts; the
            // _registered flag stays false so we don't spam toast attempts
            // (which would each throw inside Show()) on every status update.
            try { AppNotificationManager.Default.Register(); _registered = true; }
            catch (Exception e) { Debug.WriteLine($"notify: register failed (toasts disabled): {e.Message}"); }
        }
        if (!_subscribed)
        {
            _router.EventReceived += OnWireEvent;
            _subscribed = true;
        }
    }

    private void OnWireEvent(WireEvent wireEvent)
    {
        var decision = _decisions.Reduce(wireEvent, _resolveDeviceName, _getNotifyOn());
        if (decision is null) return;
        // Skip firing if registration failed — Show() would throw inside
        // the WinAppSDK projection and log a stack trace per transition.
        if (!_registered) return;
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
        if (_subscribed)
        {
            _router.EventReceived -= OnWireEvent;
            _subscribed = false;
        }
    }
}
