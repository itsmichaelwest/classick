using System;
using System.Diagnostics;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Composition.SystemBackdrops;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.System;
using WinRT.Interop;

namespace Classick_UI.Views;

public sealed partial class PopoverWindow : Window
{
    // Popover footprint (Figma 360×192). Source-of-truth in code so
    // WinAppSDK's window-header reservation doesn't surface dark bands
    // around a fixed-size XAML Grid. Hot-reload these constants in
    // dev to tweak the popover footprint.
    private const int PopoverWidthDip = 360;
    private const int PopoverHeightDip = 156;

    // Gap between the popover's VISIBLE edge and the work-area edges.
    // 12 DIP per Figma — WindowAnchor compensates for the invisible
    // shadow margin via DWM extended-frame bounds so the visible
    // corner lands exactly 12 DIP from the work-area edges. The soft
    // drop-shadow gradient that extends below the visible bottom edge
    // is clipped naturally by the (topmost) taskbar because the
    // popover is NOT topmost — see the IsAlwaysOnTop comment in the
    // constructor.
    private const int EdgeGapDip = 12;

    /// <summary>
    /// Runtime debug lock: when true, the popover stays open through
    /// focus loss so a developer can drag the XAML Live Visual Tree
    /// over it without the window dismissing. Toggled via Ctrl+Shift+L
    /// (DEBUG builds only — the hotkey itself is <c>#if DEBUG</c>-gated
    /// so the flag is unreachable in shipping builds). Static so the
    /// lock survives popover open/close cycles within a session.
    /// </summary>
    public static bool DebugLockEnabled { get; private set; }

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
        // IsAlwaysOnTop = false (default) is the correct Z-order for
        // a tray popover. The Windows taskbar (Shell_TrayWnd) is
        // itself topmost; if we set the popover topmost too, BOTH
        // sit in the topmost layer and within that layer activation
        // order wins — the popover lands above the taskbar and its
        // drop-shadow paints over the taskbar's top edge.
        //
        // With IsAlwaysOnTop=false the popover lives in the normal
        // Z-order layer, below the topmost taskbar; the DWM
        // compositor clips our shadow against the taskbar's window
        // region for free, no shadow-bleed and no hand-tweaking of
        // EdgeGapDip.
        //
        // We don't lose "stays above other apps" UX because the
        // popover auto-dismisses on focus loss anyway — if the user
        // clicks anything else, the popover closes before another
        // window could obscure it.
        appWindow.IsShownInSwitchers = false;
        appWindow.SetPresenter(presenter);

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

        // Hide the window now — Activate() from the caller would
        // otherwise show it BEFORE the XAML has laid out and
        // composited, producing a brief blank-acrylic flash. We
        // Show() in OnContentLoaded once the visual tree is ready.
        appWindow.Hide();

        Activated += OnActivatedFirstTime;

        // Always-on auto-dismiss: clicking outside the popover closes
        // it. Was previously #if !DEBUG-gated to keep the XAML Live
        // Visual Tree usable, but now we expose Ctrl+Shift+L as a
        // runtime debug lock so inspection works without disabling
        // dismissal globally. Shipping builds get the same dismiss
        // behaviour they always had.
        Activated += OnActivated;

        // Show as soon as the root grid raises Loaded (= first
        // measure + arrange complete). The compositor has the visual
        // tree by then, so Show + paint happen in the same frame and
        // there's no empty-window flash.
        if (Content is FrameworkElement contentRoot)
        {
            contentRoot.Loaded += OnContentLoaded;
        }

