using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

public partial class PopoverViewModel : ObservableObject
{
    [ObservableProperty] private string statusText = "iPod not connected";
    [ObservableProperty] private string deviceLabel = "iPod";
    [ObservableProperty] private string lastSyncedLabel = "";
    [ObservableProperty] private bool syncing;
    [ObservableProperty] private bool ipodConnected;
    [ObservableProperty] private int progressCurrent;
    [ObservableProperty] private int progressTotal;
    [ObservableProperty] private string currentTrackLabel = "";
    /// <summary>Latest line of daemon narration during a sync. Used to
    /// give the user signal during the "Preparing…" phase (scan,
    /// fingerprint, plan-build) when there's no track-count yet, and
    /// as a secondary detail line during the apply loop.</summary>
    [ObservableProperty] private string currentLogLine = "";

    // Storage. StorageProgressValue is 0..100 for the ProgressBar.
    // When unknown (no device, or query failed), all three are empty /
    // 0 — the XAML hides the storage row in that case via a binding to
    // HasStorage.
    [ObservableProperty] private string storageUsedLabel = "";
    [ObservableProperty] private string storageFreeLabel = "";
    [ObservableProperty] private double storageProgressValue;
    [ObservableProperty] private bool hasStorage;

    /// <summary>Inverse of <see cref="Syncing"/>, exposed so XAML can bind
    /// the Sync Now button's Visibility without needing a converter.</summary>
    public bool NotSyncing => !Syncing;

    /// <summary>True when the popover should show the storage bar —
    /// idle AND we actually have storage info. Hidden during sync so
    /// the progress block can take its place.</summary>
    public bool ShowStorage => !Syncing && HasStorage;

    /// <summary>True when the popover should render the "no iPod
    /// connected" empty state — centered icon + caption, no storage,
    /// no Sync Now / Eject buttons. Driven by daemon-reported
    /// connection state (see <see cref="Update"/>).</summary>
    public bool ShowEmptyState => !IpodConnected;

    /// <summary>Inverse of <see cref="ShowEmptyState"/>: the normal
    /// connected layout (device row + storage / sync progress + full
    /// footer).</summary>
    public bool ShowConnectedContent => IpodConnected;

    /// <summary>True when the footer should show the Sync Now button —
    /// connected AND idle. Hidden during sync (Stop sync takes its
    /// place) and in the empty state (no device to sync).</summary>
    public bool ShowSyncNowButton => IpodConnected && !Syncing;

    /// <summary>True between sync start and the first SummaryEvent /
    /// TrackStart, so the popover's ProgressBar can render as
    /// indeterminate (marquee) until we know the total count.</summary>
    public bool NoProgressYet => Syncing && ProgressTotal <= 0;

    /// <summary>Human-friendly "Track 12 of 50" / "Preparing…" label
    /// shown beneath the sync progress bar. Empty before a TrackStart
    /// event arrives.</summary>
    public string ProgressLabel
    {
        get
        {
            if (ProgressTotal <= 0) return Syncing ? "Preparing…" : "";
            return $"Track {ProgressCurrent} of {ProgressTotal}";
        }
    }

    /// <summary>Left-side detail caption beneath the progress bar.
    /// Prefers the current track name during the apply loop; falls
    /// back to the latest daemon log line during the prep phase so
    /// the user always sees what's happening instead of a blank
    /// caption beside "Preparing…".</summary>
    public string DetailLine =>
        !string.IsNullOrEmpty(CurrentTrackLabel) ? CurrentTrackLabel : CurrentLogLine;

    /// <summary>Storage labels for display: real values when HasStorage,
    /// em-dash placeholder otherwise. The popover always renders the
    /// storage row so its footprint stays stable while data is loading.</summary>
    public string StorageUsedDisplay => HasStorage ? StorageUsedLabel : "— used";
    public string StorageFreeDisplay => HasStorage ? StorageFreeLabel : "— free";

    partial void OnSyncingChanged(bool value)
    {
        OnPropertyChanged(nameof(NotSyncing));
        OnPropertyChanged(nameof(ShowStorage));
        OnPropertyChanged(nameof(ProgressLabel));
        OnPropertyChanged(nameof(NoProgressYet));
        OnPropertyChanged(nameof(ShowSyncNowButton));
    }
    partial void OnHasStorageChanged(bool value)
    {
        OnPropertyChanged(nameof(ShowStorage));
        OnPropertyChanged(nameof(StorageUsedDisplay));
        OnPropertyChanged(nameof(StorageFreeDisplay));
    }
    partial void OnIpodConnectedChanged(bool value)
    {
        OnPropertyChanged(nameof(ShowEmptyState));
        OnPropertyChanged(nameof(ShowConnectedContent));
        OnPropertyChanged(nameof(ShowSyncNowButton));
    }
    partial void OnStorageUsedLabelChanged(string value) => OnPropertyChanged(nameof(StorageUsedDisplay));
    partial void OnStorageFreeLabelChanged(string value) => OnPropertyChanged(nameof(StorageFreeDisplay));
    partial void OnProgressCurrentChanged(int value) => OnPropertyChanged(nameof(ProgressLabel));
    partial void OnCurrentTrackLabelChanged(string value) => OnPropertyChanged(nameof(DetailLine));
    partial void OnCurrentLogLineChanged(string value) => OnPropertyChanged(nameof(DetailLine));
    partial void OnProgressTotalChanged(int value)
    {
        OnPropertyChanged(nameof(ProgressLabel));
        OnPropertyChanged(nameof(NoProgressYet));
    }

