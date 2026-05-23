using System;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI;

/// <summary>
/// Owns the system tray icon. M2 ships idle / offline states + Quit
/// menu item. M3 adds syncing / error states + Sync Now / Settings.
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private bool _disposed;

    public event Action? QuitRequested;
    public event Action? ShowSettingsRequested;  // M4 wires the Settings menu item

    public void Initialize()
    {
        var menu = new MenuFlyout();
        var quit = new MenuFlyoutItem { Text = "Quit" };
        quit.Click += (_, _) => QuitRequested?.Invoke();
        menu.Items.Add(quit);

        _icon = new TaskbarIcon
        {
            IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(
                new Uri("ms-appx:///Assets/tray-idle.ico")),
            ToolTipText = "ipod-sync · idle",
            ContextFlyout = menu,
        };
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
