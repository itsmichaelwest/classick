using System;
using System.Collections.ObjectModel;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using Classick_UI.Ipc;

namespace Classick_UI.ViewModels;

/// <summary>
/// Shell ViewModel for SettingsWindow. Holds the live PersistedConfig
/// snapshot the user is editing and exposes per-tab sub-ViewModels.
///
/// Save model — the Settings window has no Save/Cancel buttons; each
/// sub-VM raises PropertyChanged on edits, this VM debounces writes
/// (DebounceMs) and pushes a SaveConfigCommand to the daemon. Keeps
/// the UX edit-and-forget while collapsing dropdown spam into a
/// single round-trip.
/// </summary>
public partial class SettingsViewModel : ObservableObject
{
    private const int DebounceMs = 400;

    private readonly DaemonClient _daemon;
    private readonly DaemonEventRouter _router;
    // DispatcherTimer instead of a Task.Delay+CTS pattern: resetting
    // a timer is exception-free, whereas cancelling Task.Delay throws
    // TaskCanceledException on every edit (first-chance exceptions
    // are caught but still logged by the debugger and are slow in
    // Debug builds).
    private readonly Microsoft.UI.Xaml.DispatcherTimer _debounceTimer;

    public SettingsViewModel(DaemonClient daemon, DaemonEventRouter router, ConfigUpdateEvent currentConfig)
    {
        _daemon = daemon;
        _router = router;
        General = new SettingsGeneralViewModel(currentConfig);
        Notifications = new SettingsNotificationsViewModel(currentConfig);
        History = new SettingsHistoryViewModel(daemon, router);
        About = new SettingsAboutViewModel();
        Chooser = new IpodChooserViewModel(currentConfig, daemon);

        _debounceTimer = new Microsoft.UI.Xaml.DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(DebounceMs),
        };
        _debounceTimer.Tick += OnDebounceTick;

        // Any change in a sub-VM kicks the debounce timer.
        General.PropertyChanged += (_, _) => QueueSave();
        Notifications.PropertyChanged += (_, _) => QueueSave();
        Chooser.Changed += QueueSave;
    }

    public SettingsGeneralViewModel General { get; }
    public SettingsNotificationsViewModel Notifications { get; }
    public SettingsHistoryViewModel History { get; }
    public SettingsAboutViewModel About { get; }
    public IpodChooserViewModel Chooser { get; }

    /// <summary>Schedule a debounced save. Each call resets the timer
    /// so the user's last edit within DebounceMs wins; the timer
    /// fires once on the UI thread, no continuations, no
    /// TaskCanceledException churn.</summary>
    public void QueueSave()
    {
        _debounceTimer.Stop();
        _debounceTimer.Start();
    }

    private async void OnDebounceTick(object? sender, object e)
    {
        _debounceTimer.Stop();
        await SaveAsync();
    }

    /// <summary>Push current dirty fields to the daemon as a single
    /// SaveConfigCommand. Public so tests can drive it directly.</summary>
    public async Task SaveAsync()
    {
        var cmd = new SaveConfigCommand(
            Source: General.IsSourceDirty ? General.SourcePath : null,
            Daemon: BuildDaemonSettings(),
            Ipod: null);
        try { await _daemon.SendAsync(cmd); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"settings: save failed: {e}"); }
    }

    private DaemonSettings? BuildDaemonSettings()
    {
        if (!General.IsAnyDaemonFieldDirty && !Notifications.IsDirty) return null;
        // TODO(windows-autosync): `Enabled: true` is hardcoded, but the daemon
        // now gates auto-sync on `daemon.enabled` (see
        // crates/classick/src/daemon/runtime.rs::auto_sync_enabled), not on
        // SubsequentSyncMode. So the "Manual" sync mode no longer disables
        // auto-sync on Windows. Expose an explicit auto-sync on/off control and
        // map it to Enabled here (leaving SubsequentSyncMode for apply-vs-review
        // only). Not done this session — no Windows build environment.
        return new DaemonSettings(
            Enabled: true,
            AutostartWithWindows: General.LaunchOnStartup,
            FirstSyncMode: General.FirstSyncMode,
            SubsequentSyncMode: General.SubsequentSyncMode,
            ScheduleMinutes: (uint)General.ScheduleMinutes,
            NotifyOn: Notifications.NotifyOn);
    }
}

// ---------------------------------------------------------------------
// General — Music folder, Sync mode, Sync frequency, Launch on startup,
// Remove iPod, About footer. Schedule fields used to live on a separate
// page; merged here per the Figma which folds them into General.
// ---------------------------------------------------------------------

public partial class SettingsGeneralViewModel : ObservableObject
{
    private readonly string _originalSource;
    private readonly DaemonSettings? _originalDaemon;

