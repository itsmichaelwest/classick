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
    public SettingsScheduleViewModel(ConfigUpdateEvent c) { /* T8 */ }
    public int ScheduleMinutes { get; set; } = 30;
    public bool AutostartWithWindows { get; set; }
    public bool IsAnyDirty => false;  // T8
}

public partial class SettingsHistoryViewModel : ObservableObject
{
    public SettingsHistoryViewModel(DaemonClient d, DaemonEventRouter r) { /* T9 */ }
}

public partial class SettingsAboutViewModel : ObservableObject
{
    public SettingsAboutViewModel() { /* T10 */ }
}
