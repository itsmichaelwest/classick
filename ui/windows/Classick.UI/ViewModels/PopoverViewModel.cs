using System;
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using Classick_UI.Ipc;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel : ObservableObject
{
    [ObservableProperty] private string statusText = "iPod not connected";
    [ObservableProperty] private string deviceLabel = "iPod";
    [ObservableProperty] private string lastSyncedLabel = "";
    [ObservableProperty] private bool syncing;
    [ObservableProperty] private bool ipodConnected;
    [ObservableProperty] private int progressCurrent;
    [ObservableProperty] private int progressTotal;
    [ObservableProperty] private bool finishingSync;
    [ObservableProperty] private bool paused;
    /// <summary>Raw <see cref="WireTrackStartEvent.Label"/> for the currently
    /// processing track (e.g. "ADD /Music/Artist/Album/01 Track.flac").
    /// Not rendered as the primary caption — that's a counter — but
    /// exposed as a hover tooltip on the caption so anyone curious can
    /// see exactly which file is being processed.</summary>
    [ObservableProperty] private string currentTrackLabel = "";
    /// <summary>Wall-clock time the apply loop started (first
    /// <see cref="SyncSummaryEvent"/>). Used to compute <see cref="EtaLabel"/>
    /// from <see cref="ProgressCurrent"/>/<see cref="ProgressTotal"/>.
    /// Null outside of an active sync.
    ///
    /// <para>Settable so the App can seed it when a popover is opened
    /// mid-sync — otherwise the ETA would restart from the popover-open
    /// timestamp and drift slow until the wall-clock catches up.</para>
    /// </summary>
    public DateTimeOffset? SyncStartedAt
    {
        get => _syncStartedAt;
        set
        {
            if (_syncStartedAt == value) return;
            _syncStartedAt = value;
            OnPropertyChanged(nameof(EtaLabel));
        }
    }
    private DateTimeOffset? _syncStartedAt;

    // Prompt overlay state. When the daemon reports a typed prompt
    // from the sync subprocess (source-change safeguard, retry/skip/
    // abort prompts, etc.), the popover renders an overlay panel
    // with the message and a button per option. The user's click
    // sends a DecidePromptCommand back to the daemon, which forwards
    // it to the subprocess stdin so the sync proceeds. Cleared on
    // Finish / TrackStart / explicit answer.
    [ObservableProperty] private bool promptActive;
    [ObservableProperty] private ulong promptId;
    [ObservableProperty] private string promptMessage = "";
    public ObservableCollection<string> PromptOptions { get; } = new();

    partial void OnPromptActiveChanged(bool value)
    {
        // When the prompt overlay toggles, every layout-region
        // visibility flag flips with it. Fire the dependent-property
        // notifications so the popover XAML can simply hide the
        // underlying content (vs. relying on the overlay to opaquely
        // paint over it, which doesn't work cleanly over the acrylic
        // backdrop).
        OnPropertyChanged(nameof(ShowConnectedContent));
        OnPropertyChanged(nameof(ShowEjectButton));
        OnPropertyChanged(nameof(ShowEmptyState));
        OnPropertyChanged(nameof(ShowFooter));
        OnPropertyChanged(nameof(ShowSyncNowButton));
        OnPropertyChanged(nameof(ShowSyncControls));
        OnPropertyChanged(nameof(ShowSourceRecovery));
    }

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
    /// connection state. Suppressed when a prompt overlay is active
    /// so the underlying layout doesn't bleed through.</summary>
    public bool ShowEmptyState => !IpodConnected && !HasMultipleDeviceChoices &&
        !PromptActive && !ShowSourceRecovery;
    public string EmptyStateTitle => FinishingSync ? "Finishing sync…" : "No iPod connected";
    public string EmptyStateSubtitle => FinishingSync
        ? "iPod disconnected. Finishing safely…"
        : "Please connect your iPod to begin";
    public string DisconnectedFooterText => FinishingSync
        ? "Finishing safely…"
        : "Looking for iPod on USB…";

    /// <summary>The normal connected layout (device row + storage /
    /// sync progress + full footer). Suppressed when a prompt overlay
    /// is active.</summary>
    public bool ShowConnectedContent => (IpodConnected || HasMultipleDeviceChoices) &&
        !PromptActive && !ShowSourceRecovery;
    public bool ShowEjectButton => IpodConnected && ShowConnectedContent;

    /// <summary>True when the popover should show the footer row
    /// (Sync now / Stop sync / Eject / Settings). Hidden during a
    /// pending prompt because the prompt's own option buttons are
    /// the only meaningful actions then.</summary>
    public bool ShowFooter => !PromptActive;

    /// <summary>True when the footer should show the Sync Now button —
    /// connected AND idle AND no prompt in flight.</summary>
    public bool ShowSyncNowButton => IpodConnected && !Syncing && !PromptActive &&
        !FinishingSync && !ShowSourceRecovery && DeviceReadyForSync;
    public bool ShowDeviceGuidance => !string.IsNullOrWhiteSpace(DeviceGuidance);
    public bool CanControlActiveDeviceSync => ActiveDeviceSession is not null &&
        IpodConnected && Syncing && !FinishingSync;
    public bool ShowSyncControls => CanControlActiveDeviceSync &&
        !PromptActive && !ShowSourceRecovery;
    public string SyncActionLabel => Paused ? "Resume sync" : "Sync now";

    /// <summary>True between sync start and the first SyncSummaryEvent /
    /// TrackStart, so the popover's ProgressBar can render as
    /// indeterminate (marquee) until we know the total count.</summary>
    public bool NoProgressYet => Syncing && ProgressTotal <= 0;

    /// <summary>Left-side caption beneath the sync progress bar. One
    /// short line: "Preparing sync…" until the action plan is built,
    /// then "Syncing N of M tracks". The per-track filename is exposed
    /// as a tooltip via <see cref="CurrentTrackLabel"/> for anyone who
    /// wants to see exactly which file is in flight.</summary>
    public string ProgressCaption
    {
        get
        {
            if (!Syncing) return "";
            if (ProgressTotal <= 0) return "Preparing sync…";
            return $"Syncing {ProgressCurrent} of {ProgressTotal} tracks";
        }
    }

    /// <summary>Right-side ETA caption (e.g. "about 3 min left").
    /// Suppressed during the prep phase (no track count yet) and during
    /// the first couple of tracks (the per-track average is too noisy
    /// to be useful before we have a few samples). Empty otherwise so
    /// the popover doesn't flash an obviously-wrong "5 hr left" on the
    /// first track of a fast sync.</summary>
    public string EtaLabel
    {
        get
        {
            if (ReportedEtaSeconds is { } reported) return FormatEta(reported);
            if (!Syncing || ProgressTotal <= 0 || SyncStartedAt is null) return "";
            // Use completed-track count (ProgressCurrent is 1-indexed
            // and names the currently-starting track). Wait for ≥3
            // completed before estimating so an outlier first track
            // doesn't dominate the average.
            int completed = Math.Max(0, ProgressCurrent - 1);
            if (completed < 3) return "";
            var elapsed = DateTimeOffset.Now - SyncStartedAt.Value;
            if (elapsed.TotalSeconds <= 0) return "";
            double perTrack = elapsed.TotalSeconds / completed;
            double remainingSec = perTrack * (ProgressTotal - completed);
            return FormatEta(remainingSec);
        }
    }

    private static string FormatEta(double remainingSec)
    {
        if (remainingSec < 45) return "less than a minute";
        if (remainingSec < 90) return "about a minute left";
        if (remainingSec < 3600)
        {
            int minutes = (int)Math.Round(remainingSec / 60.0);
            return $"about {minutes} min left";
        }
        double hours = remainingSec / 3600.0;
        if (hours < 1.5) return "about 1 hr left";
        return $"about {(int)Math.Round(hours)} hr left";
    }

    /// <summary>Storage labels for display: real values when HasStorage,
    /// em-dash placeholder otherwise. The popover always renders the
    /// storage row so its footprint stays stable while data is loading.</summary>
    public string StorageUsedDisplay => HasStorage ? StorageUsedLabel : "— used";
    public string StorageFreeDisplay => HasStorage ? StorageFreeLabel : "— free";

    partial void OnSyncingChanged(bool value)
    {
        OnPropertyChanged(nameof(NotSyncing));
        OnPropertyChanged(nameof(ShowStorage));
        OnPropertyChanged(nameof(ProgressCaption));
        OnPropertyChanged(nameof(EtaLabel));
        OnPropertyChanged(nameof(NoProgressYet));
        OnPropertyChanged(nameof(ShowSyncNowButton));
        OnPropertyChanged(nameof(ShowSyncControls));
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
        OnPropertyChanged(nameof(ShowEjectButton));
        OnPropertyChanged(nameof(ShowSyncNowButton));
        OnPropertyChanged(nameof(ShowSyncControls));
        OnPropertyChanged(nameof(ShowFooter));
    }
    partial void OnFinishingSyncChanged(bool value)
    {
        OnPropertyChanged(nameof(EmptyStateTitle));
        OnPropertyChanged(nameof(EmptyStateSubtitle));
        OnPropertyChanged(nameof(DisconnectedFooterText));
        OnPropertyChanged(nameof(ShowSyncNowButton));
        OnPropertyChanged(nameof(ShowSyncControls));
    }
    partial void OnPausedChanged(bool value) => OnPropertyChanged(nameof(SyncActionLabel));
    partial void OnStorageUsedLabelChanged(string value) => OnPropertyChanged(nameof(StorageUsedDisplay));
    partial void OnStorageFreeLabelChanged(string value) => OnPropertyChanged(nameof(StorageFreeDisplay));
    partial void OnProgressCurrentChanged(int value)
    {
        OnPropertyChanged(nameof(ProgressCaption));
        OnPropertyChanged(nameof(EtaLabel));
    }
    partial void OnProgressTotalChanged(int value)
    {
        OnPropertyChanged(nameof(ProgressCaption));
        OnPropertyChanged(nameof(EtaLabel));
        OnPropertyChanged(nameof(NoProgressYet));
    }

    public ObservableCollection<HistoryEntryViewModel> Recent { get; } = new();

    public void ApplyHistory(IReadOnlyList<WireHistoryEntry> entries)
    {
        Recent.Clear();
        foreach (var entry in entries.TakeLast(5).Reverse())
            Recent.Add(new HistoryEntryViewModel(entry));
        if (!Syncing && IpodConnected)
        {
            LastSyncedLabel = entries.LastOrDefault() is { } latest
                ? latest.Outcome == SyncOutcome.Ok ? "Last sync completed" : "Last sync failed"
                : "Never synced";
        }
    }

    private void ApplyStorage(StorageSnapshot? info)
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

}

// HistoryEntryViewModel canonicalized in SettingsViewModel.cs (T9).
