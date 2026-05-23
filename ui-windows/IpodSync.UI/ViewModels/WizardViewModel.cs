using System;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// One iPod candidate identified by a drive-letter scan (or, in M3, a daemon
/// device event). The triple of <see cref="Serial"/>, <see cref="ModelLabel"/>,
/// and <see cref="Drive"/> uniquely identifies a connected device for the
/// purposes of the first-launch wizard.
/// </summary>
public sealed record IpodIdentityCandidate(string Serial, string ModelLabel, string Drive);

/// <summary>
/// Payload handed off when the user clicks Finish on Step 3 of the wizard.
/// The host (<c>WizardWindow</c> code-behind) maps this to a
/// <c>SaveConfigCommand</c> on the daemon channel.
/// </summary>
public sealed record SaveConfigPayload(string Source, string IpodSerial, string IpodModelLabel);

/// <summary>
/// Backs the 3-step first-launch wizard:
/// <list type="number">
///   <item><description>Step 1: pick a music source folder.</description></item>
///   <item><description>Step 2: identify the connected iPod (via injected scan func).</description></item>
///   <item><description>Step 3: confirm and Finish (raises <see cref="WizardFinished"/>).</description></item>
/// </list>
///
/// <para>
/// The VM stays pure and unit-testable: drive scanning is supplied as
/// <c>scanFunc</c> and the daemon save-config call is supplied as
/// <c>sendConfigFunc</c>. Tests provide in-memory fakes; the production
/// code-behind wires the real local-drive poll + <c>DaemonClient.SendAsync</c>.
/// </para>
///
/// <para>
/// Step gating: <see cref="NextCommand"/> requires a non-empty
/// <see cref="SourcePath"/> on Step 1 and a non-null
/// <see cref="DetectedIpod"/> on Step 2. <see cref="FinishCommand"/> requires
/// Step 3 with both fields populated.
/// </para>
/// </summary>
public partial class WizardViewModel : ObservableObject
{
    private readonly Func<IpodIdentityCandidate?> _scanFunc;
    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private IpodIdentityCandidate? detectedIpod;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";

    public WizardViewModel(
        Func<IpodIdentityCandidate?> scanFunc,
        Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _scanFunc = scanFunc;
        _sendConfigFunc = sendConfigFunc;
    }

    partial void OnSourcePathChanged(string value) => NextCommand.NotifyCanExecuteChanged();
    partial void OnDetectedIpodChanged(IpodIdentityCandidate? value) => NextCommand.NotifyCanExecuteChanged();
    partial void OnCurrentStepChanged(int value)
    {
        NextCommand.NotifyCanExecuteChanged();
        BackCommand.NotifyCanExecuteChanged();
        FinishCommand.NotifyCanExecuteChanged();
    }

    private bool CanGoNext()
    {
        return CurrentStep switch
        {
            1 => !string.IsNullOrWhiteSpace(SourcePath),
            2 => DetectedIpod is not null,
            _ => false,
        };
    }

    [RelayCommand(CanExecute = nameof(CanGoNext))]
    private void Next()
    {
        if (CurrentStep == 1)
        {
            CurrentStep = 2;
            TriggerScan();
        }
        else if (CurrentStep == 2)
        {
            CurrentStep = 3;
        }
    }

    [RelayCommand(CanExecute = nameof(CanGoBack))]
    private void Back()
    {
        if (CurrentStep > 1) CurrentStep--;
    }

    private bool CanGoBack() => CurrentStep > 1;

    [RelayCommand]
    private void TriggerScan()
    {
        Scanning = true;
        ScanError = "";
        try
        {
            DetectedIpod = _scanFunc();
            if (DetectedIpod is null)
            {
                ScanError = "No iPod detected. Plug in your iPod and click Retry.";
            }
        }
        catch (Exception e)
        {
            ScanError = $"Scan failed: {e.Message}";
        }
        finally { Scanning = false; }
    }

    private bool CanFinish() => CurrentStep == 3 && DetectedIpod is not null && !string.IsNullOrWhiteSpace(SourcePath);

    [RelayCommand(CanExecute = nameof(CanFinish))]
    private async Task FinishAsync()
    {
        var payload = new SaveConfigPayload(
            Source: SourcePath,
            IpodSerial: DetectedIpod!.Serial,
            IpodModelLabel: DetectedIpod.ModelLabel);
        await _sendConfigFunc(payload);
        WizardFinished?.Invoke();
    }

    /// <summary>
    /// Raised after Finish completes successfully. The host typically responds
    /// by closing the wizard window.
    /// </summary>
    public event Action? WizardFinished;
}
