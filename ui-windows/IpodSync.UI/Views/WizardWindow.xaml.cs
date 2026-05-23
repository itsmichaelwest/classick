using System;
using System.Threading;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace IpodSync_UI.Views;

/// <summary>
/// M3 first-launch wizard. Hosts a <see cref="WizardViewModel"/> wired with:
/// <list type="bullet">
///   <item><description>A device-wait func that subscribes to daemon
///     <see cref="SubscribeDeviceEventsCommand"/> and awaits the next
///     <see cref="DeviceConnectedEvent"/> from <see cref="DaemonClient.Events"/>.</description></item>
///   <item><description>A save-config func that sends a <see cref="SaveConfigCommand"/>
///     through the persistent <c>App.Daemon</c> client.</description></item>
/// </list>
///
/// <para>
/// File picker initialization uses <see cref="App.WindowHandle"/> via
/// <c>InitializeWithWindow</c>, the standard WinUI 3 pattern for COM-based
/// pickers that need an HWND to parent against.
/// </para>
/// </summary>
public sealed partial class WizardWindow : Window
{
    public WizardViewModel ViewModel { get; }

    public WizardWindow()
    {
        ViewModel = new WizardViewModel(
            waitForDeviceFunc: WaitForDeviceFromDaemonAsync,
            sendConfigFunc: SendSaveConfigAsync);
        // Note: WinUI 3 Window does not have a DataContext property.
        // The XAML uses x:Bind ViewModel.* which references the
        // public ViewModel property on this code-behind directly.
        ViewModel.WizardFinished += () => DispatcherQueue.TryEnqueue(Close);
        this.Closed += (_, _) => ViewModel.CancelWait();
        this.InitializeComponent();
    }

    private async Task<IpodIdentityCandidate?> WaitForDeviceFromDaemonAsync(CancellationToken ct)
    {
        var daemon = App.Daemon;
        if (daemon is null) return null;

        await daemon.SendAsync(new SubscribeDeviceEventsCommand(), ct);
        try
        {
            while (!ct.IsCancellationRequested)
            {
                var evt = await daemon.Events.ReadAsync(ct);
                if (evt is DeviceConnectedEvent dc)
                {
                    return new IpodIdentityCandidate(dc.Serial, dc.ModelLabel, dc.Drive);
                }
                // Other event types are ignored here — App.xaml.cs may also
                // be reading from the same channel; both must consume one
                // event per loop. M4 introduces a proper event router; for
                // M3 the wizard owns the channel exclusively while open.
            }
            return null;
        }
        finally
        {
            try { await daemon.SendAsync(new UnsubscribeDeviceEventsCommand()); } catch { }
        }
    }

    private async Task SendSaveConfigAsync(SaveConfigPayload payload)
    {
        var daemon = App.Daemon;
        if (daemon is null) return;
        await daemon.SendAsync(new SaveConfigCommand(
            Source: payload.Source,
            Ipod: new IpodIdentity(payload.IpodSerial, payload.IpodModelLabel)));
    }

    private async void OnBrowseClick(object sender, RoutedEventArgs e)
    {
        var picker = new FolderPicker();
        picker.FileTypeFilter.Add("*");
        InitializeWithWindow.Initialize(picker, App.WindowHandle);
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null) ViewModel.SourcePath = folder.Path;
    }

    private void OnRetryScan(object sender, RoutedEventArgs e) => ViewModel.TriggerScanCommand.Execute(null);

    private void OnCancelClick(object sender, RoutedEventArgs e) => this.Close();

    // x:Bind helper accessors
    public Visibility IsStep(int n, int current) => n == current ? Visibility.Visible : Visibility.Collapsed;
    public Visibility NotStep(int n, int current) => n == current ? Visibility.Collapsed : Visibility.Visible;
    public Brush StepDotFill(int n, int current)
        => new SolidColorBrush(n <= current
            ? Microsoft.UI.Colors.SteelBlue
            : Microsoft.UI.Colors.LightGray);
    public Visibility HasDetection(IpodIdentityCandidate? ipod)
        => ipod is null ? Visibility.Collapsed : Visibility.Visible;
    public string FormatSerial(IpodIdentityCandidate? ipod)
        => ipod is null ? "" : $"Serial: {ipod.Serial}";
    public string FormatIpodSummary(IpodIdentityCandidate? ipod)
        => ipod is null ? "(none)" : $"{ipod.ModelLabel} · {ipod.Serial}";
}
