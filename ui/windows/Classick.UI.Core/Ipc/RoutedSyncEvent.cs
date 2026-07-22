namespace Classick_UI.Ipc;

public sealed record RoutedSyncEvent
{
    public RoutedSyncEvent(DeviceId deviceId, ulong sessionId, WireEvent wireEvent)
    {
        DeviceId = deviceId;
        SessionId = sessionId;
        Context = new SyncEventContext(sessionId, deviceId.Value);
        Event = wireEvent;
    }

    public RoutedSyncEvent(SyncEventContext context, IpcEvent wireEvent)
    {
        Context = context;
        SessionId = context.SessionId;
        DeviceId = null;
        Event = wireEvent;
    }

    public DeviceId? DeviceId { get; }
    public ulong SessionId { get; }
    public SyncEventContext Context { get; }
    public IpcEvent Event { get; }
}
