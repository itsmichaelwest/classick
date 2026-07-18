using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace Classick_UI.Ipc;

/// <summary>
/// Owns the only consumer of <see cref="DaemonClient.Events"/> and
/// dispatches typed events to N concurrent .NET subscribers. Solves
/// the M3 "wizard vs tray loop have exclusive read on the channel"
/// architectural gap.
///
/// Subscribers attach via standard <c>+=</c> on the typed events.
/// All handlers fire on a background task (not the UI thread);
/// subscribers that mutate UI state must marshal via
/// <c>DispatcherQueue.TryEnqueue</c> themselves.
///
/// Lifecycle: <see cref="Start"/> spawns the reader task;
/// <see cref="Stop"/> cancels it. Idempotent on both.
/// </summary>
public sealed class DaemonEventRouter : IDisposable
{
    private readonly ChannelReader<object> _source;
    private readonly Dictionary<string, (string RawSerial, ulong SessionId)> _activeDeviceSessions =
        new(StringComparer.OrdinalIgnoreCase);
    private CancellationTokenSource? _cts;
    private Task? _readerTask;
    private ulong? _inventoryRevision;

    public DaemonEventRouter(ChannelReader<object> source)
    {
        _source = source;
    }

    public event Action<StatusUpdateEvent>? StatusUpdated;
    public event Action<ConfigUpdateEvent>? ConfigUpdated;
    public event Action<HistoryUpdateEvent>? HistoryUpdated;
    public event Action<DeviceConnectedEvent>? DeviceConnected;
    public event Action<DeviceDisconnectedEvent>? DeviceDisconnected;
    public event Action<SyncRejectedEvent>? SyncRejected;
    public event Action<DeviceInventorySnapshotEvent>? DeviceInventorySnapshotReceived;
    public event Action<DaemonEvent>? DaemonEventReceived;
    public event Action<RoutedSyncEvent>? SyncEventReceived;

    public void Start()
    {
        if (_cts is not null) return;
        _cts = new CancellationTokenSource();
        _readerTask = Task.Run(() => ReaderLoop(_cts.Token));
    }

    public async Task StopAsync()
    {
        _cts?.Cancel();
        if (_readerTask is not null)
        {
            try { await _readerTask.ConfigureAwait(false); } catch { /* expected */ }
        }
        _readerTask = null;
        _cts?.Dispose();
        _cts = null;
    }

    public void Stop() => StopAsync().GetAwaiter().GetResult();

    private async Task ReaderLoop(CancellationToken ct)
    {
        try
        {
            await foreach (var evt in _source.ReadAllAsync(ct))
            {
                Dispatch(evt);
            }
        }
        catch (OperationCanceledException) { /* expected */ }
        catch (Exception e)
        {
            Debug.WriteLine($"daemon-event-router: reader terminated: {e}");
        }
    }

    private void Dispatch(object evt)
    {
        if (evt is DaemonEvent daemonEvent)
        {
            DaemonEventReceived?.Invoke(daemonEvent);
        }

        switch (evt)
        {
            case StatusUpdateEvent s:
                StatusUpdated?.Invoke(s);
                break;
            case ConfigUpdateEvent c:
                ConfigUpdated?.Invoke(c);
                break;
            case HistoryUpdateEvent h:
                HistoryUpdated?.Invoke(h);
                break;
            case DeviceConnectedEvent dc:
                DeviceConnected?.Invoke(dc);
                break;
            case DeviceDisconnectedEvent dd:
                DeviceDisconnected?.Invoke(dd);
                break;
            case SyncRejectedEvent sr:
                SyncRejected?.Invoke(sr);
                break;
            case DeviceInventorySnapshotEvent snapshot:
                if (UpdateActiveSessions(snapshot))
                {
                    DeviceInventorySnapshotReceived?.Invoke(snapshot);
                }
                break;
            case SyncEventEnvelope env:
                RouteSyncEvent(env);
                break;
            case IpcEvent ie:
                Debug.WriteLine(
                    $"daemon-event-router: rejected unscoped sync event {ie.GetType().Name}");
                break;
            default:
                Debug.WriteLine($"daemon-event-router: unrouted event type {evt.GetType().Name}");
                break;
        }
    }

    private bool UpdateActiveSessions(DeviceInventorySnapshotEvent snapshot)
    {
        if (_inventoryRevision is not null && snapshot.Revision < _inventoryRevision)
        {
            Debug.WriteLine(
                $"daemon-event-router: ignored stale inventory revision {snapshot.Revision}");
            return false;
        }

        _inventoryRevision = snapshot.Revision;
        _activeDeviceSessions.Clear();
        foreach (var device in snapshot.Devices)
        {
            if (device.SessionId is not { } sessionId)
            {
                continue;
            }

            _activeDeviceSessions[device.Identity.Serial] = (device.Identity.Serial, sessionId);
        }

        return true;
    }

    private void RouteSyncEvent(SyncEventEnvelope envelope)
    {
        var context = new SyncEventContext(envelope.SessionId, envelope.Serial);
        if (envelope.Serial is { } serial)
        {
            if (!_activeDeviceSessions.TryGetValue(serial, out var active) ||
                active.SessionId != envelope.SessionId)
            {
                Debug.WriteLine(
                    $"daemon-event-router: ignored stale sync_event for {serial} session {envelope.SessionId}");
                return;
            }

            context = new SyncEventContext(envelope.SessionId, active.RawSerial);
        }

        try
        {
            var inner = JsonSerializer.Deserialize<IpcEvent>(envelope.Line);
            if (inner is not null)
            {
                SyncEventReceived?.Invoke(new RoutedSyncEvent(context, inner));
            }
        }
        catch (Exception e)
        {
            Debug.WriteLine(
                $"daemon-event-router: bad sync_event line `{envelope.Line}`: {e.Message}");
        }
    }

    public void Dispose() => Stop();
}
