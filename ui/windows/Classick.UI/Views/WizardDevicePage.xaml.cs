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

        router.DeviceConnected += OnDeviceConnected;
        router.DeviceDisconnected += OnDeviceDisconnected;
        _vm.BeginScanning();

        try { await daemon.SendAsync(new SubscribeDeviceEventsCommand()); }
        catch (Exception ex) { Debug.WriteLine($"wizard-device: subscribe failed: {ex}"); }
    }

    protected override async void OnNavigatedFrom(NavigationEventArgs e)
    {
        var router = App.Router;
        if (router is not null)
        {
            router.DeviceConnected -= OnDeviceConnected;
            router.DeviceDisconnected -= OnDeviceDisconnected;
        }
        _vm?.EndScanning();
        _vm = null;

        var daemon = App.Daemon;
        if (daemon is null) return;
        try { await daemon.SendAsync(new UnsubscribeDeviceEventsCommand()); }
        catch (Exception ex) { Debug.WriteLine($"wizard-device: unsubscribe failed: {ex}"); }
    }

    private void OnDeviceConnected(DeviceConnectedEvent dc)
    {
        DispatcherQueue.TryEnqueue(() =>
            _vm?.OnDeviceConnected(new IpodIdentityCandidate(dc.Serial, dc.ModelLabel, dc.Drive, dc.Name)));
    }

    private void OnDeviceDisconnected(DeviceDisconnectedEvent dd)
    {
        DispatcherQueue.TryEnqueue(() => _vm?.OnDeviceDisconnected(dd.Serial));
    }
}
