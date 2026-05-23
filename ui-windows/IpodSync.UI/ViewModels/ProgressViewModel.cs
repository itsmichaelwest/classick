using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// Backs the Progress page: models the sync apply loop's UI state. Header
/// paths (from <see cref="HeaderEvent"/>), the action-plan denominator and
/// completed count (from <see cref="SummaryEvent"/> + <see cref="TrackDoneEvent"/>)
/// driving a determinate <c>ProgressBar</c>, the current track label (from
/// <see cref="TrackStartEvent"/>), a capped scrolling log tail combining
/// <see cref="LogEvent"/> and <see cref="ErrorEvent"/>, and a final
/// <see cref="IsFinished"/> + <see cref="FinishedSuccessfully"/> state
/// (from <see cref="FinishEvent"/>) that surfaces a Done InfoBar with an
/// eject/recovery hint.
///
/// <para>
/// <b>UI-thread contract:</b> all <c>Apply*</c> mutators touch observable
/// state and an <see cref="ObservableCollection{T}"/>, so they MUST be
/// invoked on the UI thread. IPC events arrive on a background channel
/// reader thread; the host (typically the Page or App) must marshal each
/// event through <c>App.DispatcherQueue.TryEnqueue(...)</c> before calling
/// in. Calling from a worker thread will throw
/// <see cref="System.Runtime.InteropServices.COMException"/> RPC_E_WRONG_THREAD
/// or silently corrupt collection-changed notifications.
/// </para>
///
/// <para>
/// The VM does NOT talk to <c>CoreProcess</c> directly — it stays pure and
/// unit-testable. The host wires <c>ICoreProcess</c> events to the
/// corresponding <c>Apply*</c> methods.
/// </para>
/// </summary>
public partial class ProgressViewModel : ObservableObject
{
    // Header info (from HeaderEvent — typically set before sync starts).
    [ObservableProperty] private string source = "";
    [ObservableProperty] private string ipod = "";
    [ObservableProperty] private string manifest = "";

    // Plan summary (from SummaryEvent — drives the progress bar denominator).
    [ObservableProperty] private int totalPlanned;
    [ObservableProperty] private int done;

    // Current track (from TrackStartEvent).
    [ObservableProperty] private string currentTrackLabel = "";
    [ObservableProperty] private int currentTrackIndex;
    [ObservableProperty] private int currentTrackTotal;

    // Final state (from FinishEvent).
    [ObservableProperty] private bool isFinished;
    [ObservableProperty] private bool finishedSuccessfully;
    [ObservableProperty] private string finishMessage = "";

    /// <summary>
    /// Scrolling log tail. Append-only with a capped size; oldest entries
    /// are removed when <see cref="MaxLogTailEntries"/> is exceeded so the
    /// UI doesn't unbound-grow across a long sync.
    /// </summary>
    public ObservableCollection<LogLine> LogTail { get; } = new();

    private const int MaxLogTailEntries = 200;

    /// <summary>0..100. Falls back to 0 when no plan is loaded yet.</summary>
    public double PercentComplete => TotalPlanned == 0 ? 0 : (Done * 100.0) / TotalPlanned;

    /// <summary>True while a sync is in progress (plan loaded, not yet finished).</summary>
    public bool IsBusy => !IsFinished && TotalPlanned > 0;

    // Notify computed properties when their inputs change.
    partial void OnDoneChanged(int value) => OnPropertyChanged(nameof(PercentComplete));
    partial void OnTotalPlannedChanged(int value)
    {
        OnPropertyChanged(nameof(PercentComplete));
        OnPropertyChanged(nameof(IsBusy));
    }
    partial void OnIsFinishedChanged(bool value) => OnPropertyChanged(nameof(IsBusy));

    /// <summary>
    /// Apply a header (typically before the sync starts). Call on UI thread.
    /// </summary>
    public void ApplyHeader(HeaderEvent header)
    {
        Source = header.Source;
        Ipod = header.Ipod;
        Manifest = header.Manifest;
    }

    /// <summary>
    /// Apply a Summary event. Resets Done to 0 and sets the denominator.
    /// Call on UI thread.
    /// </summary>
    public void ApplySummary(SummaryEvent summary)
    {
        TotalPlanned = summary.TotalPlanned;
        Done = 0;
    }

    /// <summary>
    /// Apply a TrackStart event. Call on UI thread.
    /// </summary>
    public void ApplyTrackStart(TrackStartEvent evt)
    {
        CurrentTrackIndex = evt.Current;
        CurrentTrackTotal = evt.Total;
        CurrentTrackLabel = evt.Label;
    }

    /// <summary>
    /// Apply a TrackDone event. Call on UI thread.
    /// </summary>
    public void ApplyTrackDone()
    {
        Done++;
    }

    /// <summary>
    /// Append a Log event to the tail (capped at <see cref="MaxLogTailEntries"/>).
    /// Call on UI thread.
    /// </summary>
    public void ApplyLog(LogEvent evt)
    {
        AppendLog(LogLevel.Info, evt.Message);
    }

    /// <summary>
    /// Append an Error event to the tail. Call on UI thread.
    /// </summary>
    public void ApplyError(ErrorEvent evt)
    {
        AppendLog(LogLevel.Error, evt.Message);
    }

    /// <summary>
    /// Mark the sync finished. Call on UI thread. On success, snaps Done to
    /// TotalPlanned so the progress bar visually completes (the wire-side
    /// final TrackDone may have already done this, but we don't assume).
    /// </summary>
    public void ApplyFinish(FinishEvent evt)
    {
        IsFinished = true;
        FinishedSuccessfully = evt.Success;
        FinishMessage = evt.Success
            ? "Sync complete. Eject the iPod before unplugging."
            : "Sync failed. See log for details; re-run with --rebuild-manifest if the iPod is in an inconsistent state.";
        if (evt.Success && TotalPlanned > 0)
        {
            Done = TotalPlanned;
        }
    }

    private void AppendLog(LogLevel level, string message)
    {
        LogTail.Add(new LogLine(DateTimeOffset.Now, level, message));
        while (LogTail.Count > MaxLogTailEntries)
        {
            LogTail.RemoveAt(0);
        }
    }

    /// <summary>
    /// Raised when the user clicks Close after the sync finishes. The host
    /// can navigate away or call <c>Window.Close()</c>.
    /// </summary>
    public event Action? CloseRequested;

    [RelayCommand]
    private void Close()
    {
        CloseRequested?.Invoke();
    }
}

/// <summary>Severity tag for entries in <see cref="ProgressViewModel.LogTail"/>.</summary>
public enum LogLevel
{
    Info,
    Error,
}

/// <summary>
/// One entry in the Progress page's scrolling log tail. Immutable so list
/// item virtualization can rely on identity.
/// </summary>
public sealed record LogLine(DateTimeOffset Timestamp, LogLevel Level, string Message);
