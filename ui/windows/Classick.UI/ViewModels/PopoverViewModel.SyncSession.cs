using System;
using Classick_UI.Ipc;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel
{
    public SyncEventContext? ActiveSyncContext { get; private set; }
    public string? DisplayedDeviceSerial { get; private set; }
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
