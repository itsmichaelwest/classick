using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceActivityPresentationTests
{
    private static readonly DeviceId First = DeviceId.Parse("000A27002138B0A8");
    private static readonly DeviceId Second = DeviceId.Parse("000A27002138B0A9");

    [Fact]
    public void ConcurrentSessions_AreAggregatedWithoutMergingTheirTrackProgress()
    {
        var store = new DeviceStore();
        store.Reduce(new DeviceInventoryEvent(null, 1, [Device(First, "First iPod", 7), Device(Second, "Second iPod", 8)], []));

        var result = DeviceActivityPresentationFactory.For(store.Devices.Values);

        Assert.Equal(AggregateDeviceActivity.Syncing, result.Activity);
        Assert.Equal("2 iPods syncing…", result.Tooltip);
    }

    [Fact]
    public void SoleSession_UsesTheAppleOwnedDeviceName()
    {
        var store = new DeviceStore();
        store.Reduce(new DeviceInventoryEvent(null, 1, [Device(First, "Michael's iPod", 7)], []));

        var result = DeviceActivityPresentationFactory.For(store.Devices.Values);

        Assert.Equal("Syncing Michael's iPod…", result.Tooltip);
        Assert.DoesNotContain(First.Value, result.Tooltip);
    }

    private static IdentifiedDeviceSnapshot Device(DeviceId id, string name, ulong? sessionId) => new(
        id,
        name,
        DeviceReadiness.Ready,
        new HardwareFacts(),
        ProfileStatus.Adopted,
        true,
        "D:\\",
        sessionId is null ? DevicePhase.Idle : DevicePhase.Syncing,
        sessionId,
        null,
        0,
        null,
        null);
}
