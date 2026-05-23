using System;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Foundation;

namespace IpodSync_UI;

/// <summary>
/// Wraps the App.xaml-defined <see cref="TaskbarIcon"/>. The icon's
/// Application-resource lifetime is what keeps the app alive in
/// tray-only mode (no visible windows). See the H.NotifyIcon
/// Windowless sample for the canonical pattern.
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private TypedEventHandler<XamlUICommand, ExecuteRequestedEventArgs>? _quitHandler;
    private bool _disposed;

    public event Action? QuitRequested;
    public event Action? ShowSettingsRequested;  // M4 wires the Settings menu item

    public void Initialize()
    {
        _icon = (TaskbarIcon)Application.Current.Resources["TrayIcon"];
        _quitCommand = (XamlUICommand)Application.Current.Resources["QuitCommand"];
        _quitHandler = (_, _) => QuitRequested?.Invoke();
        _quitCommand.ExecuteRequested += _quitHandler;
        // ForceCreate() performs the OS-level NotifyIcon registration.
        _icon.ForceCreate();
    }

    public void SetState(TrayIconState state)
    {
        if (_icon is null) return;
        string iconAsset;
        string tooltip;
        switch (state)
        {
            case TrayIconState.Idle:
                iconAsset = "tray-idle.ico";
                tooltip = "ipod-sync · idle";
                break;
            case TrayIconState.Offline:
                iconAsset = "tray-offline.ico";
                tooltip = "iPod not connected";
                break;
            default:
                iconAsset = "tray-idle.ico";
                tooltip = $"ipod-sync · {state}";
                break;
        }
        _icon.IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(
            new Uri($"ms-appx:///Assets/{iconAsset}"));
        _icon.ToolTipText = tooltip;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (_quitCommand is not null && _quitHandler is not null)
        {
            _quitCommand.ExecuteRequested -= _quitHandler;
        }
        _quitHandler = null;
        _quitCommand = null;
        _icon?.Dispose();
        _icon = null;
    }
}

public enum TrayIconState
{
    Idle,
    Syncing,  // M3
    Error,    // M3
    Offline,
}
