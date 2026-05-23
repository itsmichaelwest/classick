using System;
using System.Collections.Generic;
using System.Diagnostics;
using IpodSync_UI.Core;
using System.IO;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace IpodSync_UI.Ipc;

/// <summary>
/// Manages the <c>ipod-sync.exe --ipc-mode</c> child process and its IPC
/// stream. Spawn-once-use-once: dispose after the sync completes.
/// </summary>
/// <remarks>
/// <para>
/// Wire format and lifecycle rules are documented in
/// <c>docs/ipc-protocol.md</c>. Notable behavior contracts implemented here:
/// </para>
/// <list type="bullet">
///   <item>The <see cref="HelloEvent"/> is awaited and its
///         <c>protocol_version</c> validated before <see cref="SpawnAsync"/>
///         returns. A 5-second deadline (§1, §7) bounds the wait.</item>
///   <item>Stdout lines that fail to parse are logged via
///         <see cref="Debug.WriteLine(string)"/> and skipped (§2 "Unknown
///         messages" — UI tolerates noise; never aborts).</item>
///   <item>Empty / whitespace-only lines are silently skipped (§3, §8).</item>
///   <item>Stderr is collected best-effort for crash diagnostics — see
///         <see cref="CapturedStderr"/>.</item>
///   <item><see cref="DisposeAsync"/> sends <see cref="CancelCommand"/>, waits
///         up to 5 s, then force-kills the entire process tree if it hasn't
///         exited (§7 bounded-join pattern).</item>
/// </list>
/// </remarks>
public sealed class CoreProcess : IAsyncDisposable
{
    /// <summary>Major version the UI accepts. Bump when adopting a new core major.</summary>
    private const string SupportedProtocolMajor = "1.";

    /// <summary>Bounded wait for the hello handshake (§1).</summary>
    private static readonly TimeSpan HelloTimeout = TimeSpan.FromSeconds(5);

    /// <summary>Bounded wait for graceful exit after Cancel (§7).</summary>
    private static readonly TimeSpan ShutdownTimeout = TimeSpan.FromSeconds(5);

    private readonly Process _process;
    private readonly Channel<IpcEvent> _events;
    private readonly CancellationTokenSource _cts;
    private readonly Task _stdoutReader;
    private readonly Task _stderrCollector;
    private readonly List<string> _stderrLines;
    private int _disposed;

    /// <summary>
    /// Stream of IPC events from the core. The <see cref="HelloEvent"/> has
    /// already been consumed and validated by the time <see cref="SpawnAsync"/>
    /// returns; the first event the caller will see is whatever came next on
    /// the wire (typically a <see cref="HeaderEvent"/>).
    /// </summary>
    public ChannelReader<IpcEvent> Events => _events.Reader;

    /// <summary>
    /// Snapshot-friendly view of captured stderr lines. Empty in normal
    /// operation — the core writes structured logs to a file in IPC mode.
    /// Useful for surfacing crash diagnostics.
    /// </summary>
    public IReadOnlyList<string> CapturedStderr
    {
        get
        {
            lock (_stderrLines)
            {
                return _stderrLines.ToArray();
            }
        }
    }

    /// <summary>Underlying process handle. Exposed for diagnostic queries (HasExited, Id, ExitCode).</summary>
    public Process Process => _process;

    private CoreProcess(
        Process process,
        Channel<IpcEvent> events,
        CancellationTokenSource cts,
        Task stdoutReader,
        Task stderrCollector,
        List<string> stderrLines)
    {
        _process = process;
        _events = events;
        _cts = cts;
        _stdoutReader = stdoutReader;
        _stderrCollector = stderrCollector;
        _stderrLines = stderrLines;
    }

