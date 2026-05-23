using System;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace IpodSync_UI;

/// <summary>
/// Landing page for the app. Hosts the Start / Quit buttons. On Start, hands
/// control to an <see cref="AppController"/> that spawns the Rust core and
/// navigates the host <see cref="Frame"/> through Review / Progress pages as
/// IPC events arrive.
/// </summary>
public sealed partial class MainPage : Page
{
    private AppController? _controller;

    public MainPage()
    {
        InitializeComponent();
    }

    private async void StartButton_Click(object sender, RoutedEventArgs e)
    {
        StartButton.IsEnabled = false;
        try
        {
            // `Page.Frame` is the canonical host-frame property in WinUI (set
            // when the page is navigated to via Frame.Navigate). It's the
            // RootFrame from MainWindow.xaml.
            var frame = this.Frame
                ?? throw new InvalidOperationException("MainPage has no host Frame.");

            _controller = new AppController(frame, App.DispatcherQueue, this.XamlRoot);
            var ok = await _controller.StartAsync();
            if (!ok)
            {
                // User-visible error already shown; reset to allow retry.
                await _controller.DisposeAsync();
                _controller = null;
                StartButton.IsEnabled = true;
            }
            // If ok, the controller takes over and navigates away from MainPage.
        }
        catch (Exception ex)
        {
            StartButton.IsEnabled = true;
            var dialog = new ContentDialog
            {
                Title = "Couldn't start",
                Content = new TextBlock { Text = ex.Message, TextWrapping = TextWrapping.Wrap },
                CloseButtonText = "OK",
                XamlRoot = this.XamlRoot,
            };
            await dialog.ShowAsync();
        }
    }

    private void QuitButton_Click(object sender, RoutedEventArgs e)
    {
        App.Window?.Close();
    }
}
