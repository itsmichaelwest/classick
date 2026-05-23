using System.Threading.Tasks;
using IpodSync_UI.ViewModels;
using Xunit;

public class WizardViewModelTests
{
    [Fact]
    public void Starts_on_step_1_with_no_source()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        Assert.Equal(1, vm.CurrentStep);
        Assert.Equal("", vm.SourcePath);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void NextCommand_enabled_when_source_set_on_step_1()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"\\HOST\share\music";
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void Next_advances_to_step_2_and_triggers_initial_scan()
    {
        var vm = new WizardViewModel(scanFunc: () => new IpodIdentityCandidate("0xABC", "iPod 7G", "G:\\"),
                                     sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        Assert.Equal(2, vm.CurrentStep);
        Assert.NotNull(vm.DetectedIpod);
        Assert.Equal("0xABC", vm.DetectedIpod!.Serial);
    }

    [Fact]
    public void Step_2_NextCommand_disabled_until_ipod_detected()
    {
        var vm = new WizardViewModel(scanFunc: () => null, sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);  // advance to step 2
        Assert.Equal(2, vm.CurrentStep);
        Assert.Null(vm.DetectedIpod);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Finish_sends_save_config_with_source_and_ipod()
    {
        SaveConfigPayload? sent = null;
        var vm = new WizardViewModel(
            scanFunc: () => new IpodIdentityCandidate("X", "iPod 7G", "G:\\"),
            sendConfigFunc: p => { sent = p; return Task.CompletedTask; });
        vm.SourcePath = @"\\HOST\music";
        vm.NextCommand.Execute(null);  // step 2 (with iPod)
        vm.NextCommand.Execute(null);  // step 3
        await vm.FinishCommand.ExecuteAsync(null);
        Assert.NotNull(sent);
        Assert.Equal(@"\\HOST\music", sent!.Source);
        Assert.Equal("X", sent.IpodSerial);
        Assert.Equal("iPod 7G", sent.IpodModelLabel);
    }
}