    /// <summary>
    /// Spawn <c>ipod-sync.exe --ipc-mode</c>, await the
    /// <see cref="HelloEvent"/>, and validate <c>protocol_version</c>.
    /// </summary>
    /// <param name="corePath">Absolute path to <c>ipod-sync.exe</c>.</param>
    /// <param name="extraArgs">Additional CLI flags to forward (e.g. <c>--source</c>).</param>
    /// <param name="cancellationToken">Caller-supplied cancel for the spawn / hello wait.</param>
    /// <returns>A ready-to-use <see cref="CoreProcess"/>.</returns>
    /// <exception cref="InvalidOperationException">
    /// Thrown if the process fails to start, the hello doesn't arrive within
    /// <see cref="HelloTimeout"/>, the first event isn't a hello, or the
    /// protocol version is unsupported. The child is torn down in all cases.
    /// </exception>
    public static async Task<CoreProcess> SpawnAsync(
        string corePath,
        IReadOnlyList<string> extraArgs,
        CancellationToken cancellationToken = default)
    {
        var psi = new ProcessStartInfo
        {
            FileName = corePath,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        psi.ArgumentList.Add("--ipc-mode");
        foreach (var arg in extraArgs)
        {
            psi.ArgumentList.Add(arg);
        }

        var process = new Process { StartInfo = psi };
        if (!process.Start())
        {
            throw new InvalidOperationException($"Failed to start core process at {corePath}");
        }

        // Stdin encoding has to be set after Start() — ProcessStartInfo's
        // StandardInputEncoding property is honored, but for safety we wrap
        // the underlying BaseStream in a UTF-8 writer with no BOM. The core
        // expects UTF-8 (§3) and most platforms default to the console code
        // page which is wrong for arbitrary unicode in track titles.
        // (Process.StandardInput already respects StandardInputEncoding when
        // set on the PSI on .NET 6+, so this is belt-and-braces.)
        var events = Channel.CreateUnbounded<IpcEvent>(new UnboundedChannelOptions
        {
            SingleReader = true,
            SingleWriter = true,
        });
        var cts = new CancellationTokenSource();
        var stderrLines = new List<string>();

        var stderrCollector = Task.Run(async () =>
        {
            try
            {
                string? line;
                while ((line = await process.StandardError.ReadLineAsync().ConfigureAwait(false)) != null)
                {
                    lock (stderrLines)
                    {
                        stderrLines.Add(line);
                    }
                    Diag.Log($"ipc-stderr: {line}");
                }
            }
            catch (Exception ex)
            {
                Diag.Log($"ipc: stderr collector terminated: {ex.Message}");
            }
        });

        var stdoutReader = Task.Run(async () =>
        {
            try
            {
                string? line;
                while ((line = await process.StandardOutput.ReadLineAsync().ConfigureAwait(false)) != null)
                {
                    // Defensive: §3 says empty lines must be skipped.
                    // ReadLineAsync also drops \r so we don't need to trim it.
                    if (string.IsNullOrWhiteSpace(line))
                    {
                        continue;
                    }

                    IpcEvent? evt;
                    try
                    {
                        evt = JsonSerializer.Deserialize<IpcEvent>(line);
                    }
                    catch (JsonException jx)
                    {
                        // §2: unparseable from the core is a serious bug; log
                        // and keep reading — never abort the UI for this.
                        Diag.Log($"ipc: unparseable event line `{line}`: {jx.Message}");
                        continue;
                    }

                    if (evt is null)
                    {
                        Diag.Log($"ipc: deserializer returned null for line `{line}`");
                        continue;
                    }

                    Diag.Log($"ipc-stdout: {line}");
                    try
                    {
                        await events.Writer.WriteAsync(evt, cts.Token).ConfigureAwait(false);
                    }
                    catch (OperationCanceledException)
                    {
                        // Disposal in progress; stop forwarding.
                        break;
                    }
                }
            }
            catch (Exception ex)
            {
                Diag.Log($"ipc: stdout reader terminated: {ex.Message}");
            }
            finally
            {
                events.Writer.TryComplete();
            }
        });

        var instance = new CoreProcess(process, events, cts, stdoutReader, stderrCollector, stderrLines);

        // Bounded wait for the hello — §1, §7. Tear down on any failure so a
        // half-spawned core doesn't leak.
        using var helloTimeout = new CancellationTokenSource(HelloTimeout);
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, helloTimeout.Token);

        IpcEvent? first;
        try
        {
            first = await events.Reader.ReadAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (helloTimeout.IsCancellationRequested)
        {
            await instance.DisposeAsync().ConfigureAwait(false);
            var stderr = string.Join('\n', instance.CapturedStderr);
            throw new InvalidOperationException(
                $"Core did not emit hello within {HelloTimeout.TotalSeconds:0}s. Captured stderr:\n{stderr}");
        }
        catch (ChannelClosedException)
        {
            await instance.DisposeAsync().ConfigureAwait(false);
            var stderr = string.Join('\n', instance.CapturedStderr);
            throw new InvalidOperationException(
                $"Core exited before emitting hello. Captured stderr:\n{stderr}");
        }

        if (first is not HelloEvent hello)
        {
            await instance.DisposeAsync().ConfigureAwait(false);
            throw new InvalidOperationException(
                $"Expected hello as first event, got {first?.GetType().Name ?? "null"}");
        }

        if (!hello.ProtocolVersion.StartsWith(SupportedProtocolMajor, StringComparison.Ordinal))
        {
            await instance.DisposeAsync().ConfigureAwait(false);
            throw new InvalidOperationException(
                $"Unsupported core protocol version: {hello.ProtocolVersion}. UI supports {SupportedProtocolMajor}x.");
        }

        Diag.Log($"ipc: handshake OK (protocol={hello.ProtocolVersion}, core={hello.CoreVersion})");
        return instance;
    }

    /// <summary>
    /// Serialize <paramref name="command"/> as one JSON line and flush it to
    /// the child's stdin. Returns when the line has been written.
    /// </summary>
    public async Task SendAsync(IpcCommand command, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(command);

        var json = JsonSerializer.Serialize<IpcCommand>(command);
        Diag.Log($"ipc-stdin: {json}");
        await _process.StandardInput.WriteLineAsync(json.AsMemory(), cancellationToken).ConfigureAwait(false);
        await _process.StandardInput.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    /// <summary>
    /// Graceful shutdown: send <see cref="CancelCommand"/>, wait up to
    /// <see cref="ShutdownTimeout"/>, then force-kill the process tree if
    /// it's still alive (§7).
    /// </summary>
    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }

        if (!_process.HasExited)
        {
            try
            {
                await SendAsync(new CancelCommand()).ConfigureAwait(false);
            }
            catch
            {
                // Pipe may already be broken (core crashed / closed stdin).
                // Force-kill below handles that case.
            }

            try
            {
                using var killTimeout = new CancellationTokenSource(ShutdownTimeout);
                await _process.WaitForExitAsync(killTimeout.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                Diag.Log($"ipc: core did not exit within {ShutdownTimeout.TotalSeconds:0}s of cancel; killing");
                try
                {
                    _process.Kill(entireProcessTree: true);
                }
                catch
                {
                    // Race: process may have just exited naturally. Ignore.
                }
            }
        }

        _cts.Cancel();

        try { await _stdoutReader.ConfigureAwait(false); } catch { /* expected on cancellation */ }
        try { await _stderrCollector.ConfigureAwait(false); } catch { /* expected on cancellation */ }

        _process.Dispose();
        _cts.Dispose();
    }
}
