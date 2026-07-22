using System;
using Classick_UI.Ipc;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel
{
    public SyncEventContext? ActiveSyncContext { get; private set; }
    public string? DisplayedDeviceSerial { get; private set; }
    public DeviceSessionTarget? ActiveDeviceSession { get; private set; }
    public DeviceId? DisplayedDeviceId { get; private set; }
    private SyncEventContext? _promptContext;

    public void SetActiveSyncSession(SyncEventContext context)
    {
        ArgumentNullException.ThrowIfNull(context);
        if (!context.IsDeviceSession)
        {
            throw new ArgumentException("A popover sync session must identify a device.", nameof(context));
        }
        if (ActiveSyncContext == context)
        {
            return;
        }

        ResetSyncProgress();
        ActiveSyncContext = context;
        DisplayedDeviceSerial = context.Serial;
        OnPropertyChanged(nameof(ShowSyncControls));
    }

    public void ClearActiveSyncSession()
    {
        ResetSyncProgress();
        ActiveSyncContext = null;
        OnPropertyChanged(nameof(ShowSyncControls));
    }

    public void SetActiveDeviceSession(DeviceSessionTarget target)
    {
        ArgumentNullException.ThrowIfNull(target);
        if (ActiveDeviceSession == target) return;
        ResetSyncProgress();
        ActiveDeviceSession = target;
        DisplayedDeviceId = target.DeviceId;
        OnPropertyChanged(nameof(ShowSyncControls));
    }

    public void ClearActiveDeviceSession()
    {
        ResetSyncProgress();
        ActiveDeviceSession = null;
        OnPropertyChanged(nameof(ShowSyncControls));
    }

    public void ClearDisplayedDevice()
    {
        ClearActiveDeviceSession();
        DisplayedDeviceId = null;
        IpodConnected = false;
        Syncing = false;
        FinishingSync = false;
        Paused = false;
        DeviceLabel = "iPod";
        StatusText = "iPod not connected";
    }

    public void ApplyWireProgress(RoutedSyncEvent routed)
    {
        ArgumentNullException.ThrowIfNull(routed);
        if (routed.DeviceId is not { } deviceId ||
            ActiveDeviceSession != new DeviceSessionTarget(deviceId, routed.SessionId))
        {
            return;
        }

        // W5 owns presentation reduction. W2 only establishes the typed,
        // device-and-session routing boundary used by controls and focus.
    }

    public WireCancelSyncCommand? CreateWireCancelSyncCommand(string requestId) =>
        CanControlActiveDeviceSync && ActiveDeviceSession is { } target
            ? new WireCancelSyncCommand(target.DeviceId, target.SessionId, requestId)
            : null;

    public PauseSyncCommand? CreateWirePauseSyncCommand(string requestId) =>
        CanControlActiveDeviceSync && ActiveDeviceSession is { } target
            ? new PauseSyncCommand(target.DeviceId, target.SessionId, requestId)
            : null;

    public PromptDecisionCommand? CreateWirePromptDecisionCommand(int choice, string requestId)
    {
        return CanControlActiveDeviceSync && ActiveDeviceSession is { } target && PromptActive
            ? new PromptDecisionCommand(target.DeviceId, target.SessionId, requestId, PromptId, checked((uint)choice))
            : null;
    }

    public WireTriggerSyncCommand? CreateWireTriggerSyncCommand(string requestId)
    {
        return ShowSyncNowButton && DisplayedDeviceId is { } deviceId
            ? new WireTriggerSyncCommand(deviceId, requestId, SyncTrigger.Manual)
            : null;
    }

    public void ApplySyncProgress(RoutedSyncEvent routed)
    {
        ArgumentNullException.ThrowIfNull(routed);
        if (ActiveSyncContext != routed.Context)
        {
            return;
        }

        if (FinishingSync && routed.Event is PromptEvent)
        {
            return;
        }

        if (routed.Event is PromptEvent)
        {
            _promptContext = routed.Context;
        }
        ApplyIpcProgress(routed.Event);
    }

    public CancelSyncCommand? CreateCancelSyncCommand(string requestId)
    {
        return CanControlActiveSync && ActiveSyncContext?.Serial is { } serial
            ? new CancelSyncCommand(serial, requestId)
            : null;
    }

    public TriggerSyncCommand? CreateTriggerSyncCommand(string? fallbackSerial, string requestId)
    {
        if (!ShowSyncNowButton)
        {
            return null;
        }

        var serial = DisplayedDeviceSerial ?? fallbackSerial;
        return serial is not null
            ? new TriggerSyncCommand("manual", serial, requestId)
            : null;
    }

    public PauseCommand? CreatePauseCommand(string requestId)
    {
        return CanControlActiveSync && ActiveSyncContext?.Serial is { } serial
            ? new PauseCommand(serial, requestId)
            : null;
    }

    public DecidePromptCommand? CreatePromptDecisionCommand(int choice, string requestId)
    {
        if (!CanControlActiveSync ||
            _promptContext != ActiveSyncContext ||
            ActiveSyncContext?.Serial is not { } serial ||
            !PromptActive)
        {
            return null;
        }

        return new DecidePromptCommand(PromptId, choice, serial, requestId);
    }

    public void ClearPrompt()
    {
        _promptContext = null;
        PromptActive = false;
        PromptMessage = "";
        PromptId = 0;
        PromptOptions.Clear();
    }

    private void ResetSyncProgress()
    {
        ProgressCurrent = 0;
        ProgressTotal = 0;
        CurrentTrackLabel = "";
        SyncStartedAt = null;
        ClearPrompt();
    }
}