    public SettingsGeneralViewModel(ConfigUpdateEvent c)
    {
        _originalSource = c.Source ?? "";
        _originalDaemon = c.Daemon;
        SourcePath = _originalSource;
        IpodModelLabel = c.Ipod?.ModelLabel ?? "(not configured)";
        IpodSerial = c.Ipod?.Serial ?? "";
        FirstSyncMode = c.Daemon?.FirstSyncMode ?? "review";
        SubsequentSyncMode = c.Daemon?.SubsequentSyncMode ?? "auto_apply";
        ScheduleMinutes = (int)(c.Daemon?.ScheduleMinutes ?? 30);
        LaunchOnStartup = c.Daemon?.AutostartWithWindows ?? false;
    }

    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private string ipodModelLabel = "";
    [ObservableProperty] private string ipodSerial = "";
    [ObservableProperty] private string firstSyncMode = "review";
    [ObservableProperty] private string subsequentSyncMode = "auto_apply";
    [ObservableProperty] private int scheduleMinutes = 30;
    [ObservableProperty] private bool launchOnStartup;

    public bool IsSourceDirty => SourcePath != _originalSource;

    public bool IsAnyDaemonFieldDirty =>
        FirstSyncMode != (_originalDaemon?.FirstSyncMode ?? "review") ||
        SubsequentSyncMode != (_originalDaemon?.SubsequentSyncMode ?? "auto_apply") ||
        ScheduleMinutes != (int)(_originalDaemon?.ScheduleMinutes ?? 30) ||
        LaunchOnStartup != (_originalDaemon?.AutostartWithWindows ?? false);

    public string ScheduleLabel => ScheduleMinutes == 0
        ? "Only on plug-in"
        : ScheduleMinutes < 60
            ? $"Every {ScheduleMinutes} minutes"
            : $"Every {ScheduleMinutes / 60.0:0.#} hours";

    /// <summary>Human-readable sync-mode label for the SettingsExpander header.</summary>
    public string SyncModeSummary => SubsequentSyncMode switch
    {
        "auto_apply" => "Automatic",
        "review"     => "Manual",
        _            => SubsequentSyncMode,
    };

    partial void OnScheduleMinutesChanged(int value) => OnPropertyChanged(nameof(ScheduleLabel));
    partial void OnSubsequentSyncModeChanged(string value) => OnPropertyChanged(nameof(SyncModeSummary));
}

// ---------------------------------------------------------------------
// Notifications — currently a single NotifyOn enum on the daemon. The
// Figma sketches a per-event toggle model; we surface the same control
// surface but coalesce into NotifyOn for the wire until the daemon
// grows per-event toggles.
// ---------------------------------------------------------------------

public partial class SettingsNotificationsViewModel : ObservableObject
{
    private readonly string _originalNotifyOn;

    public SettingsNotificationsViewModel(ConfigUpdateEvent c)
    {
        _originalNotifyOn = c.Daemon?.NotifyOn ?? "all";
        ApplyFromNotifyOn(_originalNotifyOn);
    }

    [ObservableProperty] private bool notifyOnSyncComplete = true;
    [ObservableProperty] private bool notifyOnSyncFailed = true;
    [ObservableProperty] private bool notifyOnDeviceConnected = true;

    public bool IsDirty => NotifyOn != _originalNotifyOn;

    /// <summary>Coalesce the three per-event toggles into the single
    /// NotifyOn wire enum: all → any success+failure on, errors_only →
    /// only failures on, none → all off.</summary>
    public string NotifyOn => (NotifyOnSyncComplete, NotifyOnSyncFailed) switch
    {
        (true,  _)     => "all",
        (false, true)  => "errors_only",
        _              => "none",
    };

    private void ApplyFromNotifyOn(string val)
    {
        switch (val)
        {
            case "all":
                NotifyOnSyncComplete = true;
                NotifyOnSyncFailed = true;
                NotifyOnDeviceConnected = true;
                break;
            case "errors_only":
                NotifyOnSyncComplete = false;
                NotifyOnSyncFailed = true;
                NotifyOnDeviceConnected = false;
                break;
            default:
                NotifyOnSyncComplete = false;
                NotifyOnSyncFailed = false;
                NotifyOnDeviceConnected = false;
                break;
        }
    }

    partial void OnNotifyOnSyncCompleteChanged(bool value) => OnPropertyChanged(nameof(NotifyOn));
    partial void OnNotifyOnSyncFailedChanged(bool value) => OnPropertyChanged(nameof(NotifyOn));
}

// ---------------------------------------------------------------------
// History
// ---------------------------------------------------------------------

