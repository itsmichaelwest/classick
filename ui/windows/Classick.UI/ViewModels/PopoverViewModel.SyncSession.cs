using System;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel
{
    public DeviceSessionTarget? ActiveDeviceSession { get; private set; }
    public DeviceId? DisplayedDeviceId { get; private set; }
    private SyncInteraction? _wireInteraction;
    [ObservableProperty] private bool promptIsForm;
    [ObservableProperty] private string promptInput = "";
    [ObservableProperty] private string promptHint = "";
    [ObservableProperty] private bool interactionDecisionEnabled = true;
    private string? _pendingInteractionRequestId;
    public ulong? ReportedEtaSeconds
    {
        get => _reportedEtaSeconds;
        private set
        {
            if (_reportedEtaSeconds == value) return;
            _reportedEtaSeconds = value;
            OnPropertyChanged(nameof(EtaLabel));
        }
    }
    private ulong? _reportedEtaSeconds;
    private DeviceSessionTarget? _appliedPresentationTarget;
    private ulong _appliedPresentationRevision;

    public void SetActiveDeviceSession(DeviceSessionTarget target)
    {
        ArgumentNullException.ThrowIfNull(target);
        if (ActiveDeviceSession == target) return;
        ResetSyncProgress();
        ActiveDeviceSession = target;
        _appliedPresentationTarget = null;
        DisplayedDeviceId = target.DeviceId;
        OnPropertyChanged(nameof(ShowSyncControls));
    }

    public void ClearActiveDeviceSession()
    {
        ResetSyncProgress();
        ActiveDeviceSession = null;
        _appliedPresentationTarget = null;
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
        DeviceHardwareSummary = "";
        DeviceHardwareProvenance = "";
        DeviceReadinessText = "";
        DeviceGuidance = "";
        DeviceArtworkUri = "ms-appx:///Assets/ipod-generic.svg";
        DeviceArtworkDescription = "iPod";
        DeviceReadyForSync = false;
        StatusText = "iPod not connected";
    }

    public void ApplySyncPresentation(DeviceSyncPresentation? presentation)
    {
        if (presentation is null || ActiveDeviceSession != presentation.Target) return;
        if (_appliedPresentationTarget == presentation.Target &&
            _appliedPresentationRevision == presentation.Revision) return;
        _appliedPresentationTarget = presentation.Target;
        _appliedPresentationRevision = presentation.Revision;
        _pendingInteractionRequestId = null;
        InteractionDecisionEnabled = true;

        Syncing = presentation.Phase is DeviceSyncPhase.Preparing or
            DeviceSyncPhase.Syncing or
            DeviceSyncPhase.AwaitingReview or
            DeviceSyncPhase.AwaitingPrompt or
            DeviceSyncPhase.Finalizing;
        FinishingSync = presentation.Phase == DeviceSyncPhase.Finalizing;
        Paused = presentation.Phase == DeviceSyncPhase.Paused;
        ProgressCurrent = ToProgressValue(presentation.Current);
        ProgressTotal = ToProgressValue(presentation.Total);
        CurrentTrackLabel = presentation.CurrentTrack;
        ReportedEtaSeconds = presentation.EtaSeconds;
        SyncStartedAt = presentation.PlanStartedAt;

        if (presentation.Interaction is { } interaction)
        {
            _wireInteraction = interaction;
            PromptId = interaction.PromptId;
            PromptMessage = interaction.Message;
            PromptOptions.Clear();
            foreach (var option in interaction.Options) PromptOptions.Add(option);
            PromptIsForm = interaction.Kind == SyncInteractionKind.Form;
            PromptInput = interaction.Initial;
            PromptHint = interaction.Hint;
            PromptActive = true;
        }
        else
        {
            ClearPrompt();
        }

        StatusText = presentation.Phase switch
        {
            DeviceSyncPhase.Preparing => $"Preparing {DeviceLabel}…",
            DeviceSyncPhase.Finalizing => "Finalizing safely…",
            DeviceSyncPhase.Paused => "Sync paused",
            DeviceSyncPhase.Cancelled => "Sync cancelled",
            DeviceSyncPhase.Failed => presentation.ErrorMessage is { Length: > 0 } error
                ? $"Sync failed: {error}"
                : "Sync failed",
            DeviceSyncPhase.Finished => BuildFinishedSummary(presentation.Finished),
            _ => $"Syncing {DeviceLabel}…",
        };
    }

    public WireCancelSyncCommand? CreateWireCancelSyncCommand(string requestId) =>
        CanControlActiveDeviceSync && ActiveDeviceSession is { } target
            ? new WireCancelSyncCommand(target.DeviceId, target.SessionId, requestId)
            : null;

    public PauseSyncCommand? CreateWirePauseSyncCommand(string requestId) =>
        CanControlActiveDeviceSync && ActiveDeviceSession is { } target
            ? new PauseSyncCommand(target.DeviceId, target.SessionId, requestId)
            : null;

    public WireCommand? CreateWireInteractionCommand(int choice, string requestId)
    {
        if (_wireInteraction is not { } interaction ||
            ActiveDeviceSession != interaction.Owner || !PromptActive ||
            _pendingInteractionRequestId is not null)
        {
            return null;
        }

        WireCommand? command = interaction.Kind switch
        {
            SyncInteractionKind.Prompt => new PromptDecisionCommand(
                interaction.Owner.DeviceId,
                interaction.Owner.SessionId,
                requestId,
                interaction.PromptId,
                checked((uint)choice)),
            SyncInteractionKind.Form => new FormDecisionCommand(
                interaction.Owner.DeviceId,
                interaction.Owner.SessionId,
                requestId,
                interaction.PromptId,
                PromptInput),
            SyncInteractionKind.Review when choice is 0 or 1 => new ApplyReviewCommand(
                interaction.Owner.DeviceId,
                interaction.Owner.SessionId,
                requestId,
                choice == 1),
            SyncInteractionKind.Review when choice == 2 => new DryRunReviewCommand(
                interaction.Owner.DeviceId,
                interaction.Owner.SessionId,
                requestId),
            SyncInteractionKind.Review => new QuitReviewCommand(
                interaction.Owner.DeviceId,
                interaction.Owner.SessionId,
                requestId),
            _ => null,
        };
        if (command is not null)
        {
            _pendingInteractionRequestId = requestId;
            InteractionDecisionEnabled = false;
        }
        return command;
    }

    public WireTriggerSyncCommand? CreateWireTriggerSyncCommand(string requestId)
    {
        return ShowSyncNowButton && DisplayedDeviceId is { } deviceId
            ? new WireTriggerSyncCommand(deviceId, requestId, SyncTrigger.Manual)
            : null;
    }

    public void ClearPrompt()
    {
        _wireInteraction = null;
        PromptActive = false;
        PromptIsForm = false;
        PromptInput = "";
        PromptHint = "";
        PromptMessage = "";
        PromptId = 0;
        PromptOptions.Clear();
        _pendingInteractionRequestId = null;
        InteractionDecisionEnabled = true;
    }

    public void InteractionCommandFailed(string requestId, string message)
    {
        if (!string.Equals(_pendingInteractionRequestId, requestId, StringComparison.Ordinal)) return;
        _pendingInteractionRequestId = null;
        InteractionDecisionEnabled = true;
        StatusText = $"Could not send response: {message}";
    }

    private static int ToProgressValue(ulong value) =>
        value > int.MaxValue ? int.MaxValue : (int)value;

    private static string BuildFinishedSummary(SyncFinishedEvent? finished)
    {
        if (finished is null) return "Sync complete";
        if (!finished.Success) return finished.DbRestored
            ? "Sync failed · iPod database restored"
            : "Sync failed";
        if (finished.SkippedForSpace is { Tracks: > 0 } skipped)
            return $"Sync complete · {skipped.Tracks} tracks skipped for space";
        return "Sync complete";
    }

    private void ResetSyncProgress()
    {
        ProgressCurrent = 0;
        ProgressTotal = 0;
        CurrentTrackLabel = "";
        SyncStartedAt = null;
        ReportedEtaSeconds = null;
        ClearPrompt();
    }
}
