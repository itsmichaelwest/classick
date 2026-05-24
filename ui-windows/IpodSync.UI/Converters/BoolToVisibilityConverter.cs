using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Data;

namespace IpodSync_UI.Converters;

public sealed class BoolToVisibilityConverter : IValueConverter
{
    public object Convert(object value, System.Type targetType, object parameter, string language)
        => (value is bool b && b) ? Visibility.Visible : Visibility.Collapsed;

    public object ConvertBack(object value, System.Type targetType, object parameter, string language)
        => value is Visibility v && v == Visibility.Visible;
}
