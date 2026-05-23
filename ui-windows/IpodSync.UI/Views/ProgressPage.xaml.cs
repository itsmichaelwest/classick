using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI.Views;

/// <summary>
/// Renders the sync apply loop's progress: a determinate progress bar, the
/// current track label, a scrolling log tail, and a final-state InfoBar
/// when the sync finishes. The host (typically <c>MainPage</c> or <c>App</c>)
/// constructs the page, hands it a populated <see cref="ProgressViewModel"/>
/// (via the <c>ViewModel</c> property after navigation, or by passing it as
/// the navigation parameter and resolving in <c>OnNavigatedTo</c>), and
/// marshals each <c>ICoreProcess</c> event onto the UI thread via
/// <c>App.DispatcherQueue.TryEnqueue</c> before calling the VM's
/// <c>Apply*</c> methods.
/// </summary>
public sealed partial class ProgressPage : Page
{
    /// <summary>Backing VM exposed for x:Bind. M1 constructs a fresh VM on
    /// page creation; a future task will let a host inject one via navigation
    /// parameter so the same VM survives page transitions.</summary>
    public ProgressViewModel ViewModel { get; }

    public ProgressPage()
    {
        ViewModel = new ProgressViewModel();
        this.InitializeComponent();
    }
}
