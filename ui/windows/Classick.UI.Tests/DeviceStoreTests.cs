using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceStoreTests
{
    private static readonly DeviceId First = DeviceId.Parse("000A27002138B0A8");
    private static readonly DeviceId Second = DeviceId.Parse("000A27002138B0A9");

    [Fact]
    public void InterleavedDeviceState_ReducesIndependently()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First, sessionId: 41), Device(Second, sessionId: 52)));
        store.Reduce(new DeviceConfigEvent(null, First, Selection(), Settings(), Subscriptions()));
        store.Reduce(new WireTrackStartEvent(Second, 52, 3, 10, "second"));
        store.Reduce(new WireTrackStartEvent(First, 41, 2, 8, "first"));

        Assert.NotNull(store.Devices[First].Config);
        Assert.Null(store.Devices[Second].Config);
        Assert.Equal("first", Assert.IsType<WireTrackStartEvent>(store.Devices[First].LastProgress).Label);
        Assert.Equal("second", Assert.IsType<WireTrackStartEvent>(store.Devices[Second].LastProgress).Label);
    }

    [Fact]
    public void DeviceConfigArrivingBeforeInventory_IsRetainedForThatDevice()
    {
        var store = new DeviceStore();
        var config = new DeviceConfigEvent(null, First, Selection(), Settings(), Subscriptions());

        store.Reduce(config);
        store.Reduce(Inventory(1, Device(First)));

        Assert.Same(config, store.Devices[First].Config);
    }

    [Fact]
    public void HistorySnapshot_ClearsDevicesOmittedFromLaterResponse()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First), Device(Second)));
        store.Reduce(new HistoryEvent(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8830",
            [History(First), History(Second)]));

        store.Reduce(new HistoryEvent(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8831",
            [History(Second)]));

        Assert.Empty(store.Devices[First].History);
        Assert.Single(store.Devices[Second].History);
    }

    [Fact]
    public void ReconnectAndMountChange_PreserveDeviceEntry()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First, connected: false, mount: null)));
        var entry = store.Devices[First];

        store.Reduce(Inventory(2, Device(First, connected: true, mount: "E:\\")));

        Assert.Same(entry, store.Devices[First]);
        Assert.True(entry.Inventory.Connected);
        Assert.Equal("E:\\", entry.Inventory.MountPath);
    }

    [Fact]
    public void StaleSessionProgress_CannotUpdateNewSession()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First, sessionId: 72)));

        Assert.False(store.Reduce(new SyncLogEvent(First, 71, "stale")));
        Assert.True(store.Reduce(new SyncLogEvent(First, 72, "current")));

        Assert.Equal("current", Assert.IsType<SyncLogEvent>(store.Devices[First].LastProgress).Message);
    }

    [Fact]
    public void PausedProgress_ClearsTerminatedSession()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First, sessionId: 72)));

        store.Reduce(new SyncPausedEvent(First, 72));

        Assert.Null(store.Devices[First].ActiveSessionId);
        Assert.Null(store.CaptureFocusedSessionAction());
    }

    [Fact]
    public void InventoryGeneration_ReplacesUnidentifiedObservationsWithoutPromotion()
    {
        var store = new DeviceStore();
        store.Reduce(new DeviceInventoryEvent(null, 1, [], [Unidentified(7)]));
        store.Reduce(new DeviceInventoryEvent(null, 2, [Device(First)], [Unidentified(8)]));

        Assert.False(store.Unidentified.ContainsKey(7));
        Assert.True(store.Unidentified.ContainsKey(8));
        Assert.True(store.Devices.ContainsKey(First));
    }

    [Fact]
    public void Focus_IsDeterministicAndAmbiguityDisablesMutation()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First), Device(Second)));
        Assert.Null(store.CaptureFocusedDeviceAction());

        store.SelectDevice(Second);
        Assert.Equal(Second, store.CaptureFocusedDeviceAction()!.DeviceId);

        store.Reduce(Inventory(2, Device(First, sessionId: 81), Device(Second)));
        Assert.Equal(First, store.FocusedDeviceId);
        Assert.Equal(new DeviceSessionTarget(First, 81), store.CaptureFocusedSessionAction());
    }

    [Fact]
    public void ActiveSyncAlreadyInFocus_WinsWhenAnotherSyncAppears()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First, sessionId: 91), Device(Second)));
        Assert.Equal(First, store.FocusedDeviceId);

        store.Reduce(Inventory(2, Device(First, sessionId: 91), Device(Second, sessionId: 92)));

        Assert.Equal(First, store.FocusedDeviceId);
    }

    [Fact]
    public void CapturedAction_DoesNotRetargetAfterForgetOrDisconnect()
    {
        var store = new DeviceStore();
        store.Reduce(Inventory(1, Device(First), Device(Second, connected: false)));
        var captured = store.CaptureFocusedDeviceAction();

        store.Reduce(new DeviceForgottenEvent(First, "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8808"));
        store.Reduce(Inventory(2, Device(Second)));

        Assert.Equal(First, captured!.DeviceId);
        Assert.Equal(Second, store.CaptureFocusedDeviceAction()!.DeviceId);
    }

    private static DeviceInventoryEvent Inventory(ulong revision, params IdentifiedDeviceSnapshot[] devices) =>
        new(null, revision, devices, []);

    private static IdentifiedDeviceSnapshot Device(
        DeviceId id,
        bool connected = true,
        string? mount = "D:\\",
        ulong? sessionId = null) =>
        new(
            id,
            id == First ? "First iPod" : "Second iPod",
            DeviceReadiness.Ready,
            new HardwareFacts(),
            ProfileStatus.Adopted,
            connected,
            connected ? mount : null,
            sessionId is null ? DevicePhase.Idle : DevicePhase.Syncing,
            sessionId,
            null,
            0,
            null,
            null);

    private static UnidentifiedDeviceSnapshot Unidentified(ulong id) =>
        new(id, DeviceReadiness.IdentityUnavailable, new HardwareFacts());

    private static DeliveredComponent<SelectionValue> Selection() =>
        new(1, "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8811", new SelectionValue(1, SelectionMode.All, []), new DeviceCommittedDelivery());

    private static DeliveredComponent<SettingsValue> Settings() =>
        new(1, "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8812", new SettingsValue(1, false, false), new DeviceCommittedDelivery());

    private static DeliveredComponent<SubscriptionsValue> Subscriptions() =>
        new(1, "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8813", new SubscriptionsValue(1, []), new DeviceCommittedDelivery());

    private static WireHistoryEntry History(DeviceId deviceId) => new(
        deviceId,
        null,
        "2026-07-22T12:00:00Z",
        10,
        HistoryTrigger.Manual,
        SyncOperation.Sync,
        SyncOutcome.Ok);
}
