using Microsoft.UI.Xaml;

namespace IpodSync_UI.Views;

/// <summary>
/// M2 setup wizard host window. Scaffold only — T12 fills in the
/// 3-step pages (welcome / iPod identity / daemon settings) and
/// binds a <c>WizardViewModel</c>.
/// </summary>
public sealed partial class WizardWindow : Window
{
    public WizardWindow()
    {
        this.InitializeComponent();
    }
}
