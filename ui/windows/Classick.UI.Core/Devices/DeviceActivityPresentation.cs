using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public enum AggregateDeviceActivity
{
    Offline,
    Idle,
    Syncing,
}

public sealed record DeviceActivityPresentation(AggregateDeviceActivity Activity, string Tooltip);

public sealed record DeviceMountTarget(DeviceId DeviceId, ulong? InventoryRevision, string MountPath);

public static class DeviceActivityPresentationFactory
{
    public static DeviceActivityPresentation For(IEnumerable<DeviceClientState> devices)
    {
        var known = devices.ToArray();
        var active = known.Where(device => device.ActiveSessionId is not null).ToArray();
        if (active.Length > 1)
        {
            return new DeviceActivityPresentation(
                AggregateDeviceActivity.Syncing,
                $"{active.Length} iPods syncing…");
        }
        if (active.Length == 1)
        {
            var name = DevicePresentationFactory.For(active[0].Inventory).Title;
            return new DeviceActivityPresentation(AggregateDeviceActivity.Syncing, $"Syncing {name}…");
        }

        var connected = known.Where(device => device.Inventory.Connected).ToArray();
        if (connected.Length > 1)
        {
            return new DeviceActivityPresentation(
                AggregateDeviceActivity.Idle,
                $"{connected.Length} iPods connected · idle");
        }
        if (connected.Length == 1)
        {
            var name = DevicePresentationFactory.For(connected[0].Inventory).Title;
            return new DeviceActivityPresentation(AggregateDeviceActivity.Idle, $"{name} connected · idle");
        }
        return new DeviceActivityPresentation(AggregateDeviceActivity.Offline, "iPod not connected");
    }
}
