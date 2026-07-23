using System.Diagnostics;
using System.Threading.Channels;

namespace Classick_UI.Ipc;

public sealed class DaemonEventRouter : IDisposable
{
    private readonly ChannelReader<WireEvent> _source;
    private readonly Dictionary<DeviceId, ulong> _activeSessions = [];
    private readonly HashSet<DeviceId> _awaitingFinished = [];
    private CancellationTokenSource? _cts;
    private Task? _loop;
    private ulong? _inventoryRevision;

    public DaemonEventRouter(ChannelReader<WireEvent> source) => _source = source;

    public event Action<WireEvent>? EventReceived;
    public event Action<DeviceInventoryEvent>? DeviceInventoryReceived;

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
            await foreach (var wireEvent in _source.ReadAllAsync(cancellationToken).ConfigureAwait(false))
            {
                Route(wireEvent);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
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
        if (wireEvent is SyncPausedEvent or SyncCancelledEvent)
        {
            _awaitingFinished.Add(routed.DeviceId);
        }
        else if (wireEvent is SyncFinishedEvent)
        {
            _activeSessions.Remove(routed.DeviceId);
            _awaitingFinished.Remove(routed.DeviceId);
        }
    }

    private bool UpdateActiveSessions(DeviceInventoryEvent inventory)
    {
        if (_inventoryRevision is not null && inventory.Revision < _inventoryRevision)
        {
            Debug.WriteLine($"daemon-event-router: ignored stale inventory revision {inventory.Revision}");
            return false;
        }

        _inventoryRevision = inventory.Revision;
        var previous = _activeSessions.ToArray();
        _activeSessions.Clear();
        foreach (var device in inventory.Devices)
        {
            if (device.SessionId is { } sessionId)
            {
                _activeSessions[device.DeviceId] = sessionId;
                _awaitingFinished.Remove(device.DeviceId);
            }
        }
        var present = inventory.Devices.Select(device => device.DeviceId).ToHashSet();
        foreach (var (deviceId, sessionId) in previous)
        {
            if (present.Contains(deviceId) && _awaitingFinished.Contains(deviceId))
            {
                _activeSessions.TryAdd(deviceId, sessionId);
            }
        }
        _awaitingFinished.RemoveWhere(deviceId => !present.Contains(deviceId));
        return true;
    }

    public void Dispose() => Stop();
}
