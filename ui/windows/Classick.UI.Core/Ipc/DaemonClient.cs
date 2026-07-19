using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace Classick_UI.Ipc;

/// <summary>
/// Persistent named-pipe client to the running classick daemon.
/// Replaces M1's <c>CoreProcess</c> (which spawned a per-sync subprocess).
///
/// API contract:
///   - <see cref="ConnectAsync"/> opens the pipe, awaits the hello event,
///     validates protocol_version. Throws if daemon unreachable after retries.
///   - <see cref="Events"/> is a ChannelReader of daemon events. Subprocess
///     events arrive inside <see cref="SyncEventEnvelope"/> values so their
///     device and session identity cannot be separated from the payload.
///   - <see cref="SendAsync"/> writes a command line. Returns when flushed.
///   - <see cref="DisposeAsync"/> closes the pipe; daemon stays running.
/// </summary>
public sealed class DaemonClient : IAsyncDisposable
{
    public const string PipeName = Classick_UI.Core.AppIdentity.Name;
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
                if (!string.Equals(hello.ProtocolVersion, "2.0.0", StringComparison.Ordinal))
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

    private static readonly HashSet<string> DaemonEventDiscriminators = new(StringComparer.Ordinal)
    {
        "status_update", "config_update", "history_update",
        "device_connected", "device_disconnected", "sync_rejected",
        "sync_event", "device_inventory_snapshot", "library_update",
        "selection_update", "selection_preview", "playlists_update",
        "playlist_detail", "device_config_update", "device_preview",
        "resolved_tracks", "source_availability",
    };

    private static bool TryDeserialize(string line, out object? evt)
    {
        // Peek the `type` discriminator and route to the right polymorphic
        // hierarchy. Trying one type then falling back via catch doesn't
        // work cleanly: System.Text.Json throws NotSupportedException (not
        // JsonException) on unknown JsonDerivedType discriminators, and
        // an unhandled NotSupportedException would tear down the reader
        // loop. Peek-then-dispatch is both faster and exception-safe.
        evt = null;
        try
        {
            using var doc = JsonDocument.Parse(line);
            if (!doc.RootElement.TryGetProperty("type", out var typeEl) ||
                typeEl.ValueKind != JsonValueKind.String)
            {
                Debug.WriteLine($"daemon-client: line missing string `type`: {line}");
                return false;
            }
            var discriminator = typeEl.GetString()!;
            if (DaemonEventDiscriminators.Contains(discriminator))
            {
                evt = JsonSerializer.Deserialize<DaemonEvent>(line);
            }
            else
            {
                // Hello uses the subprocess event hierarchy for the handshake.
                // Any direct subprocess event is retained only so the router can
                // reject it as unscoped; v2 progress must use sync_event.
                evt = JsonSerializer.Deserialize<IpcEvent>(line);
            }
            return evt is not null;
        }
        catch (Exception e) when (e is JsonException or NotSupportedException)
        {
            Debug.WriteLine($"daemon-client: unparseable line `{line}`: {e.Message}");
            return false;
        }
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
