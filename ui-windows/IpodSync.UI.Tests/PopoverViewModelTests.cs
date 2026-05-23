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
}
