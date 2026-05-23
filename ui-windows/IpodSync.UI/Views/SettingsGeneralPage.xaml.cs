using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class SettingsGeneralPage : Page
{
    public SettingsViewModel? ViewModel { get; private set; }
    public SettingsGeneralPage() { InitializeComponent(); }
    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        ViewModel = e.Parameter as SettingsViewModel;
    }
}