        // Always-on key handling: Escape dismisses the popover (or
        // clears a pending prompt overlay). DEBUG builds attach
        // additional hotkeys for prompt-scenario triggering + the
        // runtime lock — see OnDebugKeyDown.
        //
        // AddHandler with handledEventsToo:true so the key still
        // fires after focus moves to a child Button (Button marks
        // Space/Enter handled but bubbles every other key; this
        // belt-and-suspenders handles future XAML changes too).
        if (Content is UIElement keyRoot)
        {
            keyRoot.AddHandler(
                UIElement.KeyDownEvent,
                new KeyEventHandler(OnKeyDown),
                handledEventsToo: true);
        }
    }

    private void OnContentLoaded(object sender, RoutedEventArgs e)
    {
        // Single-shot — unsubscribe so we don't re-show on subsequent
        // layout passes (theme change, reflow, etc.).
        if (sender is FrameworkElement fe) fe.Loaded -= OnContentLoaded;
        var appWindow = GetAppWindow();
        // Re-anchor after layout in case the measured client size
        // differed from PopoverWidthDip/HeightDip (very rare with our
        // fixed-size root grid but cheap to redo).
        WindowAnchor.AnchorBottomRight(
            WindowNative.GetWindowHandle(this), appWindow, EdgeGapDip);
        appWindow.Show();
    }

    private void OnActivatedFirstTime(object sender, WindowActivatedEventArgs args)
    {
        if (_firstActivationDone) return;
        _firstActivationDone = true;
        Activated -= OnActivatedFirstTime;

        var hwnd = WindowNative.GetWindowHandle(this);
        WindowAnchor.AnchorBottomRight(hwnd, GetAppWindow(), EdgeGapDip);
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
        if (args.WindowActivationState != WindowActivationState.Deactivated) return;
        // Debug lock holds the popover open through focus loss so a
        // developer can drag the XAML Live Visual Tree / VS Hot
        // Reload over it. Toggle via Ctrl+Shift+L; always false in
        // shipping builds since the hotkey is #if DEBUG-gated.
        if (DebugLockEnabled) return;
        DispatcherQueue.TryEnqueue(Close);
    }

    private async void OnSyncNow(object sender, RoutedEventArgs e)
    {
        var command = ViewModel.CreateWireTriggerSyncCommand(Guid.NewGuid().ToString("D"));
        if (command is null) return;
        try { await _daemon.SendAsync(command); }
        catch (Exception ex) { Debug.WriteLine($"popover: trigger_sync failed: {ex}"); }
    }

    private async void OnConnectSource(object sender, RoutedEventArgs e)
    {
        var requestId = Guid.NewGuid().ToString("D");
        var command = ViewModel.CreateWireSourceRetryCommand(requestId);
        if (command is null) return;

        try
        {
            await _daemon.SendAsync(command);
        }
        catch (Exception ex)
        {
            ViewModel.SourceRetrySendFailed(requestId);
            Debug.WriteLine($"popover: retry_source_mount failed: {ex}");
        }
    }

    private async void OnCancelSync(object sender, RoutedEventArgs e)
    {
        var command = ViewModel.CreateWireCancelSyncCommand(Guid.NewGuid().ToString("D"));
        if (command is null) return;
        try { await _daemon.SendAsync(command); }
        catch (Exception ex) { Debug.WriteLine($"popover: cancel_sync failed: {ex}"); }
    }

    private async void OnPauseSync(object sender, RoutedEventArgs e)
    {
        var command = ViewModel.CreateWirePauseSyncCommand(Guid.NewGuid().ToString("D"));
        if (command is null) return;
        try { await _daemon.SendAsync(command); }
        catch (Exception ex) { Debug.WriteLine($"popover: pause failed: {ex}"); }
    }

    private void OnOpenSettings(object sender, RoutedEventArgs e)
    {
        App.RequestOpenSettings();
        Close();
    }

    /// <summary>
    /// Fires when the user clicks one of the dynamic option buttons
    /// in the prompt overlay. Resolves the option index from the
    /// button's Content (the option string the daemon supplied),
    /// dispatches a <see cref="DecidePromptCommand"/> with that
    /// index, and hides the overlay so the popover returns to the
    /// progress view. The daemon forwards the choice to the sync
    /// subprocess's stdin and the apply loop's await_prompt returns.
    /// </summary>
    private async void OnPromptOptionClicked(object sender, RoutedEventArgs e)
    {
        if (sender is not Microsoft.UI.Xaml.Controls.Button btn) return;
        if (btn.Content is not string label) return;

        var index = -1;
        for (int i = 0; i < ViewModel.PromptOptions.Count; i++)
        {
            if (ViewModel.PromptOptions[i] == label) { index = i; break; }
        }
        if (index < 0)
        {
            Debug.WriteLine($"popover: prompt option '{label}' not found in current options");
            return;
        }

        var command = ViewModel.CreateWirePromptDecisionCommand(index, Guid.NewGuid().ToString("D"));
        if (command is null) return;
        // Optimistic dismiss — the daemon's response is fire-and-
        // forward; keeping the overlay up until a TrackStart arrives
        // would leave the user staring at the prompt while the
        // subprocess does prep work.
        ViewModel.ClearPrompt();
        try
        {
            await _daemon.SendAsync(command);
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"popover: decide_prompt failed: {ex}");
        }
    }

    /// <summary>
    /// Always-on keyboard handler. Escape is context-sensitive:
    /// when a prompt overlay is active it clears the overlay (so the
    /// user can recover from a stuck sync), otherwise it dismisses
    /// the popover. DEBUG builds dispatch additional Ctrl+Shift
    /// chords through <see cref="OnDebugKeyDownChord"/>.
    /// </summary>
    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Escape)
        {
            if (ViewModel.PromptActive) ViewModel.ClearPrompt();
            else Close();
            e.Handled = true;
            return;
        }

