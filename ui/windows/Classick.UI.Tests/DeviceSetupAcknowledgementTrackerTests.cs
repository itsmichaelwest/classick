using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceSetupAcknowledgementTrackerTests
{
    private static readonly DeviceId Device = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void CompletesOnlyAfterSourceAndAllAdoptionComponentsAreCanonical()
    {
        var (tracker, source, adopt) = Create();

        tracker.Observe(new GlobalConfigEvent(source.RequestId, 2, "D:\\Music", Globals()));
        Assert.False(tracker.IsComplete);

        tracker.Observe(Config(adopt));

        Assert.True(tracker.IsComplete);
        Assert.Null(tracker.Failure);
    }

    [Fact]
    public void CorrelatedMutationFailurePreventsFalseSuccess()
    {
        var (tracker, _, adopt) = Create();

        tracker.Observe(new ConfigMutationFailedEvent(
            Device,
            adopt.RequestId,
            adopt.SettingsMutationId,
            ConfigComponent.Settings,
            ConfigFailureStage.HostAcceptance,
            "could not persist profile"));

        Assert.False(tracker.IsComplete);
        Assert.Equal("could not persist profile", tracker.Failure);
    }

    [Fact]
    public void DeviceDeliveryFailureDoesNotUndoHostAcceptedSetup()
    {
        var (tracker, source, adopt) = Create();
        tracker.Observe(new GlobalConfigEvent(source.RequestId, 2, "D:\\Music", Globals()));
        tracker.Observe(new ConfigMutationFailedEvent(
            Device,
            adopt.RequestId,
            adopt.SettingsMutationId,
            ConfigComponent.Settings,
            ConfigFailureStage.DeviceDelivery,
            "iPod disconnected"));
        tracker.Observe(Config(adopt));

        Assert.True(tracker.IsComplete);
        Assert.Null(tracker.Failure);
    }

    private static (DeviceSetupAcknowledgementTracker Tracker, SetSourceLocationCommand Source, AdoptDeviceCommand Adopt) Create()
    {
        var commands = DeviceSetupCommandFactory.Create(
            new DeviceSetupIntent("D:\\Music", Device, false),
            () => Guid.NewGuid().ToString("D"));
        var source = Assert.IsType<SetSourceLocationCommand>(commands[0]);
        var adopt = Assert.IsType<AdoptDeviceCommand>(commands[1]);
        return (new DeviceSetupAcknowledgementTracker(source, adopt), source, adopt);
    }

    private static DeviceConfigEvent Config(AdoptDeviceCommand adopt) => new(
        adopt.RequestId,
        Device,
        new DeliveredComponent<SelectionValue>(1, adopt.SelectionMutationId, adopt.Selection, new PendingDeviceDelivery()),
        new DeliveredComponent<SettingsValue>(1, adopt.SettingsMutationId, adopt.Settings, new PendingDeviceDelivery()),
        new DeliveredComponent<SubscriptionsValue>(1, adopt.SubscriptionsMutationId, adopt.Subscriptions, new PendingDeviceDelivery()));

    private static GlobalSettings Globals() =>
        new(SyncMode.Review, SyncMode.AutoApply, 30, NotifyLevel.All, DropSyncBehavior.Immediate);
}
