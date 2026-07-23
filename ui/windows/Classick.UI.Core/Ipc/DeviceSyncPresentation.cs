namespace Classick_UI.Ipc;

public enum DeviceSyncPhase
{
    Preparing,
    Syncing,
    AwaitingReview,
    AwaitingPrompt,
    Finalizing,
    Paused,
    Cancelled,
    Failed,
    Finished,
}

public enum SyncInteractionKind
{
    Prompt,
    Form,
    Review,
}

public sealed record SyncInteraction(
    DeviceSessionTarget Owner,
    SyncInteractionKind Kind,
    ulong PromptId,
    string Message,
    IReadOnlyList<string> Options,
    string Initial = "",
    string Hint = "",
    bool NoDelete = false);

public sealed class DeviceSyncPresentation
{
    public DeviceSyncPresentation(DeviceSessionTarget target)
    {
        Target = target;
    }

    public DeviceSessionTarget Target { get; }
    public DeviceSyncPhase Phase { get; private set; } = DeviceSyncPhase.Preparing;
    public ulong Current { get; private set; }
    public ulong Total { get; private set; }
    public string CurrentTrack { get; private set; } = "";
    public ulong? EtaSeconds { get; private set; }
    public DateTimeOffset? PlanStartedAt { get; private set; }
    public SyncInteraction? Interaction { get; private set; }
    public string? ErrorMessage { get; private set; }
    public SyncFinishedEvent? Finished { get; private set; }
    public ulong Revision { get; private set; }

    public void Apply(WireEvent wireEvent)
    {
        if (wireEvent is not ISessionRoutedMessage routed ||
            routed.DeviceId != Target.DeviceId || routed.SessionId != Target.SessionId)
        {
            throw new ArgumentException("Progress does not belong to this device session.", nameof(wireEvent));
        }

        switch (wireEvent)
        {
            case SyncAcceptedEvent:
            case RunHeaderEvent:
                Phase = DeviceSyncPhase.Preparing;
                break;
            case SyncSummaryEvent summary:
                Phase = DeviceSyncPhase.Syncing;
                Current = 0;
                Total = summary.Summary.TotalPlanned;
                CurrentTrack = "";
                EtaSeconds = null;
                PlanStartedAt ??= DateTimeOffset.Now;
                Interaction = null;
                break;
            case ReviewRequestedEvent review:
                Phase = DeviceSyncPhase.AwaitingReview;
                Total = review.Summary.TotalPlanned;
                Interaction = new SyncInteraction(
                    Target,
                    SyncInteractionKind.Review,
                    0,
                    $"Review changes: {review.Summary.Add} add, {review.Summary.Modify} replace, " +
                    $"{review.Summary.MetadataOnly} metadata, {review.Summary.Remove} remove, " +
                    $"{review.Summary.Unchanged} unchanged.",
                    ["Apply changes", "Apply without deleting", "Dry run", "Cancel"],
                    NoDelete: review.NoDelete);
                break;
            case WirePromptEvent prompt:
                Phase = DeviceSyncPhase.AwaitingPrompt;
                Interaction = new SyncInteraction(
                    Target,
                    SyncInteractionKind.Prompt,
                    prompt.PromptId,
                    prompt.Message,
                    prompt.Options);
                break;
            case WireFormEvent form:
                Phase = DeviceSyncPhase.AwaitingPrompt;
                Interaction = new SyncInteraction(
                    Target,
                    SyncInteractionKind.Form,
                    form.PromptId,
                    form.Label,
                    ["Submit"],
                    form.Initial,
                    form.Hint);
                break;
            case WireTrackStartEvent track:
                Phase = DeviceSyncPhase.Syncing;
                Current = track.Current;
                Total = track.Total;
                CurrentTrack = track.Label;
                EtaSeconds = track.EtaSecs;
                Interaction = null;
                break;
            case WireFinalizingEvent:
                Phase = DeviceSyncPhase.Finalizing;
                EtaSeconds = null;
                Interaction = null;
                break;
            case SyncPausedEvent:
                Phase = DeviceSyncPhase.Paused;
                Interaction = null;
                break;
            case SyncCancelledEvent:
                Phase = DeviceSyncPhase.Cancelled;
                Interaction = null;
                break;
            case SyncErrorEvent error:
                Phase = DeviceSyncPhase.Failed;
                ErrorMessage = error.Message;
                Interaction = null;
                break;
            case SyncFinishedEvent finished:
                Phase = finished.Success ? DeviceSyncPhase.Finished : DeviceSyncPhase.Failed;
                Finished = finished;
                Interaction = null;
                break;
        }
        Revision++;
    }

}