#if DEBUG
        OnDebugKeyDownChord(e);
#endif
    }

#if DEBUG
    private int _debugScenarioIndex;

    /// <summary>
    /// DEBUG-only Ctrl+Shift hotkeys for iterating on the prompt
    /// overlay without driving a real sync into a real prompt, and
    /// toggling the runtime focus-loss-dismiss lock so the XAML Live
    /// Visual Tree / VS Hot Reload can hold focus over the popover:
    ///
    ///   Ctrl+Shift+1 → short prompt (2 options)
    ///   Ctrl+Shift+2 → source-change prompt (3 options, medium)
    ///   Ctrl+Shift+3 → retry-on-failure prompt (3 options, long)
    ///   Ctrl+Shift+0 → clear overlay
    ///   Ctrl+Shift+L → toggle <see cref="DebugLockEnabled"/>
    ///
    /// NumberPad0/1/2/3 alternates so the numeric keypad works too.
    /// Each scenario fires through <see cref="PopoverViewModel.ApplyIpcProgress"/>
    /// so XAML hot-reload reflects edits to the overlay immediately.
    /// </summary>
    private void OnDebugKeyDownChord(KeyRoutedEventArgs e)
    {
        var ctrl = Microsoft.UI.Input.InputKeyboardSource
            .GetKeyStateForCurrentThread(VirtualKey.Control)
            .HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
        var shift = Microsoft.UI.Input.InputKeyboardSource
            .GetKeyStateForCurrentThread(VirtualKey.Shift)
            .HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
        if (!ctrl || !shift) return;

        switch (e.Key)
        {
            case VirtualKey.Number1:
            case VirtualKey.NumberPad1:
                FireDebugPrompt(DebugPromptScenarios.Short);
                e.Handled = true;
                break;
            case VirtualKey.Number2:
            case VirtualKey.NumberPad2:
                FireDebugPrompt(DebugPromptScenarios.SourceChange);
                e.Handled = true;
                break;
            case VirtualKey.Number3:
            case VirtualKey.NumberPad3:
                FireDebugPrompt(DebugPromptScenarios.RetryOnFailure);
                e.Handled = true;
                break;
            case VirtualKey.Number0:
            case VirtualKey.NumberPad0:
                ViewModel.ClearPrompt();
                e.Handled = true;
                break;
            case VirtualKey.L:
                DebugLockEnabled = !DebugLockEnabled;
                Debug.WriteLine(
                    $"popover: debug lock {(DebugLockEnabled ? "ENABLED" : "disabled")}");
                e.Handled = true;
                break;
        }
    }

    private void FireDebugPrompt(PromptEvent canned)
    {
        // Cycle the prompt id so consecutive triggers look distinct
        // in the daemon log (matches how a real sequence of retries
        // would arrive with monotonically-increasing ids).
        _debugScenarioIndex++;
        var fresh = canned with { Id = (ulong)_debugScenarioIndex };
        ViewModel.ApplyIpcProgress(fresh);
    }
#endif
}
