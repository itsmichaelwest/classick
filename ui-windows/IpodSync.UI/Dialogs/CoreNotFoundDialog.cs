using System.Threading.Tasks;
using IpodSync_UI.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace IpodSync_UI.Dialogs;

public static class CoreNotFoundDialog
{
    /// <summary>
    /// Show a modal explaining ipod-sync.exe wasn't found and listing where
    /// the UI looked. Resolves when the user dismisses.
    /// </summary>
    public static async Task ShowAsync(XamlRoot xamlRoot, CoreNotFoundException ex)
    {
        var dialog = new ContentDialog
        {
            Title = "Can't find ipod-sync.exe",
            Content = new ScrollViewer
            {
                Content = new TextBlock
                {
                    Text = ex.Message,
                    TextWrapping = TextWrapping.Wrap,
                    FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas"),
                },
                MaxHeight = 400,
            },
            CloseButtonText = "OK",
            DefaultButton = ContentDialogButton.Close,
            XamlRoot = xamlRoot,
        };
        await dialog.ShowAsync();
    }
}
