using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceSettingsDraftStoreTests
{
    private static readonly DeviceId Device = DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void DisconnectedEditRendersImmediatelyAndSurvivesAnotherViewModel()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));

        store.Edit(Device, new SettingsValue(1, false, false), "request", "draft");
        var reopened = store.Drafts[Device];

        Assert.False(reopened.Value.AutoSync);
        Assert.Equal(DeviceSettingsSaveState.Editing, reopened.SaveState);
    }

    [Fact]
    public void CorrelatedCanonicalUpdateTransitionsAcceptedThenCommitted()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        store.Edit(Device, new SettingsValue(1, false, true), "request", "draft");

        store.ApplyCanonical(Config("draft", new SettingsValue(1, false, true), new PendingDeviceDelivery()));
        Assert.Equal(DeviceSettingsSaveState.WaitingForDevice, store.Drafts[Device].SaveState);
        Assert.False(store.Drafts[Device].Value.AutoSync);

        store.ApplyCanonical(Config("draft", new SettingsValue(1, false, true), new DeviceCommittedDelivery()));
        Assert.Equal(DeviceSettingsSaveState.Saved, store.Drafts[Device].SaveState);
    }

    [Fact]
    public void OlderCanonicalCannotRegressAcceptedOrCommittedValue()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("old", new SettingsValue(1, true, false), new DeviceCommittedDelivery(), 2));
        store.Edit(Device, new SettingsValue(1, false, true), "request", "draft");
        store.ApplyCanonical(Config("draft", new SettingsValue(1, false, true), new PendingDeviceDelivery(), 3));

        store.ApplyCanonical(Config("late-old", new SettingsValue(1, true, false), new DeviceCommittedDelivery(), 2));
        Assert.False(store.Drafts[Device].Value.AutoSync);

        store.ApplyCanonical(Config("draft", new SettingsValue(1, false, true), new DeviceCommittedDelivery(), 4));
        store.ApplyCanonical(Config("late-old", new SettingsValue(1, true, false), new DeviceCommittedDelivery(), 2));
        Assert.False(store.Drafts[Device].Value.AutoSync);
    }

    [Fact]
    public void DeviceDeliveryFailureRetainsAuthoritativeHostValueAsPending()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        store.Edit(Device, new SettingsValue(1, false, false), "request", "draft");
        store.ApplyCanonical(Config("draft", new SettingsValue(1, false, false), new PendingDeviceDelivery(), 2));

        store.ApplyFailure(new ConfigMutationFailedEvent(
            Device, "request", "draft", ConfigComponent.Settings,
            ConfigFailureStage.DeviceDelivery, "device unavailable"));

        Assert.False(store.Drafts[Device].Value.AutoSync);
        Assert.Equal(DeviceSettingsSaveState.WaitingForDevice, store.Drafts[Device].SaveState);
        Assert.Equal("draft", store.Drafts[Device].AcceptedMutationId);
    }

    [Fact]
    public void UnrelatedConfigCannotOverwritePendingHostDraft()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        store.Edit(Device, new SettingsValue(1, false, true), "request", "draft");

        store.ApplyCanonical(Config("other", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));

        Assert.False(store.Drafts[Device].Value.AutoSync);
        Assert.True(store.Drafts[Device].Value.RockboxCompat);
        Assert.Equal("draft", store.Drafts[Device].PendingMutationId);
    }

    [Fact]
    public void HostFailureRetainsDraftWithActionableError()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        store.Edit(Device, new SettingsValue(1, false, false), "request", "draft");

        Assert.True(store.ApplyFailure(new ConfigMutationFailedEvent(
            Device, "request", "draft", ConfigComponent.Settings,
            ConfigFailureStage.HostAcceptance, "disk full")));

        Assert.False(store.Drafts[Device].Value.AutoSync);
        Assert.Equal(DeviceSettingsSaveState.Failed, store.Drafts[Device].SaveState);
        Assert.Equal("disk full", store.Drafts[Device].Error);

        store.ApplyCanonical(Config("old", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        Assert.False(store.Drafts[Device].Value.AutoSync);
    }

    [Fact]
    public void SwitchingDevicesCannotLeakSettingsDrafts()
    {
        var second = DeviceId.Parse("000A27002138B0A9");
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("first", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));
        store.ApplyCanonical(Config("second", new SettingsValue(1, false, true), new DeviceCommittedDelivery()) with { DeviceId = second });

        store.Edit(Device, new SettingsValue(1, false, false), "request", "draft");

        Assert.False(store.Drafts[Device].Value.AutoSync);
        Assert.False(store.Drafts[second].Value.AutoSync);
        Assert.True(store.Drafts[second].Value.RockboxCompat);
    }

    [Fact]
    public void DeviceSettingsCommandContainsNoAppearanceMetadata()
    {
        var store = new DeviceSettingsDraftStore();
        store.ApplyCanonical(Config("canonical", new SettingsValue(1, true, false), new DeviceCommittedDelivery()));

        var command = store.Edit(
            Device,
            new SettingsValue(1, false, true),
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8901",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8902");
        var json = WireCodec.Encode(command);

        Assert.DoesNotContain("colour", json, StringComparison.Ordinal);
        Assert.DoesNotContain("model", json, StringComparison.Ordinal);
        Assert.DoesNotContain("generation", json, StringComparison.Ordinal);
        Assert.DoesNotContain("artwork", json, StringComparison.Ordinal);
    }

    private static DeviceConfigEvent Config(
        string mutationId,
        SettingsValue value,
        ConfigDelivery delivery,
        ulong revision = 1) => new(
        null,
        Device,
        new DeliveredComponent<SelectionValue>(1, $"selection-{mutationId}", new SelectionValue(1, SelectionMode.All, []), delivery),
        new DeliveredComponent<SettingsValue>(revision, mutationId, value, delivery),
        new DeliveredComponent<SubscriptionsValue>(1, $"subscriptions-{mutationId}", new SubscriptionsValue(1, []), delivery));
}
