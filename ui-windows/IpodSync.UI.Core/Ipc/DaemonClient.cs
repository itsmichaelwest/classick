using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace IpodSync_UI.Ipc;

/// <summary>
/// Persistent named-pipe client to the running ipod-sync daemon.
/// Replaces M1's <c>CoreProcess</c> (which spawned a per-sync subprocess).
///
/// API contract:
///   - <see cref="ConnectAsync"/> opens the pipe, awaits the hello event,
///     validates protocol_version. Throws if daemon unreachable after retries.
///   - <see cref="Events"/> is a ChannelReader of incoming events (both
///     DaemonEvent and forwarded IpcEvent from the sync subprocess; consumers
///     pattern-match on type).
///   - <see cref="SendAsync"/> writes a command line. Returns when flushed.
///   - <see cref="DisposeAsync"/> closes the pipe; daemon stays running.
/// </summary>
public sealed class DaemonClient : IAsyncDisposable
{
    public const string PipeName = "ipod-sync";
    private static readonly TimeSpan HelloTimeout = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan[] ReconnectBackoff = new[]
    {
        TimeSpan.FromSeconds(1),
        TimeSpan.FromSeconds(2),
        TimeSpan.FromSeconds(4),
    };

    private readonly NamedPipeClientStream _pipe;
    private readonly Channel<object> _events;
    private readonly CancellationTokenSource _cts;
    private readonly Task _readerTask;
    private int _disposed;

    public ChannelReader<object> Events => _events.Reader;

    private DaemonClient(NamedPipeClientStream pipe, Channel<object> events, CancellationTokenSource cts, Task readerTask)
    {
        _pipe = pipe;
        _events = events;
        _cts = cts;
        _readerTask = readerTask;
    }

    public static async Task<DaemonClient> ConnectAsync(CancellationToken cancellationToken = default)
    {
        Exception? lastException = null;
        foreach (var delay in ReconnectBackoff)
        {
            try
            {
                var pipe = new NamedPipeClientStream(
                    ".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
                await pipe.ConnectAsync(2000, cancellationToken).ConfigureAwait(false);

                var events = Channel.CreateUnbounded<object>(new UnboundedChannelOptions
                {
                    SingleReader = true,
                    SingleWriter = true,
                });
                var cts = new CancellationTokenSource();
                var readerTask = Task.Run(() => ReaderLoop(pipe, events.Writer, cts.Token));

                var client = new DaemonClient(pipe, events, cts, readerTask);
                // Await hello.
                using var helloTimeout = new CancellationTokenSource(HelloTimeout);
                using var linked = CancellationTokenSource.CreateLinkedTokenSource(
                    cancellationToken, helloTimeout.Token);
                var first = await events.Reader.ReadAsync(linked.Token).ConfigureAwait(false);
                if (first is not HelloEvent hello)
                {
                    await client.DisposeAsync().ConfigureAwait(false);
                    throw new InvalidOperationException($"expected hello, got {first.GetType().Name}");
                }
                if (!hello.ProtocolVersion.StartsWith("1.", StringComparison.Ordinal))
                {
                    await client.DisposeAsync().ConfigureAwait(false);
                    throw new InvalidOperationException(
                        $"daemon protocol {hello.ProtocolVersion} not supported by UI");
                }
                return client;
            }
            catch (Exception e)
            {
                lastException = e;
                Debug.WriteLine($"daemon-client: connect attempt failed: {e.Message}; backing off {delay.TotalSeconds}s");
                await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            }
        }
        throw new InvalidOperationException(
            $"daemon unreachable after {ReconnectBackoff.Length} attempts", lastException);
    }

    private static async Task ReaderLoop(NamedPipeClientStream pipe, ChannelWriter<object> writer, CancellationToken ct)
    {
        try
        {
            using var reader = new StreamReader(pipe, new UTF8Encoding(false), leaveOpen: true);
            while (!ct.IsCancellationRequested)
            {
                var line = await reader.ReadLineAsync(ct).ConfigureAwait(false);
                if (line is null) break;
                if (string.IsNullOrWhiteSpace(line)) continue;
                if (TryDeserialize(line, out var evt))
                {
                    await writer.WriteAsync(evt!, ct).ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException) { /* expected on dispose */ }
        catch (Exception e)
        {
            Debug.WriteLine($"daemon-client: reader loop terminated: {e.Message}");
        }
        finally { writer.TryComplete(); }
    }

    private static bool TryDeserialize(string line, out object? evt)
    {
        // Try the M1 IpcEvent hierarchy first (it owns Hello + all sync-
        // subprocess events). Then try DaemonEvent (status/config/etc.).
        try
        {
            evt = JsonSerializer.Deserialize<IpcEvent>(line);
            if (evt is not null) return true;
        }
        catch (JsonException) { /* fall through */ }
        try
        {
            evt = JsonSerializer.Deserialize<DaemonEvent>(line);
            if (evt is not null) return true;
        }
        catch (JsonException jx)
        {
            Debug.WriteLine($"daemon-client: unparseable line `{line}`: {jx.Message}");
        }
        evt = null;
        return false;
    }

    public async Task SendAsync(DaemonCommand command, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(command);
        var json = JsonSerializer.Serialize<DaemonCommand>(command);
        var bytes = Encoding.UTF8.GetBytes(json + "\n");
        await _pipe.WriteAsync(bytes, cancellationToken).ConfigureAwait(false);
        await _pipe.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        _cts.Cancel();
        try { await _readerTask.ConfigureAwait(false); } catch { /* expected */ }
        _pipe.Dispose();
        _cts.Dispose();
    }
}
