using System;
using System.Threading;
using System.Threading.Tasks;
using IpodSync_UI.Core;
using IpodSync_UI.Dialogs;
using IpodSync_UI.Ipc;
using IpodSync_UI.Views;
using static IpodSync_UI.Core.Diag;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI;

/// <summary>
/// Owns the <see cref="CoreProcess"/> lifecycle and routes incoming IPC events
/// to the right page / view-model. Created by <see cref="MainPage"/> when the
/// user clicks Start; disposed when the sync finishes or the user navigates
/// back via <see cref="ProgressViewModel.CloseRequested"/>.
///
/// <para>
/// Threading: <see cref="CoreProcess.Events"/> is consumed on a background
/// task spawned by <see cref="StartAsync"/>. Every event is marshaled to the
/// UI thread via <see cref="DispatcherQueue"/> before touching any
/// <see cref="ObservableObject"/> or navigating the <see cref="Frame"/>.
/// </para>
///
/// <para>
/// M1 limitation: <see cref="PromptEvent"/> and <see cref="FormEvent"/> abort
/// the sync with a user-visible dialog. Prompt / form rendering lands in M2.
/// </para>
/// </summary>
public sealed class AppController : IAsyncDisposable
{
    private readonly Frame _frame;
    private readonly DispatcherQueue _dispatcher;
    private readonly XamlRoot _xamlRoot;
    private CoreProcess? _coreProcess;
    private HeaderEvent? _stashedHeader;
    private ReviewPage? _reviewPage;
    private ProgressPage? _progressPage;
    private CancellationTokenSource? _readerCts;
    private Task? _readerTask;
    private int _disposed;

    public AppController(Frame frame, DispatcherQueue dispatcher, XamlRoot xamlRoot)
    {
        _frame = frame ?? throw new ArgumentNullException(nameof(frame));
        _dispatcher = dispatcher ?? throw new ArgumentNullException(nameof(dispatcher));
        _xamlRoot = xamlRoot ?? throw new ArgumentNullException(nameof(xamlRoot));
    }

    /// <summary>
    /// Resolve <c>ipod-sync.exe</c> via <see cref="CoreLocator"/>, spawn the
    /// child process, then start the event reader loop. Returns <c>true</c> on
    /// success; <c>false</c> on a user-visible error (which has already been
    /// shown).
    /// </summary>
    public async Task<bool> StartAsync()
    {
        string corePath;
        try
        {
            corePath = CoreLocator.Find();
        }
        catch (CoreNotFoundException ex)
        {
            await CoreNotFoundDialog.ShowAsync(_xamlRoot, ex);
            return false;
        }

        try
        {
            _coreProcess = await CoreProcess.SpawnAsync(corePath, Array.Empty<string>());
        }
        catch (Exception ex)
        {
            await ShowErrorAsync("Failed to start sync", ex.Message);
            return false;
        }

        _readerCts = new CancellationTokenSource();
        _readerTask = Task.Run(() => RunEventLoopAsync(_readerCts.Token));
        return true;
    }

    private async Task RunEventLoopAsync(CancellationToken cancellationToken)
    {
        if (_coreProcess is null) return;
        Log("app: event loop starting");
        try
        {
            await foreach (var evt in _coreProcess.Events.ReadAllAsync(cancellationToken).ConfigureAwait(false))
            {
                // Capture for the closure so the loop variable isn't reused
                // before the dispatched lambda runs.
                var captured = evt;
                Log($"app: dispatching to UI thread: {captured.GetType().Name}");
                await _dispatcher.EnqueueAsync(() => HandleEventOnUIThreadAsync(captured)).ConfigureAwait(false);
                Log($"app: UI handler returned for: {captured.GetType().Name}");
            }

            Log("app: event channel completed (core exited)");

            // Channel completed naturally — process exited. If we haven't
            // already shown a Finish UI, surface "core exited unexpectedly".
            await _dispatcher.EnqueueAsync(async () =>
            {
                if (_progressPage?.ViewModel.IsFinished != true)
                {
                    Log("app: showing 'core exited unexpectedly' dialog");
                    await ShowErrorAsync(
                        "Core exited unexpectedly",
                        "The Rust core process closed its connection before sending a finish event. " +
                        "Check %LOCALAPPDATA%\\ipod-sync\\logs\\ for clues.");
                }
            }).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            Log("app: event loop canceled (DisposeAsync)");
        }
        catch (Exception ex)
        {
            Log($"app: event loop crashed: {ex}");
            await _dispatcher.EnqueueAsync(
                () => ShowErrorAsync("IPC reader crashed", ex.Message)).ConfigureAwait(false);
        }
    }

