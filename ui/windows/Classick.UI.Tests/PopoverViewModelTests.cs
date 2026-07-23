using Classick_UI.Ipc;
using Classick_UI.ViewModels;

namespace Classick_UI.Tests;

public sealed class PopoverViewModelTests
{
    private static readonly DeviceId DeviceId = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void TypedSnapshotDrivesIdentityReadinessAndStorage()
    {
        var viewModel = new PopoverViewModel();

        viewModel.Update(Device(storage: new StorageSnapshot(1_000, 400, StorageFreshness.Live)));

        Assert.Equal("Michael's iPod", viewModel.DeviceLabel);
        Assert.True(viewModel.IpodConnected);
        Assert.True(viewModel.DeviceReadyForSync);
        Assert.True(viewModel.HasStorage);
        Assert.True(viewModel.ShowSyncNowButton);
    }

    [Fact]
    public void AppleInitializationReadinessOverridesSyncAffordance()
    {
        var viewModel = new PopoverViewModel();

        viewModel.Update(Device(
            readiness: DeviceReadiness.NeedsAppleInitialization,
            profile: ProfileStatus.NotAdopted));

        Assert.False(viewModel.ShowSyncNowButton);
        Assert.Contains("Apple setup required", viewModel.DeviceReadinessText);
        Assert.Contains("does not initialize", viewModel.DeviceGuidance);
    }

    [Fact]
    public void ControlsCaptureTheDisplayedDeviceAndActiveSession()
    {
        var viewModel = ActiveViewModel(42);

        var cancel = viewModel.CreateWireCancelSyncCommand(Request(1));
        var pause = viewModel.CreateWirePauseSyncCommand(Request(2));

        Assert.Equal(DeviceId, cancel!.DeviceId);
        Assert.Equal((ulong)42, cancel.SessionId);
        Assert.Equal(DeviceId, pause!.DeviceId);
        Assert.Equal((ulong)42, pause.SessionId);
    }

    [Fact]
    public void PromptDecisionUsesImmutablePresentationOwner()
    {
        var viewModel = ActiveViewModel(42);
        var presentation = Presentation(42);
        presentation.Apply(new WirePromptEvent(DeviceId, 42, 7, "Choose", ["Continue", "Abort"]));
        viewModel.ApplySyncPresentation(presentation);

        var captured = viewModel.CreateWireInteractionCommand(1, Request(3));
        viewModel.SetActiveDeviceSession(new DeviceSessionTarget(DeviceId, 43));

        var command = Assert.IsType<PromptDecisionCommand>(captured);
        Assert.Equal((ulong)42, command.SessionId);
        Assert.Equal((ulong)7, command.PromptId);
        Assert.Null(viewModel.CreateWireInteractionCommand(0, Request(4)));
    }

    [Fact]
    public void ReviewAndFormProduceTypedSessionCommands()
    {
        var reviewViewModel = ActiveViewModel(42);
        var review = Presentation(42);
        review.Apply(new ReviewRequestedEvent(
            DeviceId,
            42,
            new WireActionPlanSummary(1, 2, 3, 4, 5, 10),
            true));
        reviewViewModel.ApplySyncPresentation(review);

        Assert.Contains("1 add", reviewViewModel.PromptMessage);
        Assert.Contains("4 remove", reviewViewModel.PromptMessage);
        var apply = Assert.IsType<ApplyReviewCommand>(
            reviewViewModel.CreateWireInteractionCommand(1, Request(5)));
        Assert.True(apply.NoDelete);

        var deleteViewModel = ActiveViewModel(44);
        var protectedReview = Presentation(44);
        protectedReview.Apply(new ReviewRequestedEvent(
            DeviceId,
            44,
            new WireActionPlanSummary(1, 0, 0, 1, 0, 2),
            true));
        deleteViewModel.ApplySyncPresentation(protectedReview);
        var applyWithDeletes = Assert.IsType<ApplyReviewCommand>(
            deleteViewModel.CreateWireInteractionCommand(0, Request(7)));
        Assert.False(applyWithDeletes.NoDelete);

        var formViewModel = ActiveViewModel(43);
        var form = Presentation(43);
        form.Apply(new WireFormEvent(DeviceId, 43, 9, "Library name", "Classic", "Enter a name"));
        formViewModel.ApplySyncPresentation(form);
        formViewModel.PromptInput = "Road trip";
        var submit = Assert.IsType<FormDecisionCommand>(
            formViewModel.CreateWireInteractionCommand(0, Request(6)));
        Assert.Equal("Road trip", submit.Value);
    }