    public ObservableCollection<HistoryEntryViewModel> Recent { get; } = new();

    public void Update(StatusUpdateEvent s)
    {
        Syncing = s.State == "syncing";
        IpodConnected = s.IpodConnected;
        ApplyStorage(s.Storage);
        if (Syncing)
        {
            StatusText = "Syncing iPod…";
            LastSyncedLabel = "Syncing now";
            return;
        }
        if (!s.IpodConnected)
        {
            StatusText = "iPod not connected";
            LastSyncedLabel = "";
            return;
        }
        // Idle + connected.
        var last = s.LastSync;
        if (last is not null && last.Outcome != "ok")
        {
            StatusText = $"Last sync failed: {last.ErrorMessage ?? "unknown error"}";
            LastSyncedLabel = FormatLastSynced(last.Timestamp);
        }
        else
        {
            StatusText = last is null
                ? "Up to date · iPod connected"
                : $"Up to date · last sync {RelativeTime(last.Timestamp)}";
            LastSyncedLabel = last is null ? "Never synced" : FormatLastSynced(last.Timestamp);
        }
    }

    /// <summary>Set the device label, preferring the iPod's user-set
    /// firmware name (e.g. "Michael's iPod") over the generic model
    /// label ("iPod Classic 7G"). Either can be null/empty; falls back
    /// through name → modelLabel → "iPod".</summary>
    public void SetDeviceLabel(string? name, string? modelLabel)
    {
        if (!string.IsNullOrWhiteSpace(name)) DeviceLabel = name!;
        else if (!string.IsNullOrWhiteSpace(modelLabel)) DeviceLabel = modelLabel!;
        else DeviceLabel = "iPod";
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
            case HeaderEvent h:
                // Sync just started — show the source path so the
                // "Preparing…" phase has signal until the first
                // SummaryEvent gives us a track count.
                CurrentLogLine = $"Scanning {h.Source}";
                break;
            case SummaryEvent s:
                // Subprocess has built the action plan; we can flash
                // the "preparing" → real numbers transition immediately
                // even before the first TrackStart arrives.
                ProgressTotal = s.TotalPlanned;
                ProgressCurrent = 0;
                CurrentTrackLabel = "";
                CurrentLogLine = $"{s.Add} to add, {s.Modify} to update, {s.Remove} to remove";
                break;
            case TrackStartEvent t:
                ProgressCurrent = t.Current;
                ProgressTotal = t.Total;
                CurrentTrackLabel = t.Label;
                CurrentLogLine = "";
                break;
            case TrackDoneEvent:
                // Mid-track UI flicker isn't worth fighting; we wait
                // for the next TrackStart to advance the visible label.
                break;
            case LogEvent l:
                // Forward the daemon's per-step narration so the
                // "Preparing…" phase shows real progress (file walks,
                // fingerprinting, etc.) instead of a blank caption.
                CurrentLogLine = l.Message;
                break;
            case FinishEvent:
                // Daemon's subsequent Idle StatusUpdate will swap the
                // panel back to storage, but reset numbers now so a
                // re-open during the gap shows clean state.
                ProgressCurrent = 0;
                ProgressTotal = 0;
                CurrentTrackLabel = "";
                CurrentLogLine = "";
                break;
        }
    }

    private void ApplyStorage(StorageInfo? info)
    {
        if (info is null || info.TotalBytes == 0)
        {
            HasStorage = false;
            StorageUsedLabel = "";
            StorageFreeLabel = "";
            StorageProgressValue = 0;
            return;
        }
        var used = info.TotalBytes - info.FreeBytes;
        StorageUsedLabel = $"{FormatBytes(used)} used";
        StorageFreeLabel = $"{FormatBytes(info.FreeBytes)} free";
        StorageProgressValue = info.TotalBytes == 0
            ? 0
            : (double)used / info.TotalBytes * 100.0;
        HasStorage = true;
    }

    // Format like "120 GB" / "1.4 TB" — units round to the nearest sensible
    // suffix the way Windows Explorer does for drive sizes (binary base).
    private static string FormatBytes(ulong bytes)
    {
        const double KB = 1024.0;
        const double MB = KB * 1024.0;
        const double GB = MB * 1024.0;
        const double TB = GB * 1024.0;
        if (bytes >= TB) return $"{bytes / TB:0.##} TB";
        if (bytes >= GB) return $"{bytes / GB:0.#} GB";
        if (bytes >= MB) return $"{bytes / MB:0.#} MB";
        if (bytes >= KB) return $"{bytes / KB:0.#} KB";
        return $"{bytes} B";
    }

    private static string FormatLastSynced(string rfc3339)
    {
        if (!DateTimeOffset.TryParse(rfc3339, out var dt)) return "Last synced recently";
        var local = dt.ToLocalTime();
        var now = DateTimeOffset.Now;
        // Same calendar date → "Last synced at 12:30 PM"
        if (local.Date == now.Date) return $"Last synced at {local:h:mm tt}";
        // Yesterday → "Last synced yesterday at 12:30 PM"
        if (local.Date == now.Date.AddDays(-1)) return $"Last synced yesterday at {local:h:mm tt}";
        // Within a week → "Last synced Tuesday at 12:30 PM"
        if ((now - local).TotalDays < 7) return $"Last synced {local:dddd 'at' h:mm tt}";
        // Older → "Last synced 23 May at 12:30 PM"
        return $"Last synced {local:d MMM 'at' h:mm tt}";
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
