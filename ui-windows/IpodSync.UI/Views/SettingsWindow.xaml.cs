using System;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI.Views;

public sealed partial class SettingsWindow : Window
{
    public SettingsViewModel ViewModel { get; }

    public SettingsWindow(SettingsViewModel vm)
    {
        ViewModel = vm;
        InitializeComponent();
        Title = "ipod-sync settings";
        // Default to General tab.
        Nav.SelectedItem = Nav.MenuItems[0];
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem item) return;
        var tag = item.Tag as string;
        Type? pageType = tag switch
        {
            "general"  => typeof(SettingsGeneralPage),
            "schedule" => typeof(SettingsSchedulePage),
            "history"  => typeof(SettingsHistoryPage),
            "about"    => typeof(SettingsAboutPage),
            _          => null,
        };
        if (pageType is null) return;
        ContentFrame.Navigate(pageType, ViewModel);
    }

    private async void OnSave(object sender, RoutedEventArgs e)
    {
        await ViewModel.SaveAsync();
        Close();
    }

    private void OnCancel(object sender, RoutedEventArgs e) => Close();
}
