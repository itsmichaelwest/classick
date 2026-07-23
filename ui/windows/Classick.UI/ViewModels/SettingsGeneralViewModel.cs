using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class SettingsGeneralViewModel : ObservableObject
{
    private readonly DeviceStore _store;
    private string _originalSource;
    private GlobalSettings _originalGlobal;
    private SettingsValue? _loadedDeviceSettings;
    private SelectionValue? _loadedSelection;
    private SubscriptionsValue? _loadedSubscriptions;

    public SettingsGeneralViewModel(GlobalConfigEvent global, DeviceStore store, bool launchOnStartup = false)
    {
        _store = store;
        _originalSource = global.SourceRoot ?? "";
        _originalGlobal = global.Settings;
        SourcePath = _originalSource;
        FirstSyncMode = global.Settings.FirstSyncMode;
        SubsequentSyncMode = global.Settings.SubsequentSyncMode;
        ScheduleMinutes = checked((int)global.Settings.ScheduleMinutes);
        DropSyncBehavior = global.Settings.DropSyncBehavior;
        LaunchOnStartup = launchOnStartup;
    }

    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private SyncMode firstSyncMode;
    [ObservableProperty] private SyncMode subsequentSyncMode;
    [ObservableProperty] private int scheduleMinutes;
    [ObservableProperty] private DropSyncBehavior dropSyncBehavior;
    [ObservableProperty] private DeviceId? selectedDeviceId;
    [ObservableProperty] private string selectedDeviceName = "No iPod selected";
    [ObservableProperty] private string selectedDeviceSummary = "";
    [ObservableProperty] private bool selectedDeviceConnected;
    [ObservableProperty] private bool autoSync;
    [ObservableProperty] private bool rockboxCompat;
    [ObservableProperty] private string deliveryStatus = "Settings unavailable";
    [ObservableProperty] private string selectionSummary = "Selection unavailable";
    [ObservableProperty] private string subscriptionsSummary = "Subscriptions unavailable";
    [ObservableProperty] private SelectionMode deviceSelectionMode = SelectionMode.All;
    [ObservableProperty] private string playlistSubscriptionsText = "";
    [ObservableProperty] private string saveError = "";
    [ObservableProperty] private bool launchOnStartup;
    [ObservableProperty] private string lastSyncStatus = "No sync recorded";

    public bool IsApplyingCanonical { get; private set; }
    public bool IsSourceDirty => SourcePath != _originalSource;
    public bool IsGlobalDirty => FirstSyncMode != _originalGlobal.FirstSyncMode ||
        SubsequentSyncMode != _originalGlobal.SubsequentSyncMode ||
        ScheduleMinutes != checked((int)_originalGlobal.ScheduleMinutes) ||
        DropSyncBehavior != _originalGlobal.DropSyncBehavior;
    public bool IsDeviceSettingsDirty => _loadedDeviceSettings is not null &&
        (AutoSync != _loadedDeviceSettings.AutoSync || RockboxCompat != _loadedDeviceSettings.RockboxCompat);
    public bool HasSelectedDevice => SelectedDeviceId is not null;
    public bool CanEditDeviceSettings => _loadedDeviceSettings is not null;
    public bool CanEditSelection => _loadedSelection is not null;
    public bool CanEditSubscriptions => _loadedSubscriptions is not null;
    public SelectionValue? LoadedSelection => _loadedSelection;
    public bool IsSelectionDirty => _loadedSelection is not null && DeviceSelectionMode != _loadedSelection.Mode;
    public bool IsSubscriptionsDirty => _loadedSubscriptions is not null &&
        PlaylistSubscriptionsText != string.Join(", ", _loadedSubscriptions.Playlists);
    public string SyncModeSummary => SubsequentSyncMode == SyncMode.AutoApply ? "Automatic apply" : "Review before applying";

    public void SelectDevice(DeviceId? deviceId)
    {
        SelectedDeviceId = deviceId;
        RefreshDeviceDraft();
        OnPropertyChanged(nameof(HasSelectedDevice));
    }

    public void AcceptSource(string? sourceRoot)
    {
        IsApplyingCanonical = true;
        _originalSource = sourceRoot ?? "";
        SourcePath = _originalSource;
        IsApplyingCanonical = false;
    }

    public void AcceptGlobal(GlobalSettings settings)
    {
        IsApplyingCanonical = true;
        _originalGlobal = settings;
        FirstSyncMode = settings.FirstSyncMode;
        SubsequentSyncMode = settings.SubsequentSyncMode;
        ScheduleMinutes = checked((int)settings.ScheduleMinutes);
        DropSyncBehavior = settings.DropSyncBehavior;
        IsApplyingCanonical = false;
    }

    public void RefreshDeviceDraft()
    {
        IsApplyingCanonical = true;
        try
        {
            _loadedDeviceSettings = null;
            _loadedSelection = null;
            _loadedSubscriptions = null;
            AutoSync = false;
            RockboxCompat = false;
            DeliveryStatus = "Loading device settings…";
            SelectionSummary = "Loading selection…";
            SubscriptionsSummary = "Loading subscriptions…";
            if (SelectedDeviceId is not { } id || !_store.Devices.TryGetValue(id, out var device))
            {
                SelectedDeviceName = "No iPod selected";
                SelectedDeviceSummary = "";
                SelectedDeviceConnected = false;
                DeliveryStatus = "Settings unavailable";
                SelectionSummary = "Selection unavailable";
                SubscriptionsSummary = "Subscriptions unavailable";
                return;
            }
            var presentation = DevicePresentationFactory.For(device.Inventory);
            SelectedDeviceName = presentation.Title;
            SelectedDeviceSummary = presentation.HardwareSummary;
            SelectedDeviceConnected = device.Inventory.Connected;
            LastSyncStatus = device.History.LastOrDefault() is { } latest
                ? $"Last sync {latest.Timestamp} · {latest.Outcome.ToString().ToLowerInvariant()}"
                : "No sync recorded";
            if (_store.SettingsDrafts.Drafts.TryGetValue(id, out var draft))
            {
                _loadedDeviceSettings = draft.Value;
                AutoSync = draft.Value.AutoSync;
                RockboxCompat = draft.Value.RockboxCompat;
                DeliveryStatus = draft.SaveState switch
                {
                    DeviceSettingsSaveState.WaitingForDevice when draft.Error is { } error =>
                        $"Saved on this PC — waiting for iPod: {error}",
                    DeviceSettingsSaveState.WaitingForDevice => "Saved on this PC — waiting for iPod",
                    DeviceSettingsSaveState.Editing => "Saving on this PC…",
                    DeviceSettingsSaveState.Failed => $"Not saved: {draft.Error}",
                    _ => "Saved",
                };
            }
            if (_store.ComponentDrafts.Selections.TryGetValue(id, out var selection))
            {
                _loadedSelection = selection.Value;
                DeviceSelectionMode = selection.Value.Mode;
                SelectionSummary = selection.Value.Mode == SelectionMode.All
                    ? "All music"
                    : $"{selection.Value.Rules.Count} selection rules";
                SelectionSummary += SaveStateSuffix(selection.SaveState, selection.Error);
            }
            if (_store.ComponentDrafts.Subscriptions.TryGetValue(id, out var subscriptions))
            {
                _loadedSubscriptions = subscriptions.Value;
                PlaylistSubscriptionsText = string.Join(", ", subscriptions.Value.Playlists);
                SubscriptionsSummary = subscriptions.Value.Playlists.Count == 0
                    ? "No playlist subscriptions"
                    : PlaylistSubscriptionsText;
                SubscriptionsSummary += SaveStateSuffix(subscriptions.SaveState, subscriptions.Error);
            }
        }
        finally
        {
            OnPropertyChanged(nameof(CanEditDeviceSettings));
            OnPropertyChanged(nameof(CanEditSelection));
            OnPropertyChanged(nameof(CanEditSubscriptions));
            IsApplyingCanonical = false;
        }
    }

    private static string SaveStateSuffix(DeviceSettingsSaveState state, string? error) => state switch
    {
        DeviceSettingsSaveState.WaitingForDevice when error is not null => $" · Waiting for iPod: {error}",
        DeviceSettingsSaveState.WaitingForDevice => " · Saved on this PC — waiting for iPod",
        DeviceSettingsSaveState.Failed => $" · Not saved: {error}",
        DeviceSettingsSaveState.Editing => " · Saving…",
        _ => " · Saved",
    };

    partial void OnSubsequentSyncModeChanged(SyncMode value) => OnPropertyChanged(nameof(SyncModeSummary));
}
