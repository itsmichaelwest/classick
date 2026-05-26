using System;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using H.NotifyIcon;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.Win32;

namespace Classick_UI;

public enum TrayState { Idle, Syncing, Error, Offline }

/// <summary>
/// Owns the system-tray icon and its context menu.
///
/// Icon resolution at runtime:
///   Assets/tray/{StateName}_{Variant}_{Size}.png
///
/// - StateName  : Default | Syncing | Error | NotConnected  (mapped from TrayState)
/// - Variant    : Light (dark glyph for light taskbar) | Dark (light glyph for dark taskbar)
///                Driven by HKCU\...\Themes\Personalize\SystemUsesLightTheme; reacts to
///                live changes via SystemEvents.UserPreferenceChanged.
/// - Size       : 16 | 20 | 24 | 32 — hand-tuned PNG per size, picked from
///                GetSystemMetrics(SM_CXSMICON) so the HICON Windows blits into the
///                notification area matches the tray's pixel grid exactly (no
///                bilinear scaling artefacts). Refreshed on DisplaySettingsChanged
///                so DPI changes apply live.
/// </summary>
public sealed class TrayIconController : IDisposable
{
    private const string PersonalizeKey =
        @"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

    // The four hand-tuned source sizes, low → high. Order matters: SnapToSourceSize
    // walks this in ascending order and picks the first match >= the requested size,
    // falling back to the largest available.
    private static readonly int[] SourceSizes = { 16, 20, 24, 32 };

