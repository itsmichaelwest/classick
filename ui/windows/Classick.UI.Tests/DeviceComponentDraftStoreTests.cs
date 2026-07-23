using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceComponentDraftStoreTests
{
    private static readonly DeviceId Device = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void SelectionAndSubscriptionsKeepIndependentAcknowledgedDrafts()
    {
        var store = new DeviceComponentDraftStore();
        store.ApplyCanonical(Config(1, "initial", new DeviceCommittedDelivery()));

        var selection = store.EditSelection(
            Device, new SelectionValue(1, SelectionMode.Include, []), "selection-request", "selection-draft");
        var subscriptions = store.EditSubscriptions(
            Device, new SubscriptionsValue(1, ["road-trip"]), "subscriptions-request", "subscriptions-draft");

        Assert.IsType<SetSelectionCommand>(selection);
        Assert.IsType<SetSubscriptionsCommand>(subscriptions);
        Assert.Equal(DeviceSettingsSaveState.Editing, store.Selections[Device].SaveState);
        Assert.Equal(["road-trip"], store.Subscriptions[Device].Value.Playlists);
    }

    [Fact]
    public void PendingDeliveryAndOlderCanonicalPreserveAcceptedSubscription()
    {
        var store = new DeviceComponentDraftStore();
        store.ApplyCanonical(Config(2, "initial", new DeviceCommittedDelivery()));
        store.EditSubscriptions(
            Device, new SubscriptionsValue(1, ["road-trip"]), "request", "draft");
        store.ApplyCanonical(Config(3, "draft", new PendingDeviceDelivery(), ["road-trip"]));

        store.ApplyCanonical(Config(2, "late", new DeviceCommittedDelivery(), []));

        Assert.Equal(["road-trip"], store.Subscriptions[Device].Value.Playlists);
        Assert.Equal(DeviceSettingsSaveState.WaitingForDevice, store.Subscriptions[Device].SaveState);
    }

    [Fact]
    public void DeviceDeliveryFailureKeepsSubscriptionHostAuthoritative()
    {
        var store = new DeviceComponentDraftStore();
        store.ApplyCanonical(Config(1, "initial", new DeviceCommittedDelivery()));
        store.EditSubscriptions(
            Device, new SubscriptionsValue(1, ["road-trip"]), "request", "draft");
        store.ApplyCanonical(Config(2, "draft", new PendingDeviceDelivery(), ["road-trip"]));

        Assert.True(store.ApplyFailure(new ConfigMutationFailedEvent(
            Device,
            "request",
            "draft",
            ConfigComponent.Subscriptions,
            ConfigFailureStage.DeviceDelivery,
            "iPod disconnected")));

        Assert.Equal(["road-trip"], store.Subscriptions[Device].Value.Playlists);
        Assert.Equal(DeviceSettingsSaveState.WaitingForDevice, store.Subscriptions[Device].SaveState);
        Assert.Equal("iPod disconnected", store.Subscriptions[Device].Error);
    }

    private static DeviceConfigEvent Config(
        ulong revision,
        string mutation,
        ConfigDelivery delivery,
        IReadOnlyList<string>? playlists = null) => new(
            null,
            Device,
            new DeliveredComponent<SelectionValue>(revision, $"selection-{mutation}", new SelectionValue(1, SelectionMode.All, []), delivery),
            new DeliveredComponent<SettingsValue>(revision, $"settings-{mutation}", new SettingsValue(1, false, false), delivery),
            new DeliveredComponent<SubscriptionsValue>(revision, mutation, new SubscriptionsValue(1, playlists ?? []), delivery));
}
