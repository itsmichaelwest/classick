namespace Classick_UI.Ipc;

public sealed record SyncEventContext(
    ulong SessionId,
    string? Serial)
{
    public bool IsDeviceSession => !string.IsNullOrWhiteSpace(Serial);
}

public sealed record RoutedSyncEvent(
    SyncEventContext Context,
    IpcEvent Event);
