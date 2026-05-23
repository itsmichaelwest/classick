using System;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace IpodSync_UI.Views;

/// <summary>
/// M2 first-launch wizard. Hosts a <see cref="WizardViewModel"/> wired with:
/// <list type="bullet">
///   <item><description>A local-drive scan func that mirrors <c>src/ipod/device.rs::scan_for_ipod</c>
///     on the C# side. M2 uses in-process polling; M3 will replace this with
///     daemon-emitted device events.</description></item>
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
            scanFunc: ScanForIpodViaDaemon,
            sendConfigFunc: SaveConfigViaDaemon);
        ViewModel.WizardFinished += OnWizardFinished;
        this.InitializeComponent();
    }

    private IpodIdentityCandidate? ScanForIpodViaDaemon()
    {
        // M2: synchronous polling via the daemon. The wizard sends a
        // SubscribeDeviceEvents command (M3 wires actual events) then
        // immediately uses the M2 polling fallback through SaveConfig's
        // implicit detection. For M2 simplicity, fall back to scanning
        // drive letters in-process.
        // M3 will replace this with daemon-emitted device events.
        return ScanLocalDrives();
    }

    private static IpodIdentityCandidate? ScanLocalDrives()
    {
        for (char letter = 'A'; letter <= 'Z'; letter++)
        {
            var drive = $"{letter}:\\";
            if (!System.IO.Directory.Exists(drive)) continue;
            var sysInfo = System.IO.Path.Combine(drive, "iPod_Control", "Device", "SysInfo");
            if (!System.IO.File.Exists(sysInfo)) continue;
            try
            {
                var text = System.IO.File.ReadAllText(sysInfo);
                var serial = ParseField(text, "FirewireGuid");
                if (serial is null) continue;
                var model = ParseField(text, "ModelNumStr") ?? "";
                var label = DescribeModel(model);
                return new IpodIdentityCandidate(serial, label, drive);
            }
            catch { /* skip */ }
        }
        return null;
    }

    private static string? ParseField(string text, string key)
    {
        foreach (var line in text.Split('\n'))
        {
            var trimmed = line.Trim();
            if (trimmed.StartsWith(key, StringComparison.OrdinalIgnoreCase))
            {
                var rest = trimmed.Substring(key.Length).TrimStart(':', ' ').Trim();
                if (!string.IsNullOrEmpty(rest)) return rest;
            }
        }
        return null;
    }

    private static string DescribeModel(string modelNum)
    {
        var upper = modelNum.TrimStart('x').ToUpperInvariant();
        return upper switch
        {
            "MB029" or "MB147" or "MB565" => $"iPod Classic 7G ({upper})",
            _ when !string.IsNullOrEmpty(upper) => $"iPod ({upper})",
            _ => "iPod (model unknown)",
        };
    }

    private async Task SaveConfigViaDaemon(SaveConfigPayload payload)
    {
        if (App.Daemon is null) return;
        await App.Daemon.SendAsync(new SaveConfigCommand(
            Source: payload.Source,
            Ipod: new IpodIdentity(payload.IpodSerial, payload.IpodModelLabel)));
    }

    private void OnWizardFinished() => this.Close();

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
