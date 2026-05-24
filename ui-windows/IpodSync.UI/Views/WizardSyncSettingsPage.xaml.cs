using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace IpodSync_UI.Views;

public sealed partial class WizardSyncSettingsPage : Page
{
    public WizardSyncSettingsPage() => InitializeComponent();

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        DataContext = e.Parameter as WizardViewModel;
    }
}
