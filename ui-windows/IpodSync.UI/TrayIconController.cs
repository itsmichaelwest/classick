using System;
using System.IO;
using H.NotifyIcon;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;

namespace IpodSync_UI;

public enum TrayState { Idle, Syncing, Error, Offline }

public sealed class TrayIconController : IDisposable
{
    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private XamlUICommand? _syncNowCommand;
    private XamlUICommand? _settingsCommand;
    private XamlUICommand? _openPopoverCommand;
    private TrayState _state = TrayState.Offline;

    public event Action? QuitRequested;
    public event Action? SyncNowRequested;
    public event Action? SettingsRequested;
    public event Action? PopoverRequested;

    public void Initialize()
    {
        _icon = (TaskbarIcon)Application.Current.Resources["TrayIcon"];
        _quitCommand = (XamlUICommand)Application.Current.Resources["QuitCommand"];
        _syncNowCommand = (XamlUICommand)Application.Current.Resources["SyncNowCommand"];
        _settingsCommand = (XamlUICommand)Application.Current.Resources["SettingsCommand"];
        _openPopoverCommand = (XamlUICommand)Application.Current.Resources["OpenPopoverCommand"];

        _quitCommand.ExecuteRequested += (_, _) => QuitRequested?.Invoke();
        _syncNowCommand.ExecuteRequested += (_, _) => SyncNowRequested?.Invoke();
        _settingsCommand.ExecuteRequested += (_, _) => SettingsRequested?.Invoke();
        _openPopoverCommand.ExecuteRequested += (_, _) => PopoverRequested?.Invoke();

        _icon.ForceCreate();
        SetState(TrayState.Offline, "iPod not connected");
    }

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
