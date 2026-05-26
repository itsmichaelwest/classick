using Classick_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace Classick_UI.Views;

public sealed partial class WizardFolderPage : Page
{
    public WizardFolderPage() => InitializeComponent();

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        DataContext = e.Parameter as WizardViewModel;
    }

    private async void OnBrowseClick(object sender, RoutedEventArgs e)
    {
        if (DataContext is not WizardViewModel vm) return;
        var picker = new FolderPicker();
        picker.FileTypeFilter.Add("*");
        // COM picker needs an owning HWND on WinUI 3; App.WindowHandle is set
        // when the wizard window opens.
        InitializeWithWindow.Initialize(picker, App.WindowHandle);
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null) vm.SourcePath = folder.Path;
    }
}
