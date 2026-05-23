using System;
using System.Threading;
using System.Threading.Tasks;
using IpodSync_UI.ViewModels;
using Xunit;

public class WizardViewModelTests
{
    [Fact]
    public void Starts_on_step_1_with_no_source()
    {
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(null),
            sendConfigFunc: _ => Task.CompletedTask);
        Assert.Equal(1, vm.CurrentStep);
        Assert.Equal("", vm.SourcePath);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void NextCommand_enabled_when_source_set_on_step_1()
    {
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(null),
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"\\HOST\share\music";
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Next_advances_to_step_2_and_awaits_device()
    {
        var detected = new IpodIdentityCandidate("0xABC", "iPod 7G", "G:\\");
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(detected),
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        // Step 2 wait runs async; give it a moment to populate.
        await Task.Delay(100);
        Assert.Equal(2, vm.CurrentStep);
        Assert.NotNull(vm.DetectedIpod);
        Assert.Equal("0xABC", vm.DetectedIpod!.Serial);
    }

    [Fact]
    public async Task Step_2_NextCommand_disabled_until_device_arrives()
    {
        var tcs = new TaskCompletionSource<IpodIdentityCandidate?>();
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => tcs.Task,
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(2, vm.CurrentStep);
        Assert.Null(vm.DetectedIpod);
        Assert.False(vm.NextCommand.CanExecute(null));

        // Now simulate the daemon firing a DeviceConnected event.
        tcs.SetResult(new IpodIdentityCandidate("X", "iPod 7G", "G:\\"));
        await Task.Delay(50);
        Assert.NotNull(vm.DetectedIpod);
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Retry_re_runs_wait_for_device()
    {
        int waitCount = 0;
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => { waitCount++; return Task.FromResult<IpodIdentityCandidate?>(null); },
            sendConfigFunc: _ => Task.CompletedTask);
        vm.SourcePath = @"C:\music";
        vm.NextCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(1, waitCount);
        vm.TriggerScanCommand.Execute(null);
        await Task.Delay(50);
        Assert.Equal(2, waitCount);
    }

    [Fact]
    public async Task Finish_sends_save_config_with_source_and_ipod()
    {
        SaveConfigPayload? sent = null;
        var vm = new WizardViewModel(
            waitForDeviceFunc: _ => Task.FromResult<IpodIdentityCandidate?>(
                new IpodIdentityCandidate("X", "iPod 7G", "G:\\")),
            sendConfigFunc: p => { sent = p; return Task.CompletedTask; });
        vm.SourcePath = @"\\HOST\music";
        vm.NextCommand.Execute(null);  // step 2 → triggers wait
        await Task.Delay(100);
        vm.NextCommand.Execute(null);  // step 3
        await vm.FinishCommand.ExecuteAsync(null);
        Assert.NotNull(sent);
        Assert.Equal(@"\\HOST\music", sent!.Source);
        Assert.Equal("X", sent.IpodSerial);
        Assert.Equal("iPod 7G", sent.IpodModelLabel);
    }
}
