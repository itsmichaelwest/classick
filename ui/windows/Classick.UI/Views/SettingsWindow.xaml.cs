using System;
using Classick_UI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using WinRT.Interop;

namespace Classick_UI.Views;

/// <summary>
/// Settings shell — 640x740 fixed footprint, Mica backdrop, anchored
/// 12 DIP from the work-area's bottom-right corner (mirrors PopoverWindow
/// so opening Settings from the tray feels continuous with the popover).
///
/// Sizing/anchoring lives in WindowAnchor so the popover and settings
/// share identical math for client-area sizing + shadow-corrected edge
/// gap. NavView pane is locked open; settings auto-save on change via
/// the VM's debounced SaveAsync — there is no Save/Cancel footer.
/// </summary>
public sealed partial class SettingsWindow : Window
{
    // Width is 760 (not the Figma's 640) because the CommunityToolkit
    // SettingsCard switches to a wrapped layout (action below header)
    // when its width drops below SettingsCardWrapThreshold (476 DIP).
    // With 200 DIP nav + 48 DIP page padding, the card sees 760-248 =
    // 512 DIP, comfortably above the threshold — actions stay on the
    // right rail per Figma. Height stays at the Figma-spec 740.
    private const int FixedWidthDip = 760;
    private const int FixedHeightDip = 740;
    private const int EdgeGapDip = 12;

    public SettingsViewModel ViewModel { get; }

    public SettingsWindow(SettingsViewModel vm)
    {
        ViewModel = vm;
        InitializeComponent();

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(hwnd));

        // Non-resizable, no min/max — fixed footprint per design. Keep
        // the standard border + caption controls (close button) because
        // this is a normal app window (not a tray flyout).
        if (appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
        }

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(SettingsTitleBar);
        Title = "classick Settings";

        // Mica per design — the main app window gets Mica; the popover
        // is the flyout that gets Acrylic (Fluent material guidance).
        SystemBackdrop = new MicaBackdrop();

        WindowAnchor.DisableTransitions(hwnd);
        WindowAnchor.SizeClientArea(hwnd, appWindow, FixedWidthDip, FixedHeightDip);
        WindowAnchor.AnchorBottomRight(hwnd, appWindow, EdgeGapDip);

        // First activation: DWM frame bounds aren't reliable pre-activation,
        // so re-anchor once we're on screen to settle into the correct
        // shadow-corrected position.
        Activated += OnFirstActivated;

        Nav.SelectedItem = Nav.MenuItems[0];
        RebuildChooserFlyout();
        ViewModel.Chooser.Changed += RebuildChooserFlyout;
    }

    private bool _anchored;
    private void OnFirstActivated(object sender, WindowActivatedEventArgs args)
    {
        if (_anchored) return;
        _anchored = true;
        Activated -= OnFirstActivated;
        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(hwnd));
        WindowAnchor.AnchorBottomRight(hwnd, appWindow, EdgeGapDip);
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem item) return;
        var tag = item.Tag as string;
        Type? pageType = tag switch
        {
            "general"       => typeof(SettingsGeneralPage),
            "notifications" => typeof(SettingsNotificationsPage),
            "history"       => typeof(SettingsHistoryPage),
            _               => null,
        };
        if (pageType is null) return;
        ContentFrame.Navigate(pageType, ViewModel);
    }

    /// <summary>
    /// Rebuild the chooser's MenuFlyout from the current Chooser items.
    /// Each iPod gets a row with a kebab submenu (Rename, Remove), and
    /// a trailing "Add new…" item lets the user start the pair wizard
    /// for a fresh iPod. Re-runs whenever the VM raises Changed.
    /// </summary>
    private void RebuildChooserFlyout()
    {
        IpodChooserFlyout.Items.Clear();
        foreach (var item in ViewModel.Chooser.Items)
        {
            var row = new MenuFlyoutSubItem { Text = item.DisplayName };
            var select = new MenuFlyoutItem { Text = "Select" };
            select.Click += (_, _) => ViewModel.Chooser.Select(item);
            var rename = new MenuFlyoutItem { Text = "Rename…" };
            rename.Click += async (_, _) => await PromptRenameAsync(item);
            var remove = new MenuFlyoutItem { Text = "Remove" };
            remove.Click += async (_, _) => await ConfirmRemoveAsync(item);
            row.Items.Add(select);
            row.Items.Add(new MenuFlyoutSeparator());
            row.Items.Add(rename);
            row.Items.Add(remove);
            IpodChooserFlyout.Items.Add(row);
        }
        if (ViewModel.Chooser.Items.Count > 0)
        {
            IpodChooserFlyout.Items.Add(new MenuFlyoutSeparator());
        }
        var addNew = new MenuFlyoutItem
        {
            Text = "Add new…",
            Icon = new FontIcon { Glyph = "" },
        };
        addNew.Click += (_, _) => App.RequestPairNewIpod();
        IpodChooserFlyout.Items.Add(addNew);
    }

    private async System.Threading.Tasks.Task PromptRenameAsync(IpodChooserItemViewModel item)
    {
        var input = new TextBox { Text = item.DisplayName, SelectionLength = item.DisplayName.Length };
        var dialog = new ContentDialog
        {
            Title = "Rename iPod",
            Content = input,
            PrimaryButtonText = "Rename",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = SettingsRoot.XamlRoot,
        };
        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary && !string.IsNullOrWhiteSpace(input.Text))
        {
            ViewModel.Chooser.Rename(item, input.Text.Trim());
        }
    }

    private async System.Threading.Tasks.Task ConfirmRemoveAsync(IpodChooserItemViewModel item)
    {
        var dialog = new ContentDialog
        {
            Title = $"Remove {item.DisplayName}?",
            Content = "classick will forget this iPod's pairing. You can pair it again from the wizard.",
            PrimaryButtonText = "Remove",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = SettingsRoot.XamlRoot,
        };
        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary)
        {
            ViewModel.Chooser.Remove(item);
        }
    }
}