public partial class SettingsHistoryViewModel : ObservableObject
{
    private readonly DaemonClient _daemon;

    public SettingsHistoryViewModel(DaemonClient daemon, DaemonEventRouter router)
    {
        _daemon = daemon;
        router.HistoryUpdated += OnHistoryUpdated;
        Entries = new ObservableCollection<HistoryEntryViewModel>();
        _ = LoadAsync();
    }

    public ObservableCollection<HistoryEntryViewModel> Entries { get; }

    private async Task LoadAsync()
    {
        try { await _daemon.SendAsync(new GetHistoryCommand(Limit: 50)); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"history: load failed: {e}"); }
    }

    private void OnHistoryUpdated(HistoryUpdateEvent e)
    {
        App.DispatcherQueue.TryEnqueue(() =>
        {
            Entries.Clear();
            for (int i = e.Entries.Count - 1; i >= 0; i--)
            {
                Entries.Add(new HistoryEntryViewModel(e.Entries[i]));
            }
        });
    }
}

// ---------------------------------------------------------------------
// About — sits as a footer card on the General page now.
// ---------------------------------------------------------------------

public partial class SettingsAboutViewModel : ObservableObject
{
    public SettingsAboutViewModel()
    {
        var asm = System.Reflection.Assembly.GetExecutingAssembly();
        UiVersion = asm.GetName().Version?.ToString() ?? "unknown";
    }

    public string AppName => "classick";
    public string UiVersion { get; }
    public string VersionLabel => $"Version {UiVersion}";
    public string LicenseText => "MIT OR Apache-2.0";
    public string GitHubUrl => "https://github.com/itsmichaelwest/classick";
}

// ---------------------------------------------------------------------
// Multi-iPod chooser. UI shell only — the daemon currently knows one
// iPod identity, so the collection contains a single live entry today.
// Rename writes the friendly name back through SaveConfig; Remove
// requests the wizard be re-run.
// ---------------------------------------------------------------------

public partial class IpodChooserViewModel : ObservableObject
{
    private readonly DaemonClient? _daemon;
    public event Action? Changed;

    public IpodChooserViewModel(ConfigUpdateEvent c, DaemonClient? daemon = null)
    {
        _daemon = daemon;
        Items = new ObservableCollection<IpodChooserItemViewModel>();
        if (c.Ipod is { } id)
        {
            var item = new IpodChooserItemViewModel(id.Serial, id.Name, id.ModelLabel);
            Items.Add(item);
            Selected = item;
        }
    }

    public ObservableCollection<IpodChooserItemViewModel> Items { get; }

    [ObservableProperty] private IpodChooserItemViewModel? selected;

    public string SelectedDisplayName => Selected?.DisplayName ?? "No iPod paired";

    partial void OnSelectedChanged(IpodChooserItemViewModel? value)
    {
        OnPropertyChanged(nameof(SelectedDisplayName));
        Changed?.Invoke();
    }

    public void Select(IpodChooserItemViewModel item) => Selected = item;

    public void Rename(IpodChooserItemViewModel item, string newName)
    {
        item.Rename(newName);
        if (ReferenceEquals(item, Selected)) OnPropertyChanged(nameof(SelectedDisplayName));
        Changed?.Invoke();
    }

    public void Remove(IpodChooserItemViewModel item)
    {
        Items.Remove(item);
        if (ReferenceEquals(item, Selected)) Selected = Items.Count > 0 ? Items[0] : null;
        Changed?.Invoke();
        // Persist the removal: ForgetIpod clears the daemon's
        // ipod_identity from disk. Without this the iPod would
        // reappear on the next app launch because ConfigUpdate
        // would still include the old identity.
        if (_daemon is not null)
        {
            _ = SendForgetAsync(_daemon);
        }
        // Kick the wizard so the user can pair a new iPod.
        App.RequestPairNewIpod();
    }

    private static async Task SendForgetAsync(DaemonClient daemon)
    {
        try { await daemon.SendAsync(new ForgetIpodCommand()); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"chooser: forget_ipod failed: {e}"); }
    }
}

public partial class IpodChooserItemViewModel : ObservableObject
{
    public IpodChooserItemViewModel(string serial, string? friendlyName, string modelLabel)
    {
        Serial = serial;
        FriendlyName = friendlyName ?? "";
        ModelLabel = modelLabel;
    }

    public string Serial { get; }
    [ObservableProperty] private string friendlyName = "";
    public string ModelLabel { get; }

    public string DisplayName => !string.IsNullOrWhiteSpace(FriendlyName) ? FriendlyName : ModelLabel;

    public void Rename(string newName)
    {
        FriendlyName = newName;
        OnPropertyChanged(nameof(DisplayName));
    }
}
