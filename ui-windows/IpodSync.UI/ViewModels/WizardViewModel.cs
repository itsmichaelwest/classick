using System;
using System.Threading;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace IpodSync_UI.ViewModels;

/// <summary>
/// One iPod candidate identified by a daemon DeviceConnected event.
/// </summary>
public sealed record IpodIdentityCandidate(string Serial, string ModelLabel, string Drive);

/// <summary>
/// Payload handed off when the user clicks Finish on Step 3 of the wizard.
/// </summary>
public sealed record SaveConfigPayload(string Source, string IpodSerial, string IpodModelLabel);

/// <summary>
/// Backs the 3-step first-launch wizard:
/// <list type="number">
///   <item><description>Step 1: pick a music source folder.</description></item>
///   <item><description>Step 2: wait for daemon DeviceConnected event identifying the iPod.</description></item>
///   <item><description>Step 3: confirm and Finish.</description></item>
/// </list>
///
/// <para>
/// Pure / unit-testable: device wait + daemon save-config call are
/// supplied as func args. Tests pass <c>TaskCompletionSource</c>-backed
/// fakes; production code-behind wires the wait to
/// <c>DaemonClient.SubscribeDeviceEvents + event channel filter</c>.
/// </para>
/// </summary>
public partial class WizardViewModel : ObservableObject
{
    private readonly Func<CancellationToken, Task<IpodIdentityCandidate?>> _waitForDeviceFunc;
    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;
    private CancellationTokenSource? _waitCts;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private IpodIdentityCandidate? detectedIpod;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";

    public WizardViewModel(
        Func<CancellationToken, Task<IpodIdentityCandidate?>> waitForDeviceFunc,
        Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _waitForDeviceFunc = waitForDeviceFunc;
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
            _ = TriggerScanAsync();
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

    /// <summary>Wired to the Retry button.</summary>
    [RelayCommand]
    private void TriggerScan() => _ = TriggerScanAsync();

    private async Task TriggerScanAsync()
    {
        _waitCts?.Cancel();
        _waitCts = new CancellationTokenSource();
        Scanning = true;
        ScanError = "";
        DetectedIpod = null;
        try
        {
            var detected = await _waitForDeviceFunc(_waitCts.Token);
            DetectedIpod = detected;
            if (detected is null)
            {
                ScanError = "No iPod detected. Plug in your iPod and click Retry.";
            }
        }
        catch (OperationCanceledException) { /* user navigated back or closed wizard */ }
        catch (Exception e) { ScanError = $"Scan failed: {e.Message}"; }
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

    /// <summary>Cancels any in-flight device wait. Called from WizardWindow.Closed.</summary>
    public void CancelWait() => _waitCts?.Cancel();

    public event Action? WizardFinished;
}
