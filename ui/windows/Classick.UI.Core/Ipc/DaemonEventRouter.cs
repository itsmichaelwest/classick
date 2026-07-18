using System;
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
    private CancellationTokenSource? _cts;
    private Task? _readerTask;

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
    public event Action<IpcEvent>? IpcEventReceived;

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
                DeviceInventorySnapshotReceived?.Invoke(snapshot);
                break;
            case SyncEventEnvelope env:
                // Re-parse the wrapped line as an M1 IpcEvent and
                // dispatch via the IpcEvent channel.
                try
                {
                    var inner = JsonSerializer.Deserialize<IpcEvent>(env.Line);
                    if (inner is not null) IpcEventReceived?.Invoke(inner);
                }
                catch (Exception e)
                {
                    Debug.WriteLine($"daemon-event-router: bad sync_event line `{env.Line}`: {e.Message}");
                }
                break;
            case IpcEvent ie:
                // M1 events that arrive directly (e.g. Hello during
                // connect already consumed by DaemonClient; this
                // covers daemon-forwarded events that the daemon
                // happens to emit un-wrapped — defensive).
                IpcEventReceived?.Invoke(ie);
                break;
            default:
                Debug.WriteLine($"daemon-event-router: unrouted event type {evt.GetType().Name}");
                break;
        }
    }

    public void Dispose() => Stop();
}