    [Fact]
    public void TrackProgressUsesDaemonEtaAndClearsPrompt()
    {
        var viewModel = ActiveViewModel(42);
        var presentation = Presentation(42);
        presentation.Apply(new WirePromptEvent(DeviceId, 42, 7, "Choose", ["Continue"]));
        presentation.Apply(new WireTrackStartEvent(DeviceId, 42, 4, 20, "track.flac", 120));

        viewModel.ApplySyncPresentation(presentation);

        Assert.False(viewModel.PromptActive);
        Assert.Equal("Syncing 4 of 20 tracks", viewModel.ProgressCaption);
        Assert.Equal("about 2 min left", viewModel.EtaLabel);
    }

    [Fact]
    public void FinalizingClearsDaemonEta()
    {
        var viewModel = ActiveViewModel(42);
        var presentation = Presentation(42);
        presentation.Apply(new WireTrackStartEvent(DeviceId, 42, 4, 20, "track.flac", 120));
        viewModel.ApplySyncPresentation(presentation);
        Assert.NotEmpty(viewModel.EtaLabel);

        presentation.Apply(new WireFinalizingEvent(DeviceId, 42, StopReason.Cancelled, 2, 18));
        viewModel.ApplySyncPresentation(presentation);

        Assert.Empty(viewModel.EtaLabel);
    }

    [Fact]
    public void FinalizingDisablesControlsAndTerminalSummaryRemainsVisible()
    {
        var viewModel = ActiveViewModel(42);
        var presentation = Presentation(42);
        presentation.Apply(new WireFinalizingEvent(DeviceId, 42, StopReason.Cancelled, 2, 18));
        viewModel.ApplySyncPresentation(presentation);

        Assert.True(viewModel.FinishingSync);
        Assert.False(viewModel.ShowSyncControls);
        Assert.Equal("Finalizing safely…", viewModel.StatusText);

        presentation.Apply(new SyncFinishedEvent(
            DeviceId,
            42,
            true,
            new SkippedForSpaceSummary(1, 3, 2_048)));
        viewModel.ApplySyncPresentation(presentation);
        Assert.False(viewModel.Syncing);
        Assert.Equal("Sync complete · 3 tracks skipped for space", viewModel.StatusText);
    }

    [Fact]
    public void CorrelatedCommandFailureReenablesPendingInteraction()
    {
        var viewModel = ActiveViewModel(42);
        var presentation = Presentation(42);
        presentation.Apply(new WirePromptEvent(DeviceId, 42, 7, "Choose", ["Continue"]));
        viewModel.ApplySyncPresentation(presentation);
        var requestId = Request(8);
        Assert.NotNull(viewModel.CreateWireInteractionCommand(0, requestId));
        Assert.False(viewModel.InteractionDecisionEnabled);

        viewModel.InteractionCommandFailed(requestId, "Rejected");

        Assert.True(viewModel.PromptActive);
        Assert.True(viewModel.InteractionDecisionEnabled);
        Assert.Contains("Rejected", viewModel.StatusText);
    }

    [Fact]
    public void DeviceChooserNamesEveryKnownDeviceWithoutRawIds()
    {
        var store = new DeviceStore();
        var second = DeviceId.Parse("000A27002138B0A9");
        store.Reduce(new DeviceInventoryEvent(
            null,
            1,
            [Device(), Device(second, "Second iPod")],
            []));
        var viewModel = new PopoverViewModel();

        viewModel.UpdateDeviceChoices(store.Devices.Values, second);

        Assert.True(viewModel.HasMultipleDeviceChoices);
        Assert.Equal(second, viewModel.SelectedDeviceChoice!.DeviceId);
        Assert.DoesNotContain(viewModel.DeviceChoices, choice => choice.Label == choice.DeviceId.Value);
    }

