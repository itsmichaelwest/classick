using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
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
    // Gap between the popover's VISIBLE edge and the work-area edges.
    // 12 DIP per ask — we compensate for the Win11 invisible shadow
    // margin via DWM extended-frame bounds in AnchorAboveTray() so the
    // visible corner really does land 12 DIP from the screen edge.
    private const int EdgeGapDip = 12;

    public PopoverViewModel ViewModel { get; }
    private readonly DaemonClient _daemon;
    private readonly string _sourceFolder;
    private bool _firstActivationDone;

    public PopoverWindow(PopoverViewModel vm, DaemonClient daemon, string sourceFolder)
    {
        ViewModel = vm;
        _daemon = daemon;
        _sourceFolder = sourceFolder;
        InitializeComponent();

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = GetAppWindow();

        // Borderless, non-resizable, no titlebar, no min/max, always on top —
        // the standard chrome for a tray-anchored flyout.
        var presenter = OverlappedPresenter.Create();
        presenter.SetBorderAndTitleBar(hasBorder: true, hasTitleBar: false);
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.IsAlwaysOnTop = true;
        appWindow.SetPresenter(presenter);
        appWindow.IsShownInSwitchers = false;

        // Acrylic for a tray flyout — Mica is for the main app window
        // background; flyouts get Acrylic per Fluent material guidance.
        SystemBackdrop = new DesktopAcrylicBackdrop();

        // Disable DWM open/close/move/resize transition animations so the
        // window snaps into place when it appears.
        DisableWindowTransitions(hwnd);

        // Initial size + anchor BEFORE first Activate() so the flyout
        // appears already in the right place. The DWM extended-frame
        // bounds aren't available pre-activation, so we use an estimated
        // shadow offset here and re-anchor exactly on first Activated.
        SizeWindowToContent(hwnd, appWindow);
        AnchorAboveTray(hwnd, appWindow);

        // Re-anchor on first activation with the real DWM frame, then
        // (optionally) hook light-dismiss. The first SizeChanged is the
        // earliest point DWM has computed the visible-frame rect.
        Activated += OnActivatedFirstTime;
    }

    private void OnActivatedFirstTime(object sender, WindowActivatedEventArgs args)
    {
        if (_firstActivationDone) return;
        _firstActivationDone = true;
        Activated -= OnActivatedFirstTime;

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = GetAppWindow();
        AnchorAboveTray(hwnd, appWindow);

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

    /// <summary>
    /// Read Width/Height from the XAML root and resize the HWND so its
    /// CLIENT area matches those DIPs. AppWindow.Resize sets the OUTER
    /// rect (including the invisible chrome), so without this conversion
    /// the visible content area is too small and the layout clips.
    /// </summary>
    private void SizeWindowToContent(IntPtr hwnd, AppWindow appWindow)
    {
        if (Content is not FrameworkElement root) return;

        var dpi = GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;

        // Honour explicit XAML Width/Height; fall back to a measured
        // DesiredSize so a designer can switch to auto-size without
        // touching this code.
        double widthDip = !double.IsNaN(root.Width) && root.Width > 0 ? root.Width : 0;
        double heightDip = !double.IsNaN(root.Height) && root.Height > 0 ? root.Height : 0;
        if (widthDip <= 0 || heightDip <= 0)
        {
            root.Measure(new Windows.Foundation.Size(
                double.PositiveInfinity, double.PositiveInfinity));
            if (widthDip <= 0) widthDip = root.DesiredSize.Width;
            if (heightDip <= 0) heightDip = root.DesiredSize.Height;
        }
        if (widthDip <= 0) widthDip = 360;
        if (heightDip <= 0) heightDip = 208;

        var clientW = (int)Math.Round(widthDip * scale);
        var clientH = (int)Math.Round(heightDip * scale);

        // Convert client rect → outer rect via AdjustWindowRectExForDpi —
        // adds the chrome thickness from the actual window styles + DPI,
        // so e.g. on Win11 100% DPI an outer width comes out roughly 16px
        // larger than the requested client width.
        var rect = new RECT { left = 0, top = 0, right = clientW, bottom = clientH };
        var style = GetWindowLongPtrW(hwnd, GWL_STYLE).ToInt32();
        var exStyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE).ToInt32();
        AdjustWindowRectExForDpi(ref rect, (uint)style, false, (uint)exStyle, (uint)dpi);
        var outerW = rect.right - rect.left;
        var outerH = rect.bottom - rect.top;

        appWindow.Resize(new Windows.Graphics.SizeInt32(outerW, outerH));
    }

    /// <summary>
    /// Anchor the popover so its VISIBLE bottom-right corner sits
    /// EdgeGapDip from the work-area edges (right of screen, top of
    /// taskbar). Win11 windows have an invisible shadow margin (~7px
    /// per side at 100% DPI); without DWM compensation the visible
    /// corner ends up shadow-px further inward than intended — which
    /// is the "16px instead of 12px" symptom.
    /// </summary>
    public void AnchorAboveTray()
        => AnchorAboveTray(WindowNative.GetWindowHandle(this), GetAppWindow());

    private void AnchorAboveTray(IntPtr hwnd, AppWindow appWindow)
    {
        var displayArea = DisplayArea.GetFromWindowId(
            Win32Interop.GetWindowIdFromWindow(hwnd),
            DisplayAreaFallback.Primary);
        var work = displayArea.WorkArea;

        var dpi = GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;
        var gapPx = (int)Math.Round(EdgeGapDip * scale);

        var outerW = appWindow.Size.Width;
        var outerH = appWindow.Size.Height;

        // DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) returns
        // the visible frame rect; subtracting from GetWindowRect gives
        // us the per-side invisible margins. Pre-activation, DWM may
        // return all zeros — the first-activation re-anchor catches
        // the gap in that case.
        int rightShadow = 0;
        int bottomShadow = 0;
        if (DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS,
                out RECT frame, Marshal.SizeOf<RECT>()) == 0
            && GetWindowRect(hwnd, out RECT win))
        {
            rightShadow = win.right - frame.right;
            bottomShadow = win.bottom - frame.bottom;
        }

        // Push the OUTER rect rightward by the shadow margin so the
        // VISIBLE right edge lands at (work-right − gapPx).
        var x = work.X + work.Width - outerW + rightShadow - gapPx;
        var y = work.Y + work.Height - outerH + bottomShadow - gapPx;
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

    // --- Win32 interop ------------------------------------------------------

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr hwnd, out RECT lpRect);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AdjustWindowRectExForDpi(
        ref RECT lpRect, uint dwStyle, bool bMenu, uint dwExStyle, uint dpi);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr GetWindowLongPtrW(IntPtr hWnd, int nIndex);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmGetWindowAttribute(
        IntPtr hwnd, int attr, out RECT pvAttribute, int cbAttribute);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT { public int left, top, right, bottom; }

    private const int GWL_STYLE = -16;
    private const int GWL_EXSTYLE = -20;
    private const int DWMWA_TRANSITIONS_FORCEDISABLED = 3;
    private const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;

    private static void DisableWindowTransitions(IntPtr hwnd)
    {
        int disable = 1;
        _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, ref disable, sizeof(int));
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
        App.RequestOpenSettings();
        Close();
    }
}
