using Classick_UI.ViewModels;
using Microsoft.UI.Xaml.Controls;

namespace Classick_UI.Views;

/// <summary>
/// Renders an action plan and gathers the user's Apply / DryRun / Quit
/// decision. The host (typically <c>MainPage</c> or <c>App</c>) constructs
/// the page, hands it a populated <see cref="ReviewViewModel"/> (via the
/// <c>ViewModel</c> property after navigation, or by passing it as the
/// navigation parameter and resolving in <c>OnNavigatedTo</c>), and
/// subscribes to <see cref="ReviewViewModel.DecisionMade"/> to forward the
/// decision over IPC.
/// </summary>
public sealed partial class ReviewPage : Page
{
    /// <summary>Backing VM exposed for x:Bind. M1 constructs a fresh VM on
    /// page creation; a future task will let a host inject one via navigation
    /// parameter so the same VM survives page transitions.</summary>
    public ReviewViewModel ViewModel { get; }

    public ReviewPage()
    {
        ViewModel = new ReviewViewModel();
        this.InitializeComponent();
    }
}
