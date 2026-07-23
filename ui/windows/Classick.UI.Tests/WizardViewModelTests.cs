using Classick_UI.Ipc;
using Classick_UI.ViewModels;

namespace Classick_UI.Tests;

public sealed class WizardViewModelTests
{
    private static readonly string ExistingDir = Path.GetTempPath();
    private static readonly DeviceId First = DeviceId.Parse("000A27002138B0A8");
    private static readonly DeviceId Second = DeviceId.Parse("000A27002138B0A9");

    private static WizardViewModel NewVm(Func<SaveConfigPayload, Task>? send = null) =>
        new(send ?? (_ => Task.CompletedTask));

    [Fact]
    public void SetupOffersTheCompleteDeviceIndependentTranscodeProfileList()
    {
        var viewModel = NewVm();

        Assert.Equal(
            [TranscodeProfile.Alac, TranscodeProfile.Aac256, TranscodeProfile.Aac192, TranscodeProfile.Aac128],
            viewModel.TranscodeProfiles.Select(option => option.Value));
    }

    [Fact]
    public async Task FolderStepRequiresExistingDirectory()
    {
        var viewModel = NewVm();
        await viewModel.NextCommand.ExecuteAsync(null);
        Assert.False(viewModel.NextCommand.CanExecute(null));

        viewModel.SourcePath = ExistingDir;

        Assert.True(viewModel.NextCommand.CanExecute(null));
    }

    [Fact]
    public async Task DeviceStepRequiresExplicitAdoptableDeviceSelection()
    {
        var viewModel = await AtDeviceStep();
        viewModel.ApplyInventory(Inventory(
            Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted),
            Device(Second, DeviceReadiness.NeedsAppleInitialization, ProfileStatus.NotAdopted)));

        Assert.False(viewModel.NextCommand.CanExecute(null));
        viewModel.SelectedDevice = viewModel.Candidates.Single(candidate => candidate.DeviceId == Second);
        Assert.False(viewModel.NextCommand.CanExecute(null));
        viewModel.SelectedDevice = viewModel.Candidates.Single(candidate => candidate.DeviceId == First);
        Assert.True(viewModel.NextCommand.CanExecute(null));
    }

    [Fact]
    public void InventoryShowsUnsafeAndUnidentifiedRowsWithoutCommandTargets()
    {
        var viewModel = NewVm();
        viewModel.ApplyInventory(new DeviceInventoryEvent(
            null,
            1,
            [Device(First, DeviceReadiness.InvalidDatabase, ProfileStatus.Invalid)],
            [new UnidentifiedDeviceSnapshot(7, DeviceReadiness.IdentityUnavailable, new HardwareFacts())]));

        Assert.Equal(2, viewModel.Candidates.Count);
        Assert.All(viewModel.Candidates, candidate => Assert.False(candidate.CanAdopt));
        Assert.Contains(viewModel.Candidates, candidate => candidate.Status == "iPod database is invalid");
        Assert.Contains(viewModel.Candidates, candidate => candidate.ObservationId == 7 && candidate.DeviceId is null);
    }

    [Fact]
    public void InventoryRefreshPreservesSelectionByDeviceIdAcrossMountChanges()
    {
        var viewModel = NewVm();
        viewModel.ApplyInventory(Inventory(Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));
        viewModel.SelectedDevice = viewModel.Candidates[0];

        viewModel.ApplyInventory(Inventory(Device(
            First,
            DeviceReadiness.Ready,
            ProfileStatus.NotAdopted,
            mount: "E:\\")));

        Assert.Equal(First, viewModel.SelectedDevice?.DeviceId);
    }

    [Fact]
    public void InventoryRemovalClearsSelectionWithoutRetargeting()
    {
        var viewModel = NewVm();
        viewModel.ApplyInventory(Inventory(
            Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted),
            Device(Second, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));
        viewModel.SelectedDevice = viewModel.Candidates.Single(candidate => candidate.DeviceId == First);

        viewModel.ApplyInventory(Inventory(Device(Second, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));

        Assert.Null(viewModel.SelectedDevice);
    }

    [Fact]
    public async Task ManualSetupProducesDeviceIdTargetAndPerDeviceAutoSyncOff()
    {
        SaveConfigPayload? sent = null;
        var viewModel = await AtDeviceStep(payload =>
        {
            sent = payload;
            return Task.CompletedTask;
        });
        viewModel.ApplyInventory(Inventory(Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));
        viewModel.SelectedDevice = viewModel.Candidates[0];
        await viewModel.NextCommand.ExecuteAsync(null);
        viewModel.IsAutomatic = false;
        viewModel.TranscodeProfile = TranscodeProfile.Aac192;

        await viewModel.NextCommand.ExecuteAsync(null);

        Assert.Equal(5, viewModel.CurrentStep);
        Assert.Equal(First, sent?.DeviceId);
        Assert.False(sent?.AutoSync);
        Assert.Equal(TranscodeProfile.Aac192, sent?.TranscodeProfile);
    }

    [Fact]
    public async Task AutomaticSetupEnablesPerDeviceAutoSync()
    {
        SaveConfigPayload? sent = null;
        var viewModel = await AtDeviceStep(payload =>
        {
            sent = payload;
            return Task.CompletedTask;
        });
        viewModel.ApplyInventory(Inventory(Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));
        viewModel.SelectedDevice = viewModel.Candidates[0];
        await viewModel.NextCommand.ExecuteAsync(null);

        await viewModel.NextCommand.ExecuteAsync(null);

        Assert.True(sent?.AutoSync);
    }

    [Fact]
    public async Task SaveFailureKeepsWizardOnSettingsStep()
    {
        var viewModel = await AtDeviceStep(_ => throw new IOException("daemon offline"));
        viewModel.ApplyInventory(Inventory(Device(First, DeviceReadiness.Ready, ProfileStatus.NotAdopted)));
        viewModel.SelectedDevice = viewModel.Candidates[0];
        await viewModel.NextCommand.ExecuteAsync(null);

        await viewModel.NextCommand.ExecuteAsync(null);

        Assert.Equal(4, viewModel.CurrentStep);
        Assert.Contains("daemon offline", viewModel.ScanError);
    }

    private static async Task<WizardViewModel> AtDeviceStep(
        Func<SaveConfigPayload, Task>? send = null)
    {
        var viewModel = NewVm(send);
        await viewModel.NextCommand.ExecuteAsync(null);
        viewModel.SourcePath = ExistingDir;
        await viewModel.NextCommand.ExecuteAsync(null);
        return viewModel;
    }

    private static DeviceInventoryEvent Inventory(params IdentifiedDeviceSnapshot[] devices) =>
        new(null, 1, devices, []);

    private static IdentifiedDeviceSnapshot Device(
        DeviceId deviceId,
        DeviceReadiness readiness,
        ProfileStatus profile,
        string mount = "D:\\") => new(
            deviceId,
            deviceId == First ? "First iPod" : "Second iPod",
            readiness,
            new HardwareFacts(Family: new HardwareFact<IpodFamily>(
                IpodFamily.Classic,
                FactSource.Decoded,
                FactConfidence.Certain)),
            profile,
            true,
            mount,
            DevicePhase.Unconfigured,
            null,
            null,
            0,
            null,
            null);
}
