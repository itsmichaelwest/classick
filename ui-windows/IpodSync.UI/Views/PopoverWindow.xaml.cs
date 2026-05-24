using System;
using System.Diagnostics;
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
    // Popover footprint (Figma 360×192). Source-of-truth in code so
    // WinAppSDK's window-header reservation doesn't surface dark bands
    // around a fixed-size XAML Grid. Hot-reload these constants in
    // dev to tweak the popover footprint.
    private const int PopoverWidthDip = 360;
    private const int PopoverHeightDip = 192;

    // Gap between the popover's VISIBLE edge and the work-area edges.
    // 12 DIP per Figma — WindowAnchor compensates for the invisible
    // shadow margin via DWM extended-frame bounds so the visible
    // corner lands exactly 12 DIP from the work-area edges.
    private const int EdgeGapDip = 12;

    public PopoverViewModel ViewModel { get; }
    private readonly DaemonClient _daemon;
    private bool _firstActivationDone;

    public PopoverWindow(PopoverViewModel vm, DaemonClient daemon)
    {
        ViewModel = vm;
        _daemon = daemon;
        InitializeComponent();

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = GetAppWindow();

        var presenter = OverlappedPresenter.Create();
        presenter.SetBorderAndTitleBar(hasBorder: true, hasTitleBar: false);
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.IsAlwaysOnTop = true;
        appWindow.SetPresenter(presenter);
        appWindow.IsShownInSwitchers = false;

        // WinAppSDK 2.x reserves an invisible window-header drag-area
        // band inside the client rect even with hasTitleBar=false.
        // Collapse it via PreferredHeightOption + extend content so
        // the Grid fills the full client edge-to-edge — without this
        // the popover renders with ~16 DIP of dark chrome above and
        // below the Grid.
        if (AppWindowTitleBar.IsCustomizationSupported())
        {
            appWindow.TitleBar.ExtendsContentIntoTitleBar = true;
            appWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Collapsed;
        }
        ExtendsContentIntoTitleBar = true;

        SystemBackdrop = new DesktopAcrylicBackdrop();

        WindowAnchor.DisableTransitions(hwnd);
        WindowAnchor.SizeClientArea(hwnd, appWindow, PopoverWidthDip, PopoverHeightDip);
        WindowAnchor.AnchorBottomRight(hwnd, appWindow, EdgeGapDip);

        Activated += OnActivatedFirstTime;
    }

    private void OnActivatedFirstTime(object sender, WindowActivatedEventArgs args)
    {
        if (_firstActivationDone) return;
        _firstActivationDone = true;
        Activated -= OnActivatedFirstTime;

        var hwnd = WindowNative.GetWindowHandle(this);
        WindowAnchor.AnchorBottomRight(hwnd, GetAppWindow(), EdgeGapDip);

#if !DEBUG
        // Light-dismiss on focus loss. Disabled in DEBUG so the XAML Hot
        // Reload / Live Visual Tree inspector can hold focus without the
        // window vanishing the moment you click into VS.
        Activated += OnActivated;
#endif
    }

    private AppWindow GetAppWindow()
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Win32Interop.GetWindowIdFromWindow(hwnd);
        return AppWindow.GetFromWindowId(id);
    }

    public void AnchorAboveTray()
        => WindowAnchor.AnchorBottomRight(this, EdgeGapDip);

    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
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

    private async void OnCancelSync(object sender, RoutedEventArgs e)
    {
        try { await _daemon.SendAsync(new CancelSyncCommand()); }
        catch (Exception ex) { Debug.WriteLine($"popover: cancel_sync failed: {ex}"); }
    }

    private void OnOpenSettings(object sender, RoutedEventArgs e)
    {
        App.RequestOpenSettings();
        Close();
    }
}
