using Classick_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Classick_UI.Views;

public sealed partial class SettingsNotificationsPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }

    public SettingsNotificationsPage() { InitializeComponent(); }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
        Bindings.Update();
    }
}
