using System.Collections.ObjectModel;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class SettingsViewModel : ObservableObject, IDisposable
{
    private readonly DaemonClient _daemon;
    private readonly DaemonEventRouter _router;
    private readonly DeviceStore _store;
    private readonly Microsoft.UI.Xaml.DispatcherTimer _debounceTimer;
    private string? _pendingSourceRequest;
    private string? _pendingGlobalRequest;

    public SettingsViewModel(DaemonClient daemon, DaemonEventRouter router, DeviceStore store)
    {
        _daemon = daemon;
        _router = router;
        _store = store;
        var global = store.GlobalConfig ?? new GlobalConfigEvent(
            null, 0, null,
            new GlobalSettings(SyncMode.Review, SyncMode.AutoApply, 30, NotifyLevel.All, DropSyncBehavior.Immediate));
        General = new SettingsGeneralViewModel(global, store, WindowsStartupRegistration.IsEnabled());
        Notifications = new SettingsNotificationsViewModel(global.Settings.NotifyOn);
        History = new SettingsHistoryViewModel(daemon, router);
        About = new SettingsAboutViewModel();
        Chooser = new IpodChooserViewModel(store);
        General.SelectDevice(Chooser.Selected?.DeviceId);

        _debounceTimer = new Microsoft.UI.Xaml.DispatcherTimer { Interval = TimeSpan.FromMilliseconds(400) };
        _debounceTimer.Tick += OnDebounceTick;
        General.PropertyChanged += (_, eventArgs) =>
        {
            if (General.IsApplyingCanonical) return;
            if (eventArgs.PropertyName == nameof(SettingsGeneralViewModel.LaunchOnStartup))
            {
                try { WindowsStartupRegistration.SetEnabled(General.LaunchOnStartup); }
                catch (Exception exception) { General.SaveError = exception.Message; }
                return;
            }
            if (eventArgs.PropertyName is nameof(SettingsGeneralViewModel.AutoSync) or nameof(SettingsGeneralViewModel.RockboxCompat))
            {
                _ = SaveDeviceSettingsAsync();
            }
            else if (eventArgs.PropertyName == nameof(SettingsGeneralViewModel.DeviceSelectionMode))
            {
                _ = SaveSelectionAsync();
            }
            else
            {
                QueueSave();
            }
        };
        Notifications.PropertyChanged += (_, _) => QueueSave();
        Chooser.SelectionChanged += deviceId =>
        {
            General.SelectDevice(deviceId);
            if (deviceId is { } id) _ = RequestDeviceConfigAsync(id);
        };
        router.EventReceived += OnWireEvent;
        if (Chooser.Selected is { } selected) _ = RequestDeviceConfigAsync(selected.DeviceId);
    }

    public SettingsGeneralViewModel General { get; }
    public SettingsNotificationsViewModel Notifications { get; }
    public SettingsHistoryViewModel History { get; }
    public SettingsAboutViewModel About { get; }
    public IpodChooserViewModel Chooser { get; }

    public void QueueSave()
    {
        if (General.IsApplyingCanonical) return;
        _debounceTimer.Stop();
        _debounceTimer.Start();
    }

    private async void OnDebounceTick(object? sender, object e)
    {
        _debounceTimer.Stop();
        await SaveAsync();
    }

    public async Task SaveAsync()
    {
        try
        {
            if (General.IsSourceDirty)
            {
                _pendingSourceRequest = NewId();
                await _daemon.SendAsync(new SetSourceLocationCommand(_pendingSourceRequest, General.SourcePath));
            }
            if (General.IsGlobalDirty || Notifications.IsDirty)
            {
                _pendingGlobalRequest = NewId();
                await _daemon.SendAsync(new SetGlobalSettingsCommand(_pendingGlobalRequest, new GlobalSettings(
                    General.FirstSyncMode,
                    General.SubsequentSyncMode,
                    checked((uint)General.ScheduleMinutes),
                    Notifications.NotifyOn,
                    General.DropSyncBehavior)));
            }
            if (General.IsSubscriptionsDirty && General.SelectedDeviceId is { } deviceId)
            {
                var playlists = General.PlaylistSubscriptionsText.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                    .Distinct(StringComparer.Ordinal)
                    .ToArray();
                var command = _store.ComponentDrafts.EditSubscriptions(
                    deviceId,
                    new SubscriptionsValue(1, playlists),
                    NewId(),
                    NewId());
                General.RefreshDeviceDraft();
                try { await _daemon.SendAsync(command); }
                catch (Exception exception)
                {
                    _store.ComponentDrafts.ApplyFailure(new ConfigMutationFailedEvent(
                        deviceId,
                        command.RequestId,
                        command.MutationId,
                        ConfigComponent.Subscriptions,
                        ConfigFailureStage.HostAcceptance,
                        exception.Message));
                    General.RefreshDeviceDraft();
                    throw;
                }
            }
        }
        catch (Exception exception)
        {
            General.SaveError = exception.Message;
        }
    }

    private async Task SaveDeviceSettingsAsync()
    {
        if (!General.IsDeviceSettingsDirty || General.SelectedDeviceId is not { } deviceId) return;
        var command = _store.SettingsDrafts.Edit(
            deviceId,
            new SettingsValue(1, General.AutoSync, General.RockboxCompat),
            NewId(),
            NewId());
        General.RefreshDeviceDraft();
        try
        {
            await _daemon.SendAsync(command);
        }
        catch (Exception exception)
        {
            _store.SettingsDrafts.MarkTransportFailure(deviceId, exception.Message);
            General.RefreshDeviceDraft();
        }
    }

    private async Task SaveSelectionAsync()
    {
        if (!General.IsSelectionDirty || General.SelectedDeviceId is not { } deviceId ||
            General.LoadedSelection is not { } loaded) return;
        var value = new SelectionValue(
            1,
            General.DeviceSelectionMode,
            loaded.Rules);
        var command = _store.ComponentDrafts.EditSelection(deviceId, value, NewId(), NewId());
        General.RefreshDeviceDraft();
        try { await _daemon.SendAsync(command); }
        catch (Exception exception)
        {
            _store.ComponentDrafts.ApplyFailure(new ConfigMutationFailedEvent(
                deviceId,
                command.RequestId,
                command.MutationId,
                ConfigComponent.Selection,
                ConfigFailureStage.HostAcceptance,
                exception.Message));
            General.RefreshDeviceDraft();
            General.SaveError = exception.Message;
        }
    }

    public async Task ForgetSelectedAsync()
    {
        if (Chooser.Selected is not { } selected) return;
        var requestId = NewId();
        var wasLastDevice = _store.Devices.Count == 1;
        var completion = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        void OnEvent(WireEvent wireEvent)
        {
            if (wireEvent is DeviceForgottenEvent forgotten && forgotten.RequestId == requestId)
                completion.TrySetResult();
            else if (wireEvent is CommandFailedEvent failed && failed.RequestId == requestId)
                completion.TrySetException(new InvalidOperationException(failed.Message));
        }
        _router.EventReceived += OnEvent;
        try
        {
            await _daemon.SendAsync(new ForgetDeviceCommand(selected.DeviceId, requestId));
            await completion.Task.WaitAsync(TimeSpan.FromSeconds(10));
            if (wasLastDevice) App.RequestPairNewIpod();
        }
        catch (Exception exception)
        {
            General.SaveError = exception is TimeoutException
                ? "Classick did not confirm that the iPod was removed. Try again."
                : exception.Message;
            throw;
        }
        finally
        {
            _router.EventReceived -= OnEvent;
        }
    }

    public Task SyncSelectedAsync() => Chooser.Selected is { } selected
        ? _daemon.SendAsync(new WireTriggerSyncCommand(selected.DeviceId, NewId(), SyncTrigger.Manual))
        : Task.CompletedTask;

    public Task ReplaceSelectedLibraryAsync() => Chooser.Selected is { } selected
        ? _daemon.SendAsync(new ReplaceLibraryCommand(selected.DeviceId, NewId()))
        : Task.CompletedTask;

    private void OnWireEvent(WireEvent wireEvent)
    {
        if (wireEvent is GlobalConfigEvent global)
        {
            App.DispatcherQueue.TryEnqueue(() =>
            {
                if (global.RequestId == _pendingSourceRequest)
                {
                    General.AcceptSource(global.SourceRoot);
                    _pendingSourceRequest = null;
                }
                if (global.RequestId == _pendingGlobalRequest)
                {
                    General.AcceptGlobal(global.Settings);
                    Notifications.AcceptGlobal(global.Settings.NotifyOn);
                    _pendingGlobalRequest = null;
                }
            });
        }
        else if (wireEvent is CommandFailedEvent failed &&
                 (failed.RequestId == _pendingSourceRequest || failed.RequestId == _pendingGlobalRequest))
        {
            App.DispatcherQueue.TryEnqueue(() => General.SaveError = failed.Message);
            if (failed.RequestId == _pendingSourceRequest) _pendingSourceRequest = null;
            if (failed.RequestId == _pendingGlobalRequest) _pendingGlobalRequest = null;
        }
        else if (wireEvent is DeviceConfigEvent or ConfigMutationFailedEvent or HistoryEvent)
        {
            App.DispatcherQueue.TryEnqueue(General.RefreshDeviceDraft);
        }
        else if (wireEvent is DeviceInventoryEvent or DeviceForgottenEvent)
        {
            App.DispatcherQueue.TryEnqueue(() =>
            {
                Chooser.Refresh();
                General.SelectDevice(Chooser.Selected?.DeviceId);
            });
        }
    }

    private async Task RequestDeviceConfigAsync(DeviceId deviceId)
    {
        try { await _daemon.SendAsync(new GetDeviceConfigCommand(deviceId, NewId())); }
        catch (Exception exception) { General.SaveError = exception.Message; }
    }

    private static string NewId() => Guid.NewGuid().ToString("D");

    public void Dispose()
    {
        _debounceTimer.Stop();
        _router.EventReceived -= OnWireEvent;
        History.Dispose();
    }
}

public partial class SettingsNotificationsViewModel : ObservableObject
{
    private NotifyLevel _original;

    public SettingsNotificationsViewModel(NotifyLevel notifyOn)
    {
        _original = notifyOn;
        NotifyOnSyncComplete = notifyOn == NotifyLevel.All;
        NotifyOnSyncFailed = notifyOn != NotifyLevel.None;
        NotifyOnDeviceConnected = notifyOn == NotifyLevel.All;
    }

    [ObservableProperty] private bool notifyOnSyncComplete;
    [ObservableProperty] private bool notifyOnSyncFailed;
    [ObservableProperty] private bool notifyOnDeviceConnected;
    public NotifyLevel NotifyOn => NotifyOnSyncComplete ? NotifyLevel.All :
        NotifyOnSyncFailed ? NotifyLevel.ErrorsOnly : NotifyLevel.None;
    public bool IsDirty => NotifyOn != _original;
    public void AcceptGlobal(NotifyLevel notifyOn) => _original = notifyOn;
    partial void OnNotifyOnSyncCompleteChanged(bool value) => OnPropertyChanged(nameof(NotifyOn));
    partial void OnNotifyOnSyncFailedChanged(bool value) => OnPropertyChanged(nameof(NotifyOn));
}
