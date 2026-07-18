using System;
using System.Threading.Tasks;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Xunit;

public class PopoverViewModelTests
{
    private static StatusUpdateEvent Status(string state, bool ipodConnected, HistoryEntry? last = null)
        => new StatusUpdateEvent(state, true, ipodConnected, last, null);

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
