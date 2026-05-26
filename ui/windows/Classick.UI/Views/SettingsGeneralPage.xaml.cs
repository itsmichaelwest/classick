using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using Classick_UI.Core;
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
        get => ViewModel?.General.SubsequentSyncMode == "auto_apply";
        set
        {
            if (value && ViewModel is not null)
            {
                ViewModel.General.SubsequentSyncMode = "auto_apply";
                ViewModel.General.FirstSyncMode = "auto_apply";
                OnPropertyChangedSelf();
            }
        }
    }

    public bool IsManual
    {
        get => ViewModel?.General.SubsequentSyncMode == "review";
        set
        {
            if (value && ViewModel is not null)
            {
                ViewModel.General.SubsequentSyncMode = "review";
                ViewModel.General.FirstSyncMode = "review";
                OnPropertyChangedSelf();
            }
        }
    }

    public new event PropertyChangedEventHandler? PropertyChanged;
    private void OnPropertyChangedSelf()
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsAutomatic)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsManual)));
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
            ViewModel.Chooser.Remove(current);
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
}
