using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel
{
    private string? _pendingSourceRetryRequestId;

    [ObservableProperty]
    private bool sourceAttentionVisible;

    [ObservableProperty]
    private bool sourceRemounting;

    [ObservableProperty]
    private bool sourceRetryPending;

    [ObservableProperty]
    private string? availableSourceRoot;

    public string SourceAttentionTitle => "Music share needs attention";

    public string SourceAttentionMessage =>
        "Connect to the music share, then Classick will resume automatically.";

    public bool ShowSourceRecovery => !PromptActive &&
        (SourceAttentionVisible || SourceRemounting);

    public bool SourceRetryAvailable => SourceAttentionVisible && !SourceRetryPending;

    partial void OnSourceAttentionVisibleChanged(bool value)
    {
        NotifySourceRecoveryPresentationChanged();
    }

    partial void OnSourceRemountingChanged(bool value)
    {
        NotifySourceRecoveryPresentationChanged();
    }

    partial void OnSourceRetryPendingChanged(bool value)
    {
        OnPropertyChanged(nameof(SourceRetryAvailable));
    }

    public void ApplySourceAvailability(SourceAvailabilityEvent availability)
    {
        ArgumentNullException.ThrowIfNull(availability);

        if (availability.AcknowledgedRequestId is { } requestId &&
            _pendingSourceRetryRequestId is { } pendingRequestId)
        {
            if (!string.Equals(requestId, pendingRequestId, StringComparison.Ordinal))
            {
                return;
            }

            ClearPendingSourceRetry();
        }
        else if (availability.AcknowledgedRequestId is null &&
                 availability.State != SourceAvailabilityState.Remounting)
        {
            ClearPendingSourceRetry();
        }

        SourceAttentionVisible = availability.State is
            SourceAvailabilityState.AuthRequired or SourceAvailabilityState.Unavailable;
        SourceRemounting = availability.State == SourceAvailabilityState.Remounting;

        if (availability.State == SourceAvailabilityState.Available)
        {
            AvailableSourceRoot = availability.SourceRoot;
        }
    }

    public void ApplySourceAvailability(WireSourceAvailabilityEvent availability)
    {
        ArgumentNullException.ThrowIfNull(availability);
        if (availability.RequestId is { } requestId &&
            _pendingSourceRetryRequestId is { } pendingRequestId)
        {
            if (!string.Equals(requestId, pendingRequestId, StringComparison.Ordinal)) return;
            ClearPendingSourceRetry();
        }
        else if (availability.RequestId is null &&
                 availability.State != SourceAvailabilityState.Remounting)
        {
            ClearPendingSourceRetry();
        }
        SourceAttentionVisible = availability.State is
            SourceAvailabilityState.AuthRequired or SourceAvailabilityState.Unavailable;
        SourceRemounting = availability.State == SourceAvailabilityState.Remounting;
        if (availability.State == SourceAvailabilityState.Available)
        {
            AvailableSourceRoot = availability.SourceRoot;
        }
    }

    public WireRetrySourceMountCommand? CreateWireSourceRetryCommand(string requestId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(requestId);
        if (!SourceAttentionVisible || _pendingSourceRetryRequestId is not null) return null;
        _pendingSourceRetryRequestId = requestId;
        SourceRetryPending = true;
        return new WireRetrySourceMountCommand(requestId, AllowUi: true);
    }

    public RetrySourceMountCommand? CreateSourceRetryCommand(string requestId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(requestId);
        if (!SourceAttentionVisible || _pendingSourceRetryRequestId is not null)
        {
            return null;
        }

        _pendingSourceRetryRequestId = requestId;
        SourceRetryPending = true;
        return new RetrySourceMountCommand(AllowUi: true, RequestId: requestId);
    }

    public void SourceRetrySendFailed(string requestId)
    {
        if (string.Equals(
                requestId,
                _pendingSourceRetryRequestId,
                StringComparison.Ordinal))
        {
            ClearPendingSourceRetry();
        }
    }

    private void ClearPendingSourceRetry()
    {
        _pendingSourceRetryRequestId = null;
        SourceRetryPending = false;
    }

    private void NotifySourceRecoveryPresentationChanged()
    {
        OnPropertyChanged(nameof(ShowSourceRecovery));
        OnPropertyChanged(nameof(SourceRetryAvailable));
        OnPropertyChanged(nameof(ShowConnectedContent));
        OnPropertyChanged(nameof(ShowEmptyState));
        OnPropertyChanged(nameof(ShowSyncNowButton));
        OnPropertyChanged(nameof(ShowSyncControls));
    }
}
