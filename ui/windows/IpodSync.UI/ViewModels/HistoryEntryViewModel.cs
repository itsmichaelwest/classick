using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// Row-level VM for a single history entry. Shared by
/// <see cref="PopoverViewModel"/>'s recent-activity feed and
/// <see cref="SettingsHistoryViewModel"/>'s full list.
/// </summary>
public partial class HistoryEntryViewModel : ObservableObject
{
    public HistoryEntryViewModel(HistoryEntry e)
    {
        Timestamp = e.Timestamp;
        DurationSecs = e.DurationSecs;
        Trigger = e.Trigger;
        Outcome = e.Outcome;
        ErrorMessage = e.ErrorMessage;
        Summary = e.Summary;
    }

    public string Timestamp { get; }
    public ulong DurationSecs { get; }
    public string Trigger { get; }
    public string Outcome { get; }
    public string? ErrorMessage { get; }
    public SyncSummary? Summary { get; }

    public string OutcomeGlyph => Outcome switch
    {
        "ok"      => "✓",  // check
        "error"   => "!",
        "aborted" => "✗",  // cross
        _         => "?",
    };

    public string SummaryText => Summary is null
        ? (ErrorMessage ?? "")
        : $"+{Summary.Add} ~{Summary.Modify} -{Summary.Remove}" +
          (Summary.Skipped > 0 ? $", {Summary.Skipped} skipped" : "");

    public string DurationText => DurationSecs < 60
        ? $"{DurationSecs}s"
        : $"{DurationSecs / 60}m {DurationSecs % 60}s";
}
