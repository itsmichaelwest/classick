using System.Diagnostics;
using System.IO.Pipes;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;

namespace Classick_UI.Ipc;

public sealed class WireCompatibilityException(string message) : InvalidOperationException(message);

public sealed class DaemonClient : IAsyncDisposable
{
    public const string PipeName = Classick_UI.Core.AppIdentity.Name;
    private static readonly TimeSpan HelloTimeout = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan[] ReconnectBackoff =
        [TimeSpan.FromSeconds(1), TimeSpan.FromSeconds(2), TimeSpan.FromSeconds(4)];

    private readonly NamedPipeClientStream _pipe;
    private readonly StreamReader _reader;
    private readonly Channel<WireEvent> _events;
    private readonly CancellationTokenSource _cts;
    private readonly Task _readerTask;
    private int _disposed;

    public ChannelReader<WireEvent> Events => _events.Reader;
    public WireHello PeerHello { get; }

    private DaemonClient(
        NamedPipeClientStream pipe,
        StreamReader reader,
        Channel<WireEvent> events,
        CancellationTokenSource cts,
        WireHello peerHello)
    {
        _pipe = pipe;
        _reader = reader;
        _events = events;
        _cts = cts;
        PeerHello = peerHello;
        _readerTask = Task.Run(() => ReaderLoopAsync(reader, events.Writer, cts.Token));
    }

    public static async Task<DaemonClient> ConnectAsync(CancellationToken cancellationToken = default)
    {
        Exception? lastException = null;
        foreach (var delay in ReconnectBackoff)
        {
            NamedPipeClientStream? pipe = null;
            StreamReader? reader = null;
            try
            {
                pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
                await pipe.ConnectAsync(2000, cancellationToken).ConfigureAwait(false);
                reader = new StreamReader(pipe, new UTF8Encoding(false), leaveOpen: true);
                using var helloTimeout = new CancellationTokenSource(HelloTimeout);
                using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, helloTimeout.Token);
                var firstLine = await reader.ReadLineAsync(linked.Token).ConfigureAwait(false) ??
                    throw new WireCompatibilityException("Classick core closed before its protocol handshake");
                WireHello hello;
                try
                {
                    hello = WireCodec.DecodeInitialHello(firstLine);
                    WireCodec.ValidatePeerHello(hello, EndpointRole.Daemon, WireCodec.RequiredDaemonCapabilities);
                }
                catch (Exception exception) when (exception is JsonException or InvalidOperationException)
                {
                    throw new WireCompatibilityException($"This Classick app cannot communicate with the installed core: {exception.Message}");
                }

                var events = Channel.CreateUnbounded<WireEvent>(new UnboundedChannelOptions
                {
                    SingleReader = true,
                    SingleWriter = true,
                });
                return new DaemonClient(pipe, reader, events, new CancellationTokenSource(), hello);
            }
            catch (WireCompatibilityException)
            {
                reader?.Dispose();
                pipe?.Dispose();
                throw;
            }
            catch (Exception exception)
            {
                reader?.Dispose();
                pipe?.Dispose();
                lastException = exception;
                Debug.WriteLine($"daemon-client: connect attempt failed: {exception.Message}; backing off {delay.TotalSeconds}s");
                await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            }
        }
        throw new InvalidOperationException(
            $"daemon unreachable after {ReconnectBackoff.Length} attempts", lastException);
    }

    public static bool IsProtocolVersionSupported(string protocolVersion)
    {
        try
        {
            var hello = new WireHello
            {
                ProtocolVersion = protocolVersion,
                Role = EndpointRole.Daemon,
                SoftwareVersion = "0.0.0",
                Capabilities = WireCodec.RequiredDaemonCapabilities,
            };
            WireCodec.ValidatePeerHello(hello, EndpointRole.Daemon, WireCodec.RequiredDaemonCapabilities);
            return true;
        }
        catch (Exception exception) when (exception is JsonException or InvalidOperationException)
        {
            return false;
        }
    }

    private static async Task ReaderLoopAsync(
        StreamReader reader,
        ChannelWriter<WireEvent> writer,
        CancellationToken cancellationToken)
    {
        try
        {
            await foreach (var wireEvent in ReadAdmittedEventsAsync(reader, cancellationToken).ConfigureAwait(false))
            {
                await writer.WriteAsync(wireEvent, cancellationToken).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch (Exception exception)
        {
            Debug.WriteLine($"daemon-client: reader loop terminated: {exception.Message}");
        }
        finally
        {
            writer.TryComplete();
        }
    }

    internal static async IAsyncEnumerable<WireEvent> ReadAdmittedEventsAsync(
        TextReader reader,
        [System.Runtime.CompilerServices.EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            var line = await reader.ReadLineAsync(cancellationToken).ConfigureAwait(false);
            if (line is null) yield break;
            if (string.IsNullOrWhiteSpace(line)) continue;
            WireDecodeResult decoded;
            try
            {
                decoded = WireCodec.DecodeAdmittedMessage(line, WireStream.DesktopReceivingDaemonEvents);
            }
            catch (Exception exception) when (exception is JsonException or InvalidOperationException or FormatException or ArgumentException or NullReferenceException)
            {
                Debug.WriteLine($"daemon-client: unparseable line `{line}`: {exception.Message}");
                continue;
            }
            if (decoded is KnownWireMessage { Message: WireEvent wireEvent })
            {
                yield return wireEvent;
            }
        }
    }

    public async Task SendAsync(WireCommand command, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(command);
        var bytes = Encoding.UTF8.GetBytes(WireCodec.Encode(command) + "\n");
        await _pipe.WriteAsync(bytes, cancellationToken).ConfigureAwait(false);
        await _pipe.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        _cts.Cancel();
        try
        {
            await _readerTask.ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        _reader.Dispose();
        _pipe.Dispose();
        _cts.Dispose();
    }
}