    [Fact]
    public void DeviceChooserRemainsAvailableWhenFocusedRememberedDeviceIsDisconnected()
    {
        var store = new DeviceStore();
        var second = DeviceId.Parse("000A27002138B0A9");
        store.Reduce(new DeviceInventoryEvent(
            null,
            1,
            [Device() with { Connected = false, MountPath = null }, Device(second, "Second iPod")],
            []));
        var viewModel = new PopoverViewModel();
        viewModel.Update(store.Devices[DeviceId].Inventory);

        viewModel.UpdateDeviceChoices(store.Devices.Values, DeviceId);

        Assert.True(viewModel.ShowConnectedContent);
        Assert.False(viewModel.ShowEmptyState);
        Assert.False(viewModel.ShowEjectButton);
    }

    [Fact]
    public void ReapplyingSameDeviceChoicesPreservesSelectionAndItemIdentity()
    {
        var store = new DeviceStore();
        var second = DeviceId.Parse("000A27002138B0A9");
        store.Reduce(new DeviceInventoryEvent(null, 1, [Device(), Device(second, "Second iPod")], []));
        var viewModel = new PopoverViewModel();
        viewModel.UpdateDeviceChoices(store.Devices.Values, second);
        var selected = viewModel.SelectedDeviceChoice;

        viewModel.UpdateDeviceChoices(store.Devices.Values, second);

        Assert.Same(selected, viewModel.SelectedDeviceChoice);
        Assert.Same(selected, viewModel.DeviceChoices.Single(choice => choice.DeviceId == second));
    }

    [Fact]
    public void HistoryReplacesStaleSyncingLabelForSuccessAndFailure()
    {
        var viewModel = new PopoverViewModel();
        viewModel.Update(Device());
        viewModel.ApplyHistory([History(SyncOutcome.Ok)]);
        Assert.Equal("Last sync completed", viewModel.LastSyncedLabel);

        viewModel.ApplyHistory([History(SyncOutcome.Error)]);
        Assert.Equal("Last sync failed", viewModel.LastSyncedLabel);
    }

    private static PopoverViewModel ActiveViewModel(ulong sessionId)
    {
        var viewModel = new PopoverViewModel();
        viewModel.Update(Device(sessionId: sessionId));
        viewModel.SetActiveDeviceSession(new DeviceSessionTarget(DeviceId, sessionId));
        viewModel.ApplySyncPresentation(Presentation(sessionId));
        return viewModel;
    }

    private static DeviceSyncPresentation Presentation(ulong sessionId)
    {
        var presentation = new DeviceSyncPresentation(new DeviceSessionTarget(DeviceId, sessionId));
        presentation.Apply(new SyncAcceptedEvent(DeviceId, sessionId, Request(9), SyncOperation.Sync));
        return presentation;
    }

    private static IdentifiedDeviceSnapshot Device(
        DeviceId? id = null,
        string name = "Michael's iPod",
        ulong? sessionId = null,
        StorageSnapshot? storage = null,
        DeviceReadiness readiness = DeviceReadiness.Ready,
        ProfileStatus profile = ProfileStatus.Adopted) => new(
        id ?? DeviceId,
        name,
        readiness,
        new HardwareFacts(Family: new HardwareFact<IpodFamily>(
            IpodFamily.Classic,
            FactSource.Decoded,
            FactConfidence.Certain)),
        profile,
        true,
        "D:\\",
        sessionId is null ? DevicePhase.Idle : DevicePhase.Syncing,
        sessionId,
        storage,
        0,
        null,
        null);

    private static string Request(int suffix) =>
        $"018f9d7e-2f2b-7b52-9f1d-f78bdb2f88{suffix:D2}";

    private static WireHistoryEntry History(SyncOutcome outcome) => new(
        DeviceId,
        42,
        "2026-07-23T12:00:00Z",
        10,
        HistoryTrigger.Manual,
        SyncOperation.Sync,
        outcome);
}
