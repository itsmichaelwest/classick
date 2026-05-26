using System.IO;
using System.Threading.Tasks;
using Classick_UI.ViewModels;
using Xunit;

public class WizardViewModelTests
{
    // Use the OS temp dir as a SourcePath that's guaranteed to exist on
    // every test host. Picking C:\music would be flaky on machines
    // without that folder.
    private static readonly string ExistingDir = Path.GetTempPath();

    private static WizardViewModel NewVm(Func<SaveConfigPayload, Task>? send = null) =>
        new(send ?? (_ => Task.CompletedTask));

    [Fact]
    public void Starts_on_step_1_welcome_with_next_enabled()
    {
        var vm = NewVm();
        Assert.Equal(1, vm.CurrentStep);
        Assert.True(vm.IsWelcomeStep);
        // Welcome has nothing to validate — Next is always live.
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Next_from_welcome_advances_to_folder_step()
    {
        var vm = NewVm();
        await vm.NextCommand.ExecuteAsync(null);
        Assert.Equal(2, vm.CurrentStep);
        Assert.True(vm.IsFolderStep);
        // Folder step requires a source path before advancing.
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Folder_step_next_requires_existing_directory()
    {
        var vm = NewVm();
        await vm.NextCommand.ExecuteAsync(null);  // → step 2
        Assert.False(vm.NextCommand.CanExecute(null));

        // Non-existent path stays blocked even though it's non-empty.
        vm.SourcePath = @"X:\definitely-does-not-exist\nope-nope-nope-12345";
        Assert.False(vm.IsSourcePathValid);
        Assert.False(vm.NextCommand.CanExecute(null));

        vm.SourcePath = ExistingDir;
        Assert.True(vm.IsSourcePathValid);
        Assert.True(vm.NextCommand.CanExecute(null));

        // Clearing the path re-blocks.
        vm.SourcePath = "";
        Assert.False(vm.IsSourcePathValid);
        Assert.False(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task Device_step_does_not_auto_select_first_arrival()
    {
        var vm = NewVm();
        await vm.NextCommand.ExecuteAsync(null);
        vm.SourcePath = ExistingDir;
        await vm.NextCommand.ExecuteAsync(null);  // → step 3
        Assert.Equal(3, vm.CurrentStep);

        vm.OnDeviceConnected(new IpodIdentityCandidate("0xABC", "iPod 7G", "G:\\"));
        // Selection stays null — the user must explicitly pick a row.
        Assert.Null(vm.SelectedIpod);
        Assert.False(vm.NextCommand.CanExecute(null));

        vm.SelectedIpod = vm.Candidates[0];
        Assert.True(vm.NextCommand.CanExecute(null));
    }

    [Fact]
    public void OnDeviceConnected_dedupes_by_serial()
    {
        var vm = NewVm();
        var c = new IpodIdentityCandidate("X", "iPod 7G", "G:\\");
        vm.OnDeviceConnected(c);
        vm.OnDeviceConnected(c);
        vm.OnDeviceConnected(c with { Drive = "H:\\" });
        Assert.Single(vm.Candidates);
    }

    [Fact]
    public void OnDeviceConnected_refreshes_candidate_when_name_arrives_later()
    {
        // The daemon's two-phase DeviceConnected (initial → re-fire with name
        // from iTunesDB) should update the existing row in place so the
        // user's selection points at a candidate carrying the friendly name
        // by the time save_config fires.
        var vm = NewVm();
        var first = new IpodIdentityCandidate("X", "iPod Classic 7G", "G:\\");
        vm.OnDeviceConnected(first);
        vm.SelectedIpod = vm.Candidates[0];
        Assert.Null(vm.SelectedIpod!.Name);

        var second = first with { Name = "Michael's iPod" };
        vm.OnDeviceConnected(second);

        Assert.Single(vm.Candidates);
        Assert.Equal("Michael's iPod", vm.Candidates[0].Name);
        Assert.Equal("Michael's iPod", vm.SelectedIpod!.Name);
    }

    [Fact]
    public void OnDeviceDisconnected_removes_candidate_and_clears_selection()
    {
        var vm = NewVm();
        var a = new IpodIdentityCandidate("A", "iPod 7G", "G:\\");
        var b = new IpodIdentityCandidate("B", "Shuffle 2G", "H:\\");
        vm.OnDeviceConnected(a);
        vm.OnDeviceConnected(b);
        vm.SelectedIpod = b;
        vm.OnDeviceDisconnected("B");
        Assert.Single(vm.Candidates);
        Assert.Null(vm.SelectedIpod);
    }

    [Fact]
    public void IpodIdentityCandidate_DisplayName_falls_back_when_name_missing()
    {
        var anon = new IpodIdentityCandidate("X", "iPod 7G", "G:\\");
        Assert.Equal("iPod", anon.DisplayName);

        var named = anon with { Name = "Michael's iPod" };
        Assert.Equal("Michael's iPod", named.DisplayName);
    }

    [Fact]
    public async Task Sync_settings_step_advances_after_save_and_reaches_done()
    {
        SaveConfigPayload? sent = null;
        var vm = NewVm(p => { sent = p; return Task.CompletedTask; });
        await vm.NextCommand.ExecuteAsync(null);
        vm.SourcePath = ExistingDir;
        await vm.NextCommand.ExecuteAsync(null);
        vm.OnDeviceConnected(new IpodIdentityCandidate("X", "iPod 7G", "G:\\", Name: "Michael's iPod"));
        vm.SelectedIpod = vm.Candidates[0];
        await vm.NextCommand.ExecuteAsync(null);
        Assert.Equal(4, vm.CurrentStep);

        vm.IsAutomatic = false;
        vm.ScheduleMinutes = 60;
        vm.AutostartWithWindows = false;

        await vm.NextCommand.ExecuteAsync(null);

        Assert.Equal(5, vm.CurrentStep);
        Assert.True(vm.IsDoneStep);
        Assert.NotNull(sent);
        Assert.Equal(ExistingDir, sent!.Source);
        Assert.Equal("X", sent.IpodSerial);
        Assert.Equal("iPod 7G", sent.IpodModelLabel);
        // The wizard must carry the friendly name through to save_config so
        // the popover shows "Michael's iPod" immediately, without waiting
        // for the daemon to re-resolve the iTunesDB name on next launch.
        Assert.Equal("Michael's iPod", sent.IpodName);
        Assert.Equal("review", sent.SubsequentSyncMode);
        Assert.Equal(60u, sent.ScheduleMinutes);
        Assert.False(sent.AutostartWithWindows);
    }

    [Fact]
    public async Task Default_sync_settings_round_trip_as_auto_apply_30min_autostart()
    {
        SaveConfigPayload? sent = null;
        var vm = NewVm(p => { sent = p; return Task.CompletedTask; });
        await vm.NextCommand.ExecuteAsync(null);
        vm.SourcePath = ExistingDir;
        await vm.NextCommand.ExecuteAsync(null);
        vm.OnDeviceConnected(new IpodIdentityCandidate("X", "iPod 7G", "G:\\"));
        vm.SelectedIpod = vm.Candidates[0];
        await vm.NextCommand.ExecuteAsync(null);  // → step 4
        await vm.NextCommand.ExecuteAsync(null);  // → step 5 (save)
        Assert.Equal("auto_apply", sent!.SubsequentSyncMode);
        Assert.Equal(30u, sent.ScheduleMinutes);
        Assert.True(sent.AutostartWithWindows);
    }

    [Fact]
    public async Task Save_failure_keeps_user_on_sync_settings_with_error()
    {
        var vm = NewVm(_ => throw new System.IO.IOException("daemon offline"));
        await vm.NextCommand.ExecuteAsync(null);
        vm.SourcePath = ExistingDir;
        await vm.NextCommand.ExecuteAsync(null);
        vm.OnDeviceConnected(new IpodIdentityCandidate("X", "iPod 7G", "G:\\"));
        vm.SelectedIpod = vm.Candidates[0];
        await vm.NextCommand.ExecuteAsync(null);  // → step 4
        await vm.NextCommand.ExecuteAsync(null);  // attempt save → fails

        Assert.Equal(4, vm.CurrentStep);
        Assert.True(vm.HasScanError);
        Assert.Contains("daemon offline", vm.ScanError);
    }

    [Fact]
    public void HasScanError_flips_with_ScanError()
    {
        var vm = NewVm();
        Assert.False(vm.HasScanError);
        vm.ScanError = "boom";
        Assert.True(vm.HasScanError);
        vm.ScanError = "";
        Assert.False(vm.HasScanError);
    }

    [Fact]
    public async Task IsManual_and_IsAutomatic_are_inverse()
    {
        var vm = NewVm();
        Assert.True(vm.IsAutomatic);
        Assert.False(vm.IsManual);
        vm.IsManual = true;
        Assert.False(vm.IsAutomatic);
        Assert.True(vm.IsManual);
        await Task.CompletedTask;
    }

    [Fact]
    public async Task Back_returns_to_previous_step_except_on_done()
    {
        var vm = NewVm();
        await vm.NextCommand.ExecuteAsync(null);  // → 2
        vm.BackCommand.Execute(null);
        Assert.Equal(1, vm.CurrentStep);

        await vm.NextCommand.ExecuteAsync(null);
        vm.SourcePath = ExistingDir;
        await vm.NextCommand.ExecuteAsync(null);
        vm.OnDeviceConnected(new IpodIdentityCandidate("X", "iPod 7G", "G:\\"));
        vm.SelectedIpod = vm.Candidates[0];
        await vm.NextCommand.ExecuteAsync(null);  // → 4
        await vm.NextCommand.ExecuteAsync(null);  // → 5
        Assert.False(vm.BackCommand.CanExecute(null));
        Assert.False(vm.CanGoBackToPrevious);
    }
}
