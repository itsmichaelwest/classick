using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using WinRT.Interop;

namespace IpodSync_UI.Views;

public sealed partial class PopoverWindow : Window
{
    public PopoverViewModel ViewModel { get; }
    private readonly DaemonClient _daemon;
    private readonly string _sourceFolder;

    public PopoverWindow(PopoverViewModel vm, DaemonClient daemon, string sourceFolder)
    {
        ViewModel = vm;
        _daemon = daemon;
        _sourceFolder = sourceFolder;
        InitializeComponent();

        // Frameless + Mica backdrop.
        this.SystemBackdrop = new MicaBackdrop();
        var appWindow = GetAppWindow();
        appWindow.SetPresenter(AppWindowPresenterKind.CompactOverlay);
        appWindow.Resize(new Windows.Graphics.SizeInt32(360, 360));

        Activated += OnActivated;
    }

    private AppWindow GetAppWindow()
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Win32Interop.GetWindowIdFromWindow(hwnd);
        return AppWindow.GetFromWindowId(id);
    }

    /// <summary>
    /// Position the popover above the tray icon. H.NotifyIcon exposes
    /// the icon rect via its desktop coordinates; for M4 we approximate
    /// by anchoring to bottom-right of the primary display work area.
    /// M5 polish: use H.NotifyIcon.GetIconPosition once available.
    /// </summary>
    public void AnchorAboveTray()
    {
        var displayArea = DisplayArea.GetFromPoint(new Windows.Graphics.PointInt32(0, 0),
            DisplayAreaFallback.Primary);
        var work = displayArea.WorkArea;
        var appWindow = GetAppWindow();
        var x = work.X + work.Width - appWindow.Size.Width - 12;
        var y = work.Y + work.Height - appWindow.Size.Height - 12;
        appWindow.Move(new Windows.Graphics.PointInt32(x, y));
    }

    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
        // Light-dismiss: close on deactivate.
        if (args.WindowActivationState == WindowActivationState.Deactivated)
        {
            DispatcherQueue.TryEnqueue(Close);
        }
    }

    private async void OnSyncNow(object sender, RoutedEventArgs e)
    {
        try { await _daemon.SendAsync(new TriggerSyncCommand("manual")); }
        catch (Exception ex) { Debug.WriteLine($"popover: trigger_sync failed: {ex}"); }
    }

    private void OnOpenSource(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(_sourceFolder)) return;
        try
        {
            Process.Start(new ProcessStartInfo("explorer.exe", $"\"{_sourceFolder}\"")
                { UseShellExecute = true });
        }
        catch (Exception ex) { Debug.WriteLine($"popover: open source failed: {ex.Message}"); }
    }

    private void OnOpenSettings(object sender, RoutedEventArgs e)
    {
        // TODO(T13): replace with App.RequestOpenSettings() once wired in Wave 3.
        Debug.WriteLine("settings open requested");
        Close();
    }
}
