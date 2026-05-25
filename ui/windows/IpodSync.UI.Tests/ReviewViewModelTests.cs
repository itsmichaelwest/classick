using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;

namespace IpodSync_UI.Tests;

/// <summary>
/// Tests for <see cref="ReviewViewModel"/>. Pure CPU-bound — no XAML, no
/// dispatcher, no subprocess. The VM is link-compiled from the WinUI app
/// project (see csproj) so this plain-net10.0 test host can exercise it
/// without dragging in the WindowsAppRuntime initializer.
/// </summary>
public class ReviewViewModelTests
{
    private static ReviewEvent MakeEvent(
        int add = 0, int modify = 0, int metadataOnly = 0,
        int remove = 0, int unchanged = 0, bool noDelete = false) =>
        new(new ActionPlanSummary(add, modify, metadataOnly, remove, unchanged), noDelete);

    [Fact]
    public void LoadFromEvent_populates_all_fields()
    {
        var vm = new ReviewViewModel();
        var header = new HeaderEvent(@"\\HOST\share", "G:\\", "C:\\manifest.json");
        var evt = MakeEvent(add: 12, modify: 3, metadataOnly: 1, remove: 0, unchanged: 1260, noDelete: false);

        vm.LoadFromEvent(evt, header);

        Assert.Equal(@"\\HOST\share", vm.Source);
        Assert.Equal("G:\\", vm.Ipod);
        Assert.Equal("C:\\manifest.json", vm.Manifest);
        Assert.Equal(12, vm.Add);
        Assert.Equal(16, vm.TotalToApply); // 12 + 3 + 1 + 0
        Assert.True(vm.CanDecide);
    }

    [Fact]
    public void NoDelete_toggle_zeroes_EffectiveRemove()
    {
        var vm = new ReviewViewModel();
        vm.LoadFromEvent(MakeEvent(remove: 100));

        Assert.Equal(100, vm.EffectiveRemove);

        vm.NoDelete = true;

        Assert.Equal(0, vm.EffectiveRemove);
        Assert.Equal(0, vm.TotalToApply);
    }

    [Fact]
    public void Apply_command_fires_DecisionMade_with_correct_payload()
    {
        var vm = new ReviewViewModel();
        vm.LoadFromEvent(MakeEvent(add: 1));
        ReviewDecisionCommand? captured = null;
        vm.DecisionMade += cmd => captured = cmd;

        vm.ApplyCommand.Execute(null);

        Assert.NotNull(captured);
        var payload = Assert.IsType<ApplyDecision>(captured!.Decision);
        Assert.False(payload.NoDelete);
        Assert.False(vm.CanDecide); // disabled after click
    }

    [Fact]
    public void Apply_command_with_no_delete_toggle_carries_flag()
    {
        var vm = new ReviewViewModel();
        vm.LoadFromEvent(MakeEvent(add: 1, remove: 5));
        vm.NoDelete = true;
        ReviewDecisionCommand? captured = null;
        vm.DecisionMade += cmd => captured = cmd;

        vm.ApplyCommand.Execute(null);

        var payload = Assert.IsType<ApplyDecision>(captured!.Decision);
        Assert.True(payload.NoDelete);
    }

    [Fact]
    public void Cannot_decide_before_event_loaded()
    {
        var vm = new ReviewViewModel();

        Assert.False(vm.CanDecide);
        Assert.False(vm.ApplyCommand.CanExecute(null));
    }

    [Fact]
    public void Cannot_double_submit()
    {
        var vm = new ReviewViewModel();
        vm.LoadFromEvent(MakeEvent(add: 1));

        Assert.True(vm.ApplyCommand.CanExecute(null));
        vm.ApplyCommand.Execute(null);

        Assert.False(vm.ApplyCommand.CanExecute(null));
    }
}