    private async Task HandleEventOnUIThreadAsync(IpcEvent evt)
    {
        switch (evt)
        {
            case HelloEvent:
                // Already validated in SpawnAsync; ignore.
                break;
            case HeaderEvent header:
                _stashedHeader = header;
                break;
            case ReviewEvent review:
                OnReview(review);
                break;
            case SummaryEvent summary:
                OnSummary(summary);
                break;
            case TrackStartEvent ts:
                EnsureProgressPage().ViewModel.ApplyTrackStart(ts);
                break;
            case TrackDoneEvent:
                EnsureProgressPage().ViewModel.ApplyTrackDone();
                break;
            case LogEvent log:
                EnsureProgressPage().ViewModel.ApplyLog(log);
                break;
            case ErrorEvent err:
                EnsureProgressPage().ViewModel.ApplyError(err);
                break;
            case FinishEvent fin:
                EnsureProgressPage().ViewModel.ApplyFinish(fin);
                break;
            case PromptEvent or FormEvent:
                // M1 limitation: the UI does not render interactive prompts
                // yet. Force-cancel and tell the user. M2 introduces prompt /
                // form dialogs that round-trip the decision back to the core.
                await ShowErrorAsync(
                    "Interactive prompt required",
                    "The core requested a prompt (e.g. ffmpeg not found, iPod unplugged) " +
                    "but the M1 UI doesn't render prompts yet. Use the TUI for now " +
                    "(run ipod-sync.exe from a terminal without --ipc-mode), or wait for M2.\n\n" +
                    "Aborting sync.");
                if (_coreProcess is not null)
                {
                    await _coreProcess.DisposeAsync();
                }
                break;
        }
    }

    private void OnReview(ReviewEvent review)
    {
        Log("app: OnReview entered; constructing ReviewPage");
        _reviewPage = new ReviewPage();
        _reviewPage.ViewModel.LoadFromEvent(review, _stashedHeader);
        _reviewPage.ViewModel.DecisionMade += async cmd =>
        {
            Log($"app: ReviewViewModel.DecisionMade fired: {cmd}");
            if (_coreProcess is not null)
            {
                try
                {
                    Log($"app: calling SendAsync({cmd.GetType().Name})");
                    await _coreProcess.SendAsync(cmd);
                    Log($"app: SendAsync({cmd.GetType().Name}) returned");
                }
                catch (Exception ex)
                {
                    Log($"app: SendAsync threw: {ex}");
                    await _dispatcher.EnqueueAsync(
                        () => ShowErrorAsync("Failed to send decision", ex.Message));
                }
            }
            else
            {
                Log("app: DecisionMade but _coreProcess is null (already disposed?)");
            }
        };
        _frame.Content = _reviewPage;
        Log("app: ReviewPage navigation complete");
    }

    private void OnSummary(SummaryEvent summary)
    {
        var page = EnsureProgressPage();
        if (_stashedHeader is not null)
        {
            page.ViewModel.ApplyHeader(_stashedHeader);
        }
        page.ViewModel.ApplySummary(summary);
    }

    private ProgressPage EnsureProgressPage()
    {
        if (_progressPage is null)
        {
            _progressPage = new ProgressPage();
            _progressPage.ViewModel.CloseRequested += async () =>
            {
                // User clicked Close — tear down and return to the landing page.
                await DisposeAsync();
                _frame.Content = new MainPage();
            };
            _frame.Content = _progressPage;
        }
        return _progressPage;
    }

    private async Task ShowErrorAsync(string title, string message)
    {
        var dialog = new ContentDialog
        {
            Title = title,
            Content = new TextBlock { Text = message, TextWrapping = TextWrapping.Wrap },
            CloseButtonText = "OK",
            XamlRoot = _xamlRoot,
        };
        await dialog.ShowAsync();
    }

    public async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }

        _readerCts?.Cancel();
        if (_readerTask is not null)
        {
            try { await _readerTask.ConfigureAwait(false); }
            catch { /* expected on cancellation */ }
        }
        if (_coreProcess is not null)
        {
            await _coreProcess.DisposeAsync().ConfigureAwait(false);
            _coreProcess = null;
        }
        _readerCts?.Dispose();
        _readerCts = null;
    }
}

internal static class DispatcherQueueExtensions
{
    /// <summary>
    /// Enqueue a synchronous action on the UI thread and await its completion.
    /// Exceptions thrown by <paramref name="action"/> propagate to the caller.
    /// </summary>
    public static Task EnqueueAsync(this DispatcherQueue queue, Action action)
    {
        var tcs = new TaskCompletionSource();
        if (!queue.TryEnqueue(() =>
        {
            try { action(); tcs.SetResult(); }
            catch (Exception ex) { tcs.SetException(ex); }
        }))
        {
            tcs.SetException(new InvalidOperationException("Dispatcher rejected work."));
        }
        return tcs.Task;
    }

    /// <summary>
    /// Enqueue an async function on the UI thread and await its completion.
    /// Exceptions thrown by <paramref name="func"/> propagate to the caller.
    /// </summary>
    public static Task EnqueueAsync(this DispatcherQueue queue, Func<Task> func)
    {
        var tcs = new TaskCompletionSource();
        if (!queue.TryEnqueue(async () =>
        {
            try { await func().ConfigureAwait(true); tcs.SetResult(); }
            catch (Exception ex) { tcs.SetException(ex); }
        }))
        {
            tcs.SetException(new InvalidOperationException("Dispatcher rejected work."));
        }
        return tcs.Task;
    }
}
