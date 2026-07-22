using System.Diagnostics;
using System.Threading.Channels;

namespace Classick_UI.Ipc;

public sealed class DaemonEventRouter : IDisposable
{
    private readonly Func<CancellationToken, IAsyncEnumerable<object>> _readEvents;
    private readonly Dictionary<DeviceId, ulong> _activeSessions = [];
    private CancellationTokenSource? _cts;
    private Task? _loop;
    private ulong? _inventoryRevision;

    public DaemonEventRouter(ChannelReader<WireEvent> source) =>
        _readEvents = cancellationToken => ReadWireEvents(source, cancellationToken);

    public DaemonEventRouter(ChannelReader<object> source) =>
        _readEvents = cancellationToken => source.ReadAllAsync(cancellationToken);

    public event Action<WireEvent>? EventReceived;
    public event Action<DeviceInventoryEvent>? DeviceInventoryReceived;
    public event Action<StatusUpdateEvent>? StatusUpdated;
    public event Action<ConfigUpdateEvent>? ConfigUpdated;
    public event Action<HistoryUpdateEvent>? HistoryUpdated;
    public event Action<DeviceConnectedEvent>? DeviceConnected;
    public event Action<DeviceDisconnectedEvent>? DeviceDisconnected;
    public event Action<SyncRejectedEvent>? SyncRejected;
    public event Action<DeviceInventorySnapshotEvent>? DeviceInventorySnapshotReceived;
    public event Action<DaemonEvent>? DaemonEventReceived;
    public event Action<RoutedSyncEvent>? SyncEventReceived;
    public event Action<SourceAvailabilityEvent>? SourceAvailabilityUpdated;

    public void Start()
    {
        if (_loop is not null) return;
        _cts = new CancellationTokenSource();
        _loop = RunAsync(_cts.Token);
    }

    public void Stop() => StopAsync().GetAwaiter().GetResult();

    public async Task StopAsync()
    {
        if (_cts is null) return;
        var cts = _cts;
        var loop = _loop;
        cts.Cancel();
        if (loop is not null)
        {
            try
            {
                await loop.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
            }
        }
        if (ReferenceEquals(_cts, cts))
        {
            _cts = null;
            _loop = null;
        }
        cts.Dispose();
    }

    private async Task RunAsync(CancellationToken cancellationToken)
    {
        try
        {
            await foreach (var message in _readEvents(cancellationToken).ConfigureAwait(false))
            {
                switch (message)
                {
                    case WireEvent wireEvent:
                        Route(wireEvent);
                        break;
                    case DaemonEvent daemonEvent:
                        RouteLegacy(daemonEvent);
                        break;
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
    }

    private void RouteLegacy(DaemonEvent daemonEvent)
    {
        DaemonEventReceived?.Invoke(daemonEvent);
        switch (daemonEvent)
        {
            case StatusUpdateEvent status:
                StatusUpdated?.Invoke(status);
                break;
            case ConfigUpdateEvent config:
                ConfigUpdated?.Invoke(config);
                break;
            case HistoryUpdateEvent history:
                HistoryUpdated?.Invoke(history);
                break;
            case DeviceConnectedEvent connected:
                DeviceConnected?.Invoke(connected);
                break;
            case DeviceDisconnectedEvent disconnected:
                DeviceDisconnected?.Invoke(disconnected);
                break;
            case SyncRejectedEvent rejected:
                SyncRejected?.Invoke(rejected);
                break;
            case DeviceInventorySnapshotEvent inventory:
                DeviceInventorySnapshotReceived?.Invoke(inventory);
                break;
            case SourceAvailabilityEvent availability:
                SourceAvailabilityUpdated?.Invoke(availability);
                break;
        }
    }

    internal void Route(WireEvent wireEvent)
    {
        switch (wireEvent)
        {
            case DeviceInventoryEvent inventory when !UpdateActiveSessions(inventory):
                return;
            case DeviceInventoryEvent inventory:
                DeviceInventoryReceived?.Invoke(inventory);
                break;
            case SyncAcceptedEvent accepted:
                _activeSessions[accepted.DeviceId] = accepted.SessionId;
                break;
        }

        if (wireEvent is not ISessionRoutedMessage routed || wireEvent is SyncAcceptedEvent)
        {
            EventReceived?.Invoke(wireEvent);
            return;
        }
        if (!_activeSessions.TryGetValue(routed.DeviceId, out var activeSession) || activeSession != routed.SessionId)
        {
            Debug.WriteLine($"daemon-event-router: ignored stale {wireEvent.GetType().Name} for {routed.DeviceId} session {routed.SessionId}");
            return;
        }
        EventReceived?.Invoke(wireEvent);
        SyncEventReceived?.Invoke(new RoutedSyncEvent(routed.DeviceId, routed.SessionId, wireEvent));
    }

    private bool UpdateActiveSessions(DeviceInventoryEvent inventory)
    {
        if (_inventoryRevision is not null && inventory.Revision < _inventoryRevision)
        {
            Debug.WriteLine($"daemon-event-router: ignored stale inventory revision {inventory.Revision}");
            return false;
        }

        _inventoryRevision = inventory.Revision;
        _activeSessions.Clear();
        foreach (var device in inventory.Devices)
        {
            if (device.SessionId is { } sessionId)
            {
                _activeSessions[device.DeviceId] = sessionId;
            }
        }
        return true;
    }

    private static async IAsyncEnumerable<object> ReadWireEvents(
        ChannelReader<WireEvent> source,
        [System.Runtime.CompilerServices.EnumeratorCancellation] CancellationToken cancellationToken)
    {
        await foreach (var wireEvent in source.ReadAllAsync(cancellationToken).ConfigureAwait(false))
        {
            yield return wireEvent;
        }
    }

    public void Dispose() => Stop();
}
