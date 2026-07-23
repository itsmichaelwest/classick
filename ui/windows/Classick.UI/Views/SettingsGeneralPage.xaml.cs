using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using Classick_UI.Core;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Windows.Storage.Pickers;

namespace Classick_UI.Views;

public sealed partial class SettingsGeneralPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }

    public SettingsGeneralPage()
    {
        InitializeComponent();
    }

    // Two-way bridges between SubsequentSyncMode ("auto_apply"/"review")
    // and the RadioButton IsChecked booleans. RadioButtons can't bind to
    // an enum directly without a converter, so we expose two computed
    // boolean properties on the page itself.
    public bool IsAutomatic
    {
        get => ViewModel?.General.SubsequentSyncMode == SyncMode.AutoApply;
        set
        {
            if (value && ViewModel is not null)
            {
                ViewModel.General.SubsequentSyncMode = SyncMode.AutoApply;
                ViewModel.General.FirstSyncMode = SyncMode.AutoApply;
                OnPropertyChangedSelf();
            }
        }
    }

    public bool IsManual
    {
        get => ViewModel?.General.SubsequentSyncMode == SyncMode.Review;
        set
        {
            if (value && ViewModel is not null)
            {
                ViewModel.General.SubsequentSyncMode = SyncMode.Review;
                ViewModel.General.FirstSyncMode = SyncMode.Review;
                OnPropertyChangedSelf();
            }
        }
    }

    public bool IsSelectionAll
    {
        get => ViewModel?.General.DeviceSelectionMode == SelectionMode.All;
        set { if (value && ViewModel is not null) SetSelectionMode(SelectionMode.All); }
    }

    public bool IsSelectionInclude
    {
        get => ViewModel?.General.DeviceSelectionMode == SelectionMode.Include;
        set { if (value && ViewModel is not null) SetSelectionMode(SelectionMode.Include); }
    }

    public bool IsSelectionExclude
    {
        get => ViewModel?.General.DeviceSelectionMode == SelectionMode.Exclude;
        set { if (value && ViewModel is not null) SetSelectionMode(SelectionMode.Exclude); }
    }

    private void SetSelectionMode(SelectionMode mode)
    {
        ViewModel!.General.DeviceSelectionMode = mode;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionAll)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionInclude)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionExclude)));
    }

    public new event PropertyChangedEventHandler? PropertyChanged;
    private void OnPropertyChangedSelf()
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsAutomatic)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsManual)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionAll)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionInclude)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsSelectionExclude)));
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
        OnPropertyChangedSelf();
    }

    private async void OnPickSource(object sender, RoutedEventArgs e)
    {
        if (ViewModel is null) return;
        var picker = new FolderPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, App.SettingsWindowHandle);
        picker.FileTypeFilter.Add("*");
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null) ViewModel.General.SourcePath = folder.Path;
    }

    private async void OnRemoveIpod(object sender, RoutedEventArgs e)
    {
        if (ViewModel?.Chooser.Selected is not { } current) return;
        var dialog = new ContentDialog
        {
            Title = $"Remove {current.DisplayName}?",
            Content = "classick will forget this iPod's pairing. You'll be guided through the wizard to pair an iPod again.",
            PrimaryButtonText = "Remove",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = XamlRoot,
        };
        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary)
        {
            try { await ViewModel.ForgetSelectedAsync(); }
            catch (Exception exception) { Debug.WriteLine($"settings: forget device failed: {exception.Message}"); }
        }
    }

    private void OnShowLogFolder(object sender, RoutedEventArgs e)
    {
        var path = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            AppIdentity.Name, "logs");
        Directory.CreateDirectory(path);
        try { Process.Start(new ProcessStartInfo("explorer.exe", $"\"{path}\"") { UseShellExecute = true }); }
        catch (Exception ex) { Debug.WriteLine($"settings: open log folder failed: {ex.Message}"); }
    }

    private async void OnSyncNow(object sender, RoutedEventArgs e)
    {
        if (ViewModel is null) return;
        try { await ViewModel.SyncSelectedAsync(); }
        catch (Exception exception) { Debug.WriteLine($"settings: sync failed: {exception.Message}"); }
    }

    private async void OnReplaceLibrary(object sender, RoutedEventArgs e)
    {
        if (ViewModel is null) return;
        var dialog = new ContentDialog
        {
            Title = "Replace this iPod's library?",
            Content = "Classick will explicitly replace the selected iPod's music from the current library.",
            PrimaryButtonText = "Replace",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        try { await ViewModel.ReplaceSelectedLibraryAsync(); }
        catch (Exception exception) { Debug.WriteLine($"settings: replace failed: {exception.Message}"); }
    }
}
