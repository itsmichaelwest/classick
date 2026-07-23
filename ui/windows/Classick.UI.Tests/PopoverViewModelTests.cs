using System;
using System.Threading.Tasks;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Xunit;

public class PopoverViewModelTests
{
    private static readonly DeviceId WireDeviceId = DeviceId.Parse("000A27002138B0A8");
    private static StatusUpdateEvent Status(string state, bool ipodConnected, HistoryEntry? last = null)
        => new StatusUpdateEvent(state, true, ipodConnected, last, null, null, 0, null, null);

    [Fact]
    public void Initial_status_text_is_offline_when_no_status_received_yet()
    {
        var vm = new PopoverViewModel();
        Assert.Equal("iPod not connected", vm.StatusText);
    }

    [Fact]
    public void Update_with_idle_and_connected_shows_up_to_date()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("idle", ipodConnected: true));
        Assert.StartsWith("Up to date", vm.StatusText);
    }

    [Fact]
    public void Update_with_syncing_shows_syncing()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        Assert.Equal("Syncing iPod…", vm.StatusText);
    }

    [Fact]
    public void Update_with_idle_and_disconnected_shows_offline()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("idle", ipodConnected: false));
        Assert.Equal("iPod not connected", vm.StatusText);
    }

    [Fact]
    public void Update_with_error_history_shows_error_text()
    {
        var vm = new PopoverViewModel();
        var failed = new HistoryEntry("2026-05-25T10:00:00Z", 5, "manual", "error",
            "Source unreachable", null, "SERIAL-A");
        vm.Update(Status("idle", ipodConnected: true, last: failed));
        Assert.Contains("Last sync failed", vm.StatusText);
    }

    [Fact]
    public void Device_snapshot_updates_label_connection_and_storage_atomically()
    {
        var vm = new PopoverViewModel();
        var device = new DeviceSnapshot(
            new DeviceIdentitySnapshot("SERIAL-B", "iPod Classic", "Beta"),
            Configured: true,
            Connected: true,
            Mount: "B:\\",
            Phase: "idle",
            SessionId: null,
            Storage: new StorageInfo(TotalBytes: 1000, FreeBytes: 400),
            SyncedCount: 12,
            LibraryCount: 20,
            LatestSuccessfulSync: null,
            LatestAttempt: null,
            LastTerminalError: null,
            SelectionRevision: 1,
            SettingsRevision: 1,
            SubscriptionsRevision: 1);

        vm.Update(device);

        Assert.Equal("Beta", vm.DeviceLabel);
        Assert.True(vm.IpodConnected);
        Assert.False(vm.Syncing);
        Assert.True(vm.HasStorage);
    }

    [Fact]
    public void PromptEvent_populates_overlay_state()
    {
        var vm = new PopoverViewModel();
        vm.ApplyIpcProgress(new PromptEvent(
            Id: 42,
            Message: "Source root has changed since the last sync.",
            Options: new[] { "Continue", "Use --no-delete", "Abort" }));
        Assert.True(vm.PromptActive);
        Assert.Equal((ulong)42, vm.PromptId);
        Assert.Equal("Source root has changed since the last sync.", vm.PromptMessage);
        Assert.Equal(3, vm.PromptOptions.Count);
        Assert.Equal("Continue", vm.PromptOptions[0]);
        Assert.Equal("Use --no-delete", vm.PromptOptions[1]);
        Assert.Equal("Abort", vm.PromptOptions[2]);
    }

    [Fact]
    public void Active_device_context_targets_cancel_pause_and_prompt_to_syncing_device()
    {
        var vm = new PopoverViewModel();
        var context = new SyncEventContext(SessionId: 42, Serial: "SERIAL-B");
        vm.SetActiveSyncSession(context);
        vm.Update(Device("SERIAL-B", connected: true, phase: "syncing", sessionId: 42));

        var cancel = vm.CreateCancelSyncCommand("cancel-request");
        var pause = vm.CreatePauseCommand("pause-request");
        vm.ApplySyncProgress(new RoutedSyncEvent(
            context,
            new PromptEvent(7, "Choose", ["Continue", "Abort"])));

        var prompt = vm.CreatePromptDecisionCommand(choice: 1, "prompt-request");

        Assert.Equal("SERIAL-B", cancel?.Serial);
        Assert.Equal("SERIAL-B", pause?.Serial);
        Assert.Equal("SERIAL-B", prompt?.Serial);
        Assert.Equal(7UL, prompt?.Id);
        Assert.Equal(1, prompt?.Choice);
    }

    [Fact]
    public void Disconnected_drain_keeps_context_but_disables_device_controls()
    {
        var vm = new PopoverViewModel();
        var context = new SyncEventContext(SessionId: 42, Serial: "SERIAL-B");
        vm.SetActiveSyncSession(context);

        vm.Update(Device("SERIAL-B", connected: false, phase: "disconnected", sessionId: 42));

        Assert.Equal(context, vm.ActiveSyncContext);
        Assert.False(vm.IpodConnected);
        Assert.False(vm.Syncing);
        Assert.True(vm.FinishingSync);
        Assert.Equal("Finishing sync…", vm.EmptyStateTitle);
        Assert.False(vm.ShowSyncControls);
        Assert.Null(vm.CreateCancelSyncCommand("cancel"));
        Assert.Null(vm.CreatePauseCommand("pause"));
    }

    [Fact]
    public void Paused_device_exposes_resume_target_without_active_session()
    {
        var vm = new PopoverViewModel();

        vm.Update(Device("SERIAL-B", connected: true, phase: "paused", sessionId: null));
        var resume = vm.CreateTriggerSyncCommand("SERIAL-A", "resume-request");

        Assert.Equal("SERIAL-B", vm.DisplayedDeviceSerial);
        Assert.Equal("Resume sync", vm.SyncActionLabel);
        Assert.True(vm.ShowSyncNowButton);
        Assert.Equal("SERIAL-B", resume?.Serial);
    }

    [Fact]
    public void Stale_session_progress_and_prompt_response_are_rejected()
    {
        var vm = new PopoverViewModel();
        var stale = new SyncEventContext(SessionId: 41, Serial: "SERIAL-B");
        var active = new SyncEventContext(SessionId: 42, Serial: "SERIAL-B");
        vm.SetActiveSyncSession(stale);
        vm.ApplySyncProgress(new RoutedSyncEvent(
            stale,
            new PromptEvent(7, "Old prompt", ["Continue"])));
        vm.SetActiveSyncSession(active);

        vm.ApplySyncProgress(new RoutedSyncEvent(
            stale,
            new TrackStartEvent(Current: 9, Total: 10, Label: "stale.flac")));

        Assert.Equal(0, vm.ProgressCurrent);
        Assert.False(vm.PromptActive);
        Assert.Null(vm.CreatePromptDecisionCommand(choice: 0, "stale-request"));
    }

    [Fact]
    public void Protocol3_controls_capture_device_and_session()
    {
        var vm = new PopoverViewModel();
        vm.Update(WireDevice(sessionId: 42));
        vm.SetActiveDeviceSession(new DeviceSessionTarget(WireDeviceId, 42));

        var cancel = vm.CreateWireCancelSyncCommand("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8820");
        var pause = vm.CreateWirePauseSyncCommand("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8821");

        Assert.Equal(WireDeviceId, cancel?.DeviceId);
        Assert.Equal((ulong)42, cancel?.SessionId);
        Assert.Equal(WireDeviceId, pause?.DeviceId);
        Assert.Equal((ulong)42, pause?.SessionId);
    }

    [Fact]
    public void Protocol3_manual_sync_targets_displayed_device()
    {
        var vm = new PopoverViewModel();
        vm.Update(WireDevice(sessionId: null));

        var command = vm.CreateWireTriggerSyncCommand("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8822");

        Assert.Equal(WireDeviceId, command?.DeviceId);
        Assert.Equal(SyncTrigger.Manual, command?.Trigger);
    }

    [Fact]
    public void Protocol3_readiness_disables_mutation_and_surfaces_Apple_guidance()
    {
        var vm = new PopoverViewModel();
        vm.Update(WireDevice(
            sessionId: null,
            readiness: DeviceReadiness.NeedsAppleInitialization,
            profile: ProfileStatus.NotAdopted));

        Assert.False(vm.DeviceReadyForSync);
        Assert.False(vm.ShowSyncNowButton);
        Assert.Contains("Apple setup required", vm.DeviceReadinessText);
        Assert.Contains("does not initialize", vm.DeviceGuidance);
    }

    [Fact]
    public void Protocol3_missing_colour_uses_generic_accessible_artwork()
    {
        var vm = new PopoverViewModel();
        vm.Update(WireDevice(sessionId: null));

        Assert.Equal("iPod classic", vm.DeviceArtworkDescription);
        Assert.DoesNotContain("silver", vm.DeviceArtworkDescription, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void Clearing_ambiguous_focus_disables_protocol3_mutation()
    {
        var vm = new PopoverViewModel();
        vm.Update(WireDevice(sessionId: null));

        vm.ClearDisplayedDevice();

        Assert.Null(vm.DisplayedDeviceId);
        Assert.Null(vm.CreateWireTriggerSyncCommand("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8823"));
    }

    private static DeviceSnapshot Device(
        string serial,
        bool connected,
        string phase,
        ulong? sessionId)
    {
        return new DeviceSnapshot(
            new DeviceIdentitySnapshot(serial, "iPod Classic", serial),
            Configured: true,
            Connected: connected,
            Mount: connected ? $"{serial}:\\" : null,
            Phase: phase,
            SessionId: sessionId,
            Storage: null,
            SyncedCount: 0,
            LibraryCount: null,
            LatestSuccessfulSync: null,
            LatestAttempt: null,
            LastTerminalError: null,
            SelectionRevision: 1,
            SettingsRevision: 1,
            SubscriptionsRevision: 1);
    }

    private static IdentifiedDeviceSnapshot WireDevice(
        ulong? sessionId,
        DeviceReadiness readiness = DeviceReadiness.Ready,
        ProfileStatus profile = ProfileStatus.Adopted) => new(
        WireDeviceId,
        "Michael's iPod",
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
        null,
        0,
        null,
        null);

    [Fact]
    public void TrackStart_clears_pending_prompt_defensively()
    {
        // Belt-and-suspenders: if the subprocess answered the prompt
        // via some other path (e.g. internal tracker bail), the
        // overlay must not linger over an active TrackStart.
        var vm = new PopoverViewModel();
        vm.ApplyIpcProgress(new PromptEvent(7, "x", new[] { "a", "b" }));
        Assert.True(vm.PromptActive);

        vm.ApplyIpcProgress(new TrackStartEvent(1, 100, "song.flac"));
        Assert.False(vm.PromptActive);
        Assert.Empty(vm.PromptOptions);
    }

    [Fact]
    public void Finish_clears_pending_prompt()
    {
        var vm = new PopoverViewModel();
        vm.ApplyIpcProgress(new PromptEvent(7, "x", new[] { "a" }));
        Assert.True(vm.PromptActive);

        vm.ApplyIpcProgress(new FinishEvent(true));
        Assert.False(vm.PromptActive);
    }

    [Fact]
    public void ClearPrompt_resets_all_overlay_state()
    {
        var vm = new PopoverViewModel();
        vm.ApplyIpcProgress(new PromptEvent(99, "hello", new[] { "ok" }));
        Assert.True(vm.PromptActive);

        vm.ClearPrompt();
        Assert.False(vm.PromptActive);
        Assert.Equal("", vm.PromptMessage);
        Assert.Equal((ulong)0, vm.PromptId);
        Assert.Empty(vm.PromptOptions);
    }

    [Fact]
    public void Active_prompt_hides_connected_and_footer_so_overlay_doesnt_need_opaque_background()
    {
        // The popover renders its prompt overlay over an acrylic
        // backdrop, so layering an "opaque" brush on top still lets
        // the underlying layout bleed through. Instead we suppress
        // the underlying content via these flag properties — the
        // overlay then sits on its own transparent background and
        // the popover shows only the prompt.
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        Assert.True(vm.ShowConnectedContent);
        Assert.True(vm.ShowFooter);

        vm.ApplyIpcProgress(new PromptEvent(1, "msg", new[] { "ok" }));
        Assert.False(vm.ShowConnectedContent);
        Assert.False(vm.ShowFooter);
        Assert.False(vm.ShowSyncNowButton);

        vm.ClearPrompt();
        Assert.True(vm.ShowConnectedContent);
        Assert.True(vm.ShowFooter);
    }

    [Fact]
    public void ProgressCaption_is_empty_when_not_syncing()
    {
        var vm = new PopoverViewModel();
        Assert.Equal("", vm.ProgressCaption);
    }

    [Fact]
    public void ProgressCaption_shows_preparing_before_summary()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        // No SummaryEvent yet → still in the prep phase.
        Assert.Equal("Preparing sync…", vm.ProgressCaption);
    }

    [Fact]
    public void ProgressCaption_shows_counter_after_summary_and_track_start()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        vm.ApplyIpcProgress(new SummaryEvent(Add: 20, Modify: 5, MetadataOnly: 0, Remove: 5, Unchanged: 0, TotalPlanned: 30));
        vm.ApplyIpcProgress(new TrackStartEvent(Current: 7, Total: 30, Label: "ADD /Music/x.flac"));
        Assert.Equal("Syncing 7 of 30 tracks", vm.ProgressCaption);
    }

    [Fact]
    public void Current_track_label_is_kept_for_tooltip_use()
    {
        // We dropped the per-track filename from the primary caption,
        // but it stays on CurrentTrackLabel so the XAML can hang it off
        // a ToolTipService.ToolTip on the caption TextBlock.
        var vm = new PopoverViewModel();
        vm.ApplyIpcProgress(new TrackStartEvent(1, 10, "ADD /Music/Artist/track.flac"));
        Assert.Equal("ADD /Music/Artist/track.flac", vm.CurrentTrackLabel);
    }

    [Fact]
    public void EtaLabel_is_empty_in_warmup_window()
    {
        // First two completed tracks are too noisy to estimate from —
        // we wait until we have at least 3 samples.
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        vm.ApplyIpcProgress(new SummaryEvent(Add: 100, Modify: 0, MetadataOnly: 0, Remove: 0, Unchanged: 0, TotalPlanned: 100));
        vm.ApplyIpcProgress(new TrackStartEvent(1, 100, "ADD /a")); // 0 completed
        Assert.Equal("", vm.EtaLabel);
        vm.ApplyIpcProgress(new TrackStartEvent(3, 100, "ADD /c")); // 2 completed
        Assert.Equal("", vm.EtaLabel);
    }

    [Fact]
    public void EtaLabel_renders_after_warmup_with_seeded_start_time()
    {
        // Seed SyncStartedAt far enough in the past that the per-track
        // average produces a stable, named ETA bucket.
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        vm.ApplyIpcProgress(new SummaryEvent(Add: 100, Modify: 0, MetadataOnly: 0, Remove: 0, Unchanged: 0, TotalPlanned: 100));
        vm.ApplyIpcProgress(new TrackStartEvent(11, 100, "ADD /k")); // 10 completed
        // Pretend the apply loop started 60 seconds ago → 6s/track →
        // ~90 tracks remaining → ~540s → "about 9 min left" bucket.
        vm.SyncStartedAt = DateTimeOffset.Now.AddSeconds(-60);
        Assert.Matches(@"about \d+ min left", vm.EtaLabel);
    }

    [Fact]
    public void FinishEvent_clears_eta_state()
    {
        var vm = new PopoverViewModel();
        vm.Update(Status("syncing", ipodConnected: true));
        vm.ApplyIpcProgress(new SummaryEvent(Add: 10, Modify: 0, MetadataOnly: 0, Remove: 0, Unchanged: 0, TotalPlanned: 10));
        Assert.NotNull(vm.SyncStartedAt);
        vm.ApplyIpcProgress(new FinishEvent(true));
        Assert.Null(vm.SyncStartedAt);
        Assert.Equal("", vm.EtaLabel);
    }

    [Fact]
    public void Active_prompt_also_hides_empty_state()
    {
        // Disconnect → empty state visible. A prompt during the
        // disconnected state is rare (apply_loop won't start without
        // a device) but we keep the suppression consistent so the
        // overlay can't end up layered over the "No iPod" hero.
        var vm = new PopoverViewModel();
        vm.Update(Status("idle", ipodConnected: false));
        Assert.True(vm.ShowEmptyState);

        vm.ApplyIpcProgress(new PromptEvent(1, "msg", new[] { "ok" }));
        Assert.False(vm.ShowEmptyState);
    }
}
