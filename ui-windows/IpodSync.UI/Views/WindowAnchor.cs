using System;
using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using WinRT.Interop;

namespace IpodSync_UI.Views;

/// <summary>
/// Shared bottom-right tray-corner anchoring math used by both
/// PopoverWindow and SettingsWindow. Keeps the VISIBLE bottom-right
/// corner of the window exactly <paramref name="edgeGapDip"/> DIPs
/// from the work-area edges (right of screen, top of taskbar).
///
/// Win11 windows carry an invisible shadow margin (~7px per side at
/// 100% DPI). Subtracting the per-side shadow via
/// DWMWA_EXTENDED_FRAME_BOUNDS keeps the visible corner at exactly
/// the requested gap instead of (gap + shadow) px inward.
/// </summary>
internal static class WindowAnchor
{
    public static void AnchorBottomRight(Microsoft.UI.Xaml.Window window, int edgeGapDip = 12)
    {
        var hwnd = WindowNative.GetWindowHandle(window);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(hwnd));
        AnchorBottomRight(hwnd, appWindow, edgeGapDip);
    }

    public static void AnchorBottomRight(IntPtr hwnd, AppWindow appWindow, int edgeGapDip = 12)
    {
        var displayArea = DisplayArea.GetFromWindowId(
            Win32Interop.GetWindowIdFromWindow(hwnd),
            DisplayAreaFallback.Primary);
        var work = displayArea.WorkArea;

        var dpi = GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;
        var gapPx = (int)Math.Round(edgeGapDip * scale);

        var outerW = appWindow.Size.Width;
        var outerH = appWindow.Size.Height;

        int rightShadow = 0;
        int bottomShadow = 0;
        if (DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS,
                out RECT frame, Marshal.SizeOf<RECT>()) == 0
            && GetWindowRect(hwnd, out RECT win))
        {
            rightShadow = win.right - frame.right;
            bottomShadow = win.bottom - frame.bottom;
        }

        var x = work.X + work.Width - outerW + rightShadow - gapPx;
        var y = work.Y + work.Height - outerH + bottomShadow - gapPx;
        appWindow.Move(new Windows.Graphics.PointInt32(x, y));
    }

    /// <summary>
    /// Resize so the window's CLIENT (content) area is exactly the
    /// requested DIP size. Uses <see cref="AppWindow.ResizeClient"/> —
    /// the WinAppSDK API that sets client area directly, sidestepping
    /// the "compute outer = client + AdjustWindowRectExForDpi" trap.
    ///
    /// The old AdjustWindowRectExForDpi path produced taller-than-asked
    /// windows for borderless+no-titlebar styles, because the native
    /// window class still reports caption + border bits even when the
    /// presenter visually hides them — so the computed outer rect was
    /// padded for chrome that wasn't actually present, and the visible
    /// client ended up larger than the request.
    /// </summary>
    public static void SizeClientArea(IntPtr hwnd, AppWindow appWindow, double widthDip, double heightDip)
    {
        var dpi = GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;
        var clientW = (int)Math.Round(widthDip * scale);
        var clientH = (int)Math.Round(heightDip * scale);
        appWindow.ResizeClient(new Windows.Graphics.SizeInt32(clientW, clientH));
    }

    public static void DisableTransitions(IntPtr hwnd)
    {
        int disable = 1;
        _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, ref disable, sizeof(int));
    }

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr hwnd, out RECT lpRect);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    private static extern int DwmGetWindowAttribute(
        IntPtr hwnd, int attr, out RECT pvAttribute, int cbAttribute);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT { public int left, top, right, bottom; }

    private const int DWMWA_TRANSITIONS_FORCEDISABLED = 3;
    private const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;
}
