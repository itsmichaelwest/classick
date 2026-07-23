using Classick_UI.Ipc;
using Classick_UI.ViewModels;

namespace Classick_UI.Tests;

public sealed class SettingsGeneralViewModelTests
{
    private static readonly DeviceId First = DeviceId.Parse("000A27002138B0A8");
    private static readonly DeviceId Second = DeviceId.Parse("000A27002138B0A9");

    [Fact]
    public void SwitchingToUnloadedDeviceClearsAndDisablesPriorDeviceValues()
    {
        var store = StoreWithDevices();
        store.Reduce(Config(First, new SettingsValue(1, true, true)));
        var viewModel = new SettingsGeneralViewModel(Global(), store);
        viewModel.SelectDevice(First);
        Assert.True(viewModel.AutoSync);

        viewModel.SelectDevice(Second);

        Assert.False(viewModel.AutoSync);
        Assert.False(viewModel.RockboxCompat);
        Assert.False(viewModel.CanEditDeviceSettings);
        Assert.False(viewModel.CanEditSelection);
        Assert.False(viewModel.CanEditSubscriptions);
    }

    [Fact]
    public void DisconnectedCanonicalConfigRemainsEditableAndDeviceSpecific()
    {
        var store = StoreWithDevices();
        store.Reduce(Config(Second, new SettingsValue(1, false, true)));
        var viewModel = new SettingsGeneralViewModel(Global(), store);

        viewModel.SelectDevice(Second);

        Assert.True(viewModel.CanEditDeviceSettings);
        Assert.False(viewModel.AutoSync);
        Assert.True(viewModel.RockboxCompat);
    }

    [Fact]
    public void LastSyncUsesNewestHistoryEntry()
    {
        var store = StoreWithDevices();
        store.Reduce(Config(First, new SettingsValue(1, true, false)));
        store.Reduce(new HistoryEvent(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8801",
            [History("2026-07-20T10:00:00Z"), History("2026-07-22T10:00:00Z")]));
        var viewModel = new SettingsGeneralViewModel(Global(), store);

        viewModel.SelectDevice(First);

        Assert.Contains("2026-07-22", viewModel.LastSyncStatus);
    }

    [Fact]
    public void ExcludeSelectionModeIsPresentedExactly()
    {
        var store = StoreWithDevices();
        var config = Config(First, new SettingsValue(1, true, false)) with
        {
            Selection = new DeliveredComponent<SelectionValue>(
                2,
                Guid.NewGuid().ToString("D"),
                new SelectionValue(1, SelectionMode.Exclude, [new GenreSelectionRule("Podcasts")]),
                new DeviceCommittedDelivery()),
        };
        store.Reduce(config);
        var viewModel = new SettingsGeneralViewModel(Global(), store);

        viewModel.SelectDevice(First);

        Assert.Equal(SelectionMode.Exclude, viewModel.DeviceSelectionMode);
        Assert.Single(viewModel.LoadedSelection!.Rules);
    }

    private static DeviceStore StoreWithDevices()
    {
        var store = new DeviceStore();
        store.Reduce(new DeviceInventoryEvent(null, 1, [Device(First, true), Device(Second, false)], []));
        return store;
    }

    private static IdentifiedDeviceSnapshot Device(DeviceId id, bool connected) => new(
        id,
        id == First ? "First" : "Second",
        DeviceReadiness.Ready,
        new HardwareFacts(),
        ProfileStatus.Adopted,
        connected,
        connected ? "D:\\" : null,
        connected ? DevicePhase.Idle : DevicePhase.Disconnected,
        null,
        null,
        0,
        null,
        null);

    private static DeviceConfigEvent Config(DeviceId id, SettingsValue settings) => new(
        null,
        id,
        new DeliveredComponent<SelectionValue>(1, Guid.NewGuid().ToString("D"), new SelectionValue(1, SelectionMode.All, []), new DeviceCommittedDelivery()),
        new DeliveredComponent<SettingsValue>(1, Guid.NewGuid().ToString("D"), settings, new DeviceCommittedDelivery()),
        new DeliveredComponent<SubscriptionsValue>(1, Guid.NewGuid().ToString("D"), new SubscriptionsValue(1, []), new DeviceCommittedDelivery()));

    private static GlobalConfigEvent Global() => new(
        null,
        1,
        "C:\\Music",
        new GlobalSettings(SyncMode.Review, SyncMode.AutoApply, 30, NotifyLevel.All, DropSyncBehavior.Immediate));

    private static WireHistoryEntry History(string timestamp) => new(
        First,
        null,
        timestamp,
        10,
        HistoryTrigger.Manual,
        SyncOperation.Sync,
        SyncOutcome.Ok);
}
