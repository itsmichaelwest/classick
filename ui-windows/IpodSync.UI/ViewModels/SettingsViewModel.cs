using System;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync_UI.Ipc;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// Shell ViewModel for SettingsWindow. Holds the live PersistedConfig
/// snapshot the user is editing and exposes per-tab sub-ViewModels.
/// T7–T10 add the sub-VM bodies + bindings.
/// </summary>
public partial class SettingsViewModel : ObservableObject
{
    private readonly DaemonClient _daemon;
    private readonly DaemonEventRouter _router;

    public SettingsViewModel(DaemonClient daemon, DaemonEventRouter router, ConfigUpdateEvent currentConfig)
    {
        _daemon = daemon;
        _router = router;
        General = new SettingsGeneralViewModel(currentConfig);
        Schedule = new SettingsScheduleViewModel(currentConfig);
        History = new SettingsHistoryViewModel(daemon, router);
        About = new SettingsAboutViewModel();
    }

    public SettingsGeneralViewModel General { get; }
    public SettingsScheduleViewModel Schedule { get; }
    public SettingsHistoryViewModel History { get; }
    public SettingsAboutViewModel About { get; }

    /// <summary>
    /// Aggregate dirty fields across tabs into a single SaveConfigCommand.
    /// </summary>
    public async Task SaveAsync()
    {
        var cmd = new SaveConfigCommand(
            Source: General.IsSourceDirty ? General.SourcePath : null,
            Daemon: BuildDaemonSettings(),
            Ipod: null  // Re-identify flow is M5
        );
        try { await _daemon.SendAsync(cmd); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"settings: save failed: {e}"); }
    }

    private DaemonSettings? BuildDaemonSettings()
    {
        if (!General.IsAnyDaemonFieldDirty && !Schedule.IsAnyDirty) return null;
        return new DaemonSettings(
            Enabled: true,
            AutostartWithWindows: Schedule.AutostartWithWindows,
            FirstSyncMode: General.FirstSyncMode,
            SubsequentSyncMode: General.SubsequentSyncMode,
            ScheduleMinutes: (uint)Schedule.ScheduleMinutes,
            NotifyOn: General.NotifyOn);
    }
}

// Sub-VM stubs — filled in by T7–T10. Defined here so SettingsViewModel
// compiles in T6's standalone wave; T7–T10 add the [ObservableProperty]
// fields + Save logic for each tab.

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
        NotifyOn = c.Daemon?.NotifyOn ?? "all";
    }

    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private string ipodModelLabel = "";
    [ObservableProperty] private string ipodSerial = "";
    [ObservableProperty] private string firstSyncMode = "review";
    [ObservableProperty] private string subsequentSyncMode = "auto_apply";
    [ObservableProperty] private string notifyOn = "all";

    public bool IsSourceDirty => SourcePath != _originalSource;
    public bool IsAnyDaemonFieldDirty =>
        FirstSyncMode != (_originalDaemon?.FirstSyncMode ?? "review") ||
        SubsequentSyncMode != (_originalDaemon?.SubsequentSyncMode ?? "auto_apply") ||
        NotifyOn != (_originalDaemon?.NotifyOn ?? "all");
}

public partial class SettingsScheduleViewModel : ObservableObject
{
    private readonly DaemonSettings? _originalDaemon;

    public SettingsScheduleViewModel(ConfigUpdateEvent c)
    {
        _originalDaemon = c.Daemon;
        ScheduleMinutes = (int)(c.Daemon?.ScheduleMinutes ?? 30);
        AutostartWithWindows = c.Daemon?.AutostartWithWindows ?? false;
    }

    [ObservableProperty] private int scheduleMinutes = 30;
    [ObservableProperty] private bool autostartWithWindows;

    public bool IsAnyDirty =>
        ScheduleMinutes != (int)(_originalDaemon?.ScheduleMinutes ?? 30) ||
        AutostartWithWindows != (_originalDaemon?.AutostartWithWindows ?? false);

    public string ScheduleLabel => ScheduleMinutes == 0
        ? "Disabled"
        : ScheduleMinutes < 60
            ? $"Every {ScheduleMinutes} minutes"
            : $"Every {ScheduleMinutes / 60.0:0.#} hours";

    partial void OnScheduleMinutesChanged(int value) => OnPropertyChanged(nameof(ScheduleLabel));
}

public partial class SettingsHistoryViewModel : ObservableObject
{
    private readonly DaemonClient _daemon;

    public SettingsHistoryViewModel(DaemonClient daemon, DaemonEventRouter router)
    {
        _daemon = daemon;
        router.HistoryUpdated += OnHistoryUpdated;
        Entries = new System.Collections.ObjectModel.ObservableCollection<HistoryEntryViewModel>();
        _ = LoadAsync();
    }

    public System.Collections.ObjectModel.ObservableCollection<HistoryEntryViewModel> Entries { get; }

    private async Task LoadAsync()
    {
        try { await _daemon.SendAsync(new GetHistoryCommand(Limit: 50)); }
        catch (Exception e) { System.Diagnostics.Debug.WriteLine($"history: load failed: {e}"); }
    }

    private void OnHistoryUpdated(HistoryUpdateEvent e)
    {
        // Dispatcher marshal happens in callers that need UI thread.
        // The collection's CollectionChanged is fired on whatever
        // thread invokes Add; SettingsHistoryPage marshals before
        // calling into this method by binding-dispatcher contract.
        // For safety we dispatch here.
        App.DispatcherQueue.TryEnqueue(() =>
        {
            Entries.Clear();
            // Reverse so newest is first.
            for (int i = e.Entries.Count - 1; i >= 0; i--)
            {
                Entries.Add(new HistoryEntryViewModel(e.Entries[i]));
            }
        });
    }
}

public partial class SettingsAboutViewModel : ObservableObject
{
    public SettingsAboutViewModel()
    {
        var asm = System.Reflection.Assembly.GetExecutingAssembly();
        UiVersion = asm.GetName().Version?.ToString() ?? "unknown";
    }

    public string UiVersion { get; }
    public string LicenseText => "MIT OR Apache-2.0";
    public string GitHubUrl => "https://github.com/itsmichaelwest/ipod-sync";
}

// HistoryEntryViewModel lives in ViewModels/HistoryEntryViewModel.cs so it can
// be link-compiled into the net10.0 test project without dragging in
// SettingsViewModel's WinUI-app dependencies (e.g. App.DispatcherQueue).
