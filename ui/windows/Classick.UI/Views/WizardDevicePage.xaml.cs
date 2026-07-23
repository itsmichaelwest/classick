using System;
using System.Diagnostics;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Classick_UI.Views;

public sealed partial class WizardDevicePage : Page
{
    private WizardViewModel? _vm;

    public WizardDevicePage() => InitializeComponent();

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _vm = e.Parameter as WizardViewModel;
        DataContext = _vm;
        if (_vm is null) return;

        var router = App.Router;
        var daemon = App.Daemon;
        if (router is null || daemon is null) return;

        router.DeviceInventoryReceived += OnDeviceInventory;
        _vm.BeginScanning();
        _vm.ApplyInventory(
            App.Store.Devices.Values.Select(device => device.Inventory),
            App.Store.Unidentified.Values);

        try { await daemon.SendAsync(new GetInventoryCommand(Guid.NewGuid().ToString("D"))); }
        catch (Exception ex) { Debug.WriteLine($"wizard-device: inventory failed: {ex}"); }
    }

    protected override async void OnNavigatedFrom(NavigationEventArgs e)
    {
        var router = App.Router;
        if (router is not null)
        {
            router.DeviceInventoryReceived -= OnDeviceInventory;
        }
        _vm?.EndScanning();
        _vm = null;

        await Task.CompletedTask;
    }

    private void OnDeviceInventory(DeviceInventoryEvent inventory)
    {
        DispatcherQueue.TryEnqueue(() => _vm?.ApplyInventory(inventory));
    }

    private void OnSelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_vm?.SelectedDevice is { CanAdopt: false }) _vm.SelectedDevice = null;
    }
}
