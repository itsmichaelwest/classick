using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

public partial class PopoverViewModel : ObservableObject
{
    [ObservableProperty] private string statusText = "iPod not connected";
    [ObservableProperty] private string deviceLabel = "";
    [ObservableProperty] private bool syncing;
    [ObservableProperty] private int progressCurrent;
    [ObservableProperty] private int progressTotal;
    [ObservableProperty] private string currentTrackLabel = "";

    public ObservableCollection<HistoryEntryViewModel> Recent { get; } = new();

    public void Update(StatusUpdateEvent s)
    {
        Syncing = s.State == "syncing";
        if (Syncing)
        {
            StatusText = "Syncing iPod…";
            return;
        }
        if (!s.IpodConnected)
        {
            StatusText = "iPod not connected";
            return;
        }
        // Idle + connected.
        var last = s.LastSync;
        if (last is not null && last.Outcome != "ok")
        {
            StatusText = $"Last sync failed: {last.ErrorMessage ?? "unknown error"}";
        }
        else
        {
            StatusText = last is null
                ? "Up to date · iPod connected"
                : $"Up to date · last sync {RelativeTime(last.Timestamp)}";
        }
    }

    public void ApplyHistory(HistoryUpdateEvent h)
    {
        Recent.Clear();
        // Newest 5.
        var start = Math.Max(0, h.Entries.Count - 5);
        for (int i = h.Entries.Count - 1; i >= start; i--)
        {
            Recent.Add(new HistoryEntryViewModel(h.Entries[i]));
        }
    }

    public void ApplyIpcProgress(IpcEvent evt)
    {
        switch (evt)
        {
            case TrackStartEvent t:
                ProgressCurrent = t.Current;
                ProgressTotal = t.Total;
                CurrentTrackLabel = t.Label;
                break;
        }
    }

    private static string RelativeTime(string rfc3339)
    {
        if (!DateTimeOffset.TryParse(rfc3339, out var dt)) return "recently";
        var delta = DateTimeOffset.UtcNow - dt;
        if (delta.TotalMinutes < 1) return "just now";
        if (delta.TotalMinutes < 60) return $"{(int)delta.TotalMinutes} min ago";
        if (delta.TotalHours < 24) return $"{(int)delta.TotalHours} hr ago";
        return $"{(int)delta.TotalDays} days ago";
    }
}

// HistoryEntryViewModel canonicalized in SettingsViewModel.cs (T9).
