using System.Collections.ObjectModel;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class IpodChooserViewModel : ObservableObject
{
    private readonly DeviceStore _store;

    public IpodChooserViewModel(DeviceStore store)
    {
        _store = store;
        Refresh();
    }

    public ObservableCollection<IpodChooserItemViewModel> Items { get; } = new();
    [ObservableProperty] private IpodChooserItemViewModel? selected;
    public string SelectedDisplayName => Selected?.DisplayName ?? "No iPod paired";
    public event Action<DeviceId?>? SelectionChanged;
    public event Action? ItemsChanged;
    partial void OnSelectedChanged(IpodChooserItemViewModel? value)
    {
        OnPropertyChanged(nameof(SelectedDisplayName));
        SelectionChanged?.Invoke(value?.DeviceId);
    }
    public void Select(IpodChooserItemViewModel item) => Selected = item;

    public void Refresh()
    {
        var selectedId = Selected?.DeviceId;
        Items.Clear();
        foreach (var device in _store.Devices.Values.OrderBy(
            device => DevicePresentationFactory.For(device.Inventory).Title,
            StringComparer.CurrentCultureIgnoreCase))
        {
            Items.Add(new IpodChooserItemViewModel(
                device.Inventory.DeviceId,
                DevicePresentationFactory.For(device.Inventory).Title,
                device.Inventory.Connected));
        }
        Selected = Items.FirstOrDefault(item => item.DeviceId == selectedId) ?? Items.FirstOrDefault();
        ItemsChanged?.Invoke();
    }
}

public sealed record IpodChooserItemViewModel(DeviceId DeviceId, string DisplayName, bool Connected);

public partial class SettingsHistoryViewModel : ObservableObject, IDisposable
{
    private readonly DaemonClient _daemon;
    private readonly DaemonEventRouter _router;
    public SettingsHistoryViewModel(DaemonClient daemon, DaemonEventRouter router)
    {
        _daemon = daemon;
        _router = router;
        router.EventReceived += OnEvent;
        _ = LoadAsync();
    }
    public ObservableCollection<HistoryEntryViewModel> Entries { get; } = new();
    private async Task LoadAsync()
    {
        try { await _daemon.SendAsync(new WireGetHistoryCommand(Guid.NewGuid().ToString("D"), 50)); }
        catch { }
    }
    private void OnEvent(WireEvent wireEvent)
    {
        if (wireEvent is not HistoryEvent history) return;
        App.DispatcherQueue.TryEnqueue(() =>
        {
            Entries.Clear();
            foreach (var entry in history.Entries.Reverse()) Entries.Add(new HistoryEntryViewModel(entry));
        });
    }
    public void Dispose() => _router.EventReceived -= OnEvent;
}

public sealed class SettingsAboutViewModel
{
    public string AppName => "classick";
    public string VersionLabel => $"Version {System.Reflection.Assembly.GetExecutingAssembly().GetName().Version}";
}
