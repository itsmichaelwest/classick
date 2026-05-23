using System;
using System.IO;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Windows.Foundation;

namespace IpodSync_UI;

/// <summary>
/// 4-state tray icon driven by daemon StatusUpdate events.
/// </summary>
public enum TrayState { Idle, Syncing, Error, Offline }

/// <summary>
/// Owns the H.NotifyIcon-backed system-tray icon. Lifetime is anchored
/// by the TaskbarIcon defined as an Application.Resource in App.xaml
/// (so the dispatcher stays alive while no windows are open).
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private XamlUICommand? _syncNowCommand;
    private TrayState _state = TrayState.Offline;

    public event Action? QuitRequested;
    public event Action? SyncNowRequested;

    public void Initialize()
    {
        _icon = (TaskbarIcon)Application.Current.Resources["TrayIcon"];
        _quitCommand = (XamlUICommand)Application.Current.Resources["QuitCommand"];
        _syncNowCommand = (XamlUICommand)Application.Current.Resources["SyncNowCommand"];
        _quitCommand.ExecuteRequested += (_, _) => QuitRequested?.Invoke();
        _syncNowCommand.ExecuteRequested += (_, _) => SyncNowRequested?.Invoke();
        _icon.ForceCreate();
        SetState(TrayState.Offline, "iPod not connected");
    }

    /// <summary>
    /// Swap icon + tooltip atomically. Safe to call from any thread —
    /// H.NotifyIcon marshals to the UI thread internally.
    /// </summary>
    public void SetState(TrayState state, string tooltip)
    {
        if (_icon is null) return;
        _state = state;
        var iconPath = state switch
        {
            TrayState.Idle    => "Assets/tray-idle.ico",
            TrayState.Syncing => "Assets/tray-syncing.ico",
            TrayState.Error   => "Assets/tray-error.ico",
            TrayState.Offline => "Assets/tray-offline.ico",
            _                  => "Assets/tray-offline.ico",
        };
        var abs = Path.Combine(AppContext.BaseDirectory, iconPath);
        if (File.Exists(abs))
        {
            _icon.IconSource = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri(abs));
        }
        _icon.ToolTipText = tooltip;
    }

    public TrayState CurrentState => _state;

    public void Dispose()
    {
        _icon?.Dispose();
        _icon = null;
    }
}
