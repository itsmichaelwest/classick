using System;
using System.Diagnostics;
using System.IO;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsAboutPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsAboutPage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }

    private void OnShowLogFolder(object sender, RoutedEventArgs e)
    {
        var path = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "ipod-sync", "logs");
        Directory.CreateDirectory(path);
        try { Process.Start(new ProcessStartInfo("explorer.exe", $"\"{path}\"") { UseShellExecute = true }); }
        catch (Exception ex) { Debug.WriteLine($"about: open log folder failed: {ex.Message}"); }
    }
}
