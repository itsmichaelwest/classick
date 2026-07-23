using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public partial class PopoverViewModel
{
    [ObservableProperty] private string deviceHardwareSummary = "";
    [ObservableProperty] private string deviceHardwareProvenance = "";
    [ObservableProperty] private string deviceReadinessText = "";
    [ObservableProperty] private string deviceGuidance = "";
    [ObservableProperty] private string deviceArtworkUri = "ms-appx:///Assets/ipod-generic.svg";
    [ObservableProperty] private string deviceArtworkDescription = "iPod";
    [ObservableProperty] private bool deviceReadyForSync = true;

    partial void OnDeviceReadyForSyncChanged(bool value) =>
        OnPropertyChanged(nameof(ShowSyncNowButton));

    partial void OnDeviceGuidanceChanged(string value) =>
        OnPropertyChanged(nameof(ShowDeviceGuidance));

    public void Update(IdentifiedDeviceSnapshot device)
    {
        ArgumentNullException.ThrowIfNull(device);
        var presentation = DevicePresentationFactory.For(device);
        DisplayedDeviceId = device.DeviceId;
        DeviceLabel = presentation.Title;
        DeviceHardwareSummary = presentation.HardwareSummary;
        DeviceHardwareProvenance = presentation.HardwareProvenance;
        DeviceReadinessText = presentation.Status;
        DeviceGuidance = presentation.Guidance;
        DeviceArtworkUri = presentation.Artwork.AssetUri;
        DeviceArtworkDescription = presentation.Artwork.AccessibleDescription;
        DeviceReadyForSync = device.Readiness == DeviceReadiness.Ready &&
            device.ProfileStatus == ProfileStatus.Adopted;
        FinishingSync = false;
        Paused = device.Phase == DevicePhase.Paused;
        Syncing = device.Phase == DevicePhase.Syncing;
        IpodConnected = device.Connected;
        ApplyStorage(device.Storage);
        if (Syncing)
        {
            StatusText = "Syncing iPod…";
            LastSyncedLabel = "Syncing now";
        }
        else if (!device.Connected)
        {
            StatusText = "iPod not connected";
            LastSyncedLabel = "";
        }
        else if (device.LastTerminalError is { } error)
        {
            StatusText = $"Last sync failed: {error}";
        }
        else
        {
            StatusText = "Up to date · iPod connected";
        }
    }
}
