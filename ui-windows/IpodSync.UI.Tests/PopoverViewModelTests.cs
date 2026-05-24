using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
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
            "Source unreachable", null);
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