    private const int SM_CXSMICON = 49;

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int nIndex);

    [DllImport("user32.dll")]
    private static extern bool DestroyIcon(IntPtr hIcon);

    private TaskbarIcon? _icon;
    private XamlUICommand? _quitCommand;
    private XamlUICommand? _syncNowCommand;
    private XamlUICommand? _settingsCommand;
    private XamlUICommand? _openPopoverCommand;
    private DispatcherQueue? _dispatcher;

    private TrayState _state = TrayState.Offline;
    private string _tooltip = "iPod not connected";
    private bool _taskbarUsesLightTheme;
    private int _iconSize = 32;

    // Track the HICON we last handed the tray so we can DestroyIcon it
    // when swapping in a new one. Without this we leak one HICON per
    // theme/DPI/state change for the lifetime of the process.
    private IntPtr _currentHIcon = IntPtr.Zero;
    private Icon? _currentIcon;

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

        _dispatcher = DispatcherQueue.GetForCurrentThread();
        _taskbarUsesLightTheme = ReadTaskbarUsesLightTheme();
        _iconSize = ReadCurrentTrayIconSize();

        // SystemEvents fires on its own thread; marshal back to the UI thread
        // before touching the TaskbarIcon (its IconSource is XAML-thread-affined).
        SystemEvents.UserPreferenceChanged += OnUserPreferenceChanged;
        SystemEvents.DisplaySettingsChanged += OnDisplaySettingsChanged;

        // Apply the icon BEFORE ForceCreate. ForceCreate triggers
        // H.NotifyIcon's WriteableBitmap → HICON conversion, which throws
        // (ERROR_INSUFFICIENT_BUFFER inside WinRT marshalling) if IconSource
        // is null or unresolvable. Setting it via file path here sidesteps the
        // MRT indexer entirely.
        _state = TrayState.Offline;
        _tooltip = "iPod not connected";
        ApplyCurrentIcon();
        _icon.ForceCreate();
    }

    /// <summary>Show or hide the tray icon. Used during first-run /
    /// re-pair flows to keep the tray hidden while the wizard owns
    /// the user's attention — the wizard is the primary surface
    /// until an iPod identity is committed to config.</summary>
    public void SetVisible(bool visible)
    {
        if (_icon is null) return;
        _icon.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
    }

    public void SetState(TrayState state, string tooltip)
    {
        if (_icon is null) return;
        _state = state;
        _tooltip = tooltip;
        ApplyCurrentIcon();
    }

    public TrayState CurrentState => _state;

    private void ApplyCurrentIcon()
    {
        if (_icon is null) return;

        var stateName = _state switch
        {
            TrayState.Idle     => "Default",
            TrayState.Syncing  => "Syncing",
            TrayState.Error    => "Error",
            TrayState.Offline  => "NotConnected",
            _                  => "Default",
        };
        var variant = _taskbarUsesLightTheme ? "Light" : "Dark";

        var path = ResolveIconPath(stateName, variant, _iconSize);
        if (path is null)
        {
            System.Diagnostics.Debug.WriteLine(
                $"TrayIconController: no tray PNG found for state={stateName} variant={variant} size={_iconSize}");
            _icon.ToolTipText = _tooltip;
            return;
        }

        // We use the .Icon (System.Drawing.Icon) property rather than
        // .IconSource (Microsoft.UI.Xaml.Media.ImageSource) because the
        // ImageSource path is async — H.NotifyIcon tries to convert it
        // to an HICON synchronously inside ForceCreate / refresh, but
        // a BitmapImage created from a URI hasn't decoded yet, so the
        // WriteableBitmap pixel buffer is empty and the WinRT marshal
        // throws ERROR_INSUFFICIENT_BUFFER ("the data area passed to a
        // system call is too small"). Loading via GDI+ Bitmap and
        // GetHicon() gives us a fully-populated HICON synchronously.
        try
        {
            using var bitmap = new Bitmap(path);
            var newHIcon = bitmap.GetHicon();
            var newIcon = Icon.FromHandle(newHIcon);

            _icon.Icon = newIcon;

            // Now that the tray has the new HICON, free the old one.
            var oldHIcon = _currentHIcon;
            var oldIcon = _currentIcon;
            _currentHIcon = newHIcon;
            _currentIcon = newIcon;
            oldIcon?.Dispose();
            if (oldHIcon != IntPtr.Zero) DestroyIcon(oldHIcon);
        }
        catch (Exception e)
        {
            System.Diagnostics.Debug.WriteLine(
                $"TrayIconController: failed to load tray icon from '{path}': {e.Message}");
        }
        _icon.ToolTipText = _tooltip;
    }

    /// <summary>
    /// Walks a fallback chain so a single missing PNG can't take down
    /// startup: requested size → other sizes (largest-first) → opposite
    /// variant → Default state. Returns the first existing absolute path,
    /// or null if nothing under Assets\tray\ exists at all (deployment bug).
    /// </summary>
    private static string? ResolveIconPath(string stateName, string variant, int size)
    {
        var states = stateName == "Default" ? new[] { stateName } : new[] { stateName, "Default" };
        var variants = new[] { variant, variant == "Light" ? "Dark" : "Light" };
        // Try the requested size first, then descend through other source sizes
        // (largest-first) so we prefer a downscaled crisp icon over an upscaled
        // blurry one.
        var sizes = new[] { size }
            .Concat(SourceSizes.Where(s => s != size).OrderByDescending(s => s))
            .ToArray();

        foreach (var st in states)
        foreach (var v in variants)
        foreach (var sz in sizes)
        {
            var abs = Path.Combine(AppContext.BaseDirectory, "Assets", "tray", $"{st}_{v}_{sz}.png");
            if (File.Exists(abs)) return abs;
        }
        return null;
    }

    private static bool ReadTaskbarUsesLightTheme()
    {
        // Prefer UISettings.GetColorValue(Background) — it returns the
        // actual system theme background color, which matches what the
        // taskbar renders. The HKCU SystemUsesLightTheme registry
        // value is unreliable on some Win11 setups (custom theme
        // packs, certain accent configurations) where it can stay at
        // 1 even when the user is visually on a dark taskbar.
        //
        // Falls back to the registry read if UISettings throws (e.g.
        // unpackaged-app COM init issues).
        try
        {
            var ui = new Windows.UI.ViewManagement.UISettings();
            var bg = ui.GetColorValue(Windows.UI.ViewManagement.UIColorType.Background);
            // Light theme background ≈ white; dark theme ≈ black.
            // The midpoint (384 / 3 ≈ 128 per channel) splits the
            // two cleanly even for tinted themes.
            return (bg.R + bg.G + bg.B) > 384;
        }
        catch { /* fall through to registry */ }

        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(PersonalizeKey);
            return (key?.GetValue("SystemUsesLightTheme") as int?) == 1;
        }
        catch
        {
            return false;
        }
    }

    private static int ReadCurrentTrayIconSize()
    {
        // SM_CXSMICON is the small-icon dimension Windows uses for the notification
        // area at the current system DPI. Maps roughly to:
        //   100% DPI → 16    150% DPI → 24
        //   125% DPI → 20    200% DPI → 32
        // We snap to the closest size we have hand-tuned art for.
        int px;
        try { px = GetSystemMetrics(SM_CXSMICON); }
        catch { px = 16; }
        return SnapToSourceSize(px);
    }

    private static int SnapToSourceSize(int requested)
    {
        if (requested <= SourceSizes[0]) return SourceSizes[0];
        for (var i = 0; i < SourceSizes.Length - 1; i++)
        {
            var lo = SourceSizes[i];
            var hi = SourceSizes[i + 1];
            if (requested >= lo && requested <= hi)
            {
                // Round-half-up to the nearer source size so DPI buckets that
                // sit between two sources (e.g. 28 between 24 and 32) bias
                // upward — easier for Windows to downscale crisply than to
                // upscale.
                return (requested - lo) <= (hi - requested) ? lo : hi;
            }
        }
        return SourceSizes[^1];
    }

    private void OnUserPreferenceChanged(object? sender, UserPreferenceChangedEventArgs e)
    {
        // General covers theme/personalization changes on Win10+; checking it
        // narrowly avoids re-reading the registry on unrelated pref changes
        // (locale, accessibility, etc.) that also fire this event.
        if (e.Category != UserPreferenceCategory.General) return;

        var next = ReadTaskbarUsesLightTheme();
        if (next == _taskbarUsesLightTheme) return;
        _taskbarUsesLightTheme = next;

        _dispatcher?.TryEnqueue(ApplyCurrentIcon);
    }

    private void OnDisplaySettingsChanged(object? sender, EventArgs e)
    {
        // Fires for resolution, refresh rate, AND DPI changes. We only care
        // about the size bucket; re-snap and reapply only if it changed.
        var next = ReadCurrentTrayIconSize();
        if (next == _iconSize) return;
        _iconSize = next;

        _dispatcher?.TryEnqueue(ApplyCurrentIcon);
    }

    public void Dispose()
    {
        SystemEvents.UserPreferenceChanged -= OnUserPreferenceChanged;
        SystemEvents.DisplaySettingsChanged -= OnDisplaySettingsChanged;
        _icon?.Dispose();
        _icon = null;
        _currentIcon?.Dispose();
        _currentIcon = null;
        if (_currentHIcon != IntPtr.Zero)
        {
            DestroyIcon(_currentHIcon);
            _currentHIcon = IntPtr.Zero;
        }
    }
}
