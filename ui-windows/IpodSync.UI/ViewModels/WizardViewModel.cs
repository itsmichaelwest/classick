using System;
using System.Collections.ObjectModel;
using System.IO;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace IpodSync_UI.ViewModels;

public sealed record IpodIdentityCandidate(
    string Serial,
    string ModelLabel,
    string Drive,
    string? Name = null)
{
    /// <summary>Falls back through Name → "iPod" so the list never renders blank.</summary>
    public string DisplayName => string.IsNullOrWhiteSpace(Name) ? "iPod" : Name!;
}

public sealed record SaveConfigPayload(
    string Source,
    string IpodSerial,
    string IpodModelLabel,
    string? IpodName,
    string SubsequentSyncMode,
    uint ScheduleMinutes,
    bool AutostartWithWindows);

public partial class WizardViewModel : ObservableObject
{
    public const int TotalSteps = 5;

    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private IpodIdentityCandidate? selectedIpod;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";

    // Defaults mirror DaemonSettings defaults so a click-through wizard
    // produces the same config as a manual user would set up via Settings.
    [ObservableProperty] private bool isAutomatic = true;
    [ObservableProperty] private int scheduleMinutes = 30;
    [ObservableProperty] private bool autostartWithWindows = true;

    public ObservableCollection<IpodIdentityCandidate> Candidates { get; } = new();

    public WizardViewModel(Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _sendConfigFunc = sendConfigFunc;
    }

    partial void OnSourcePathChanged(string value)
    {
        OnPropertyChanged(nameof(IsSourcePathValid));
        NextCommand.NotifyCanExecuteChanged();
    }
    partial void OnSelectedIpodChanged(IpodIdentityCandidate? value) => NextCommand.NotifyCanExecuteChanged();
    partial void OnScanErrorChanged(string value) => OnPropertyChanged(nameof(HasScanError));
    partial void OnCurrentStepChanged(int value)
    {
        NextCommand.NotifyCanExecuteChanged();
        BackCommand.NotifyCanExecuteChanged();
        FinishCommand.NotifyCanExecuteChanged();
        OnPropertyChanged(nameof(IsWelcomeStep));
        OnPropertyChanged(nameof(IsFolderStep));
        OnPropertyChanged(nameof(IsDeviceStep));
        OnPropertyChanged(nameof(IsSyncSettingsStep));
        OnPropertyChanged(nameof(IsDoneStep));
        OnPropertyChanged(nameof(ShowNextButton));
        OnPropertyChanged(nameof(ShowFinishButton));
        OnPropertyChanged(nameof(CanGoBackToPrevious));
    }
    partial void OnIsAutomaticChanged(bool value) => OnPropertyChanged(nameof(IsManual));

    /// <summary>Inverse projection of <see cref="IsAutomatic"/> for the "Manual" radio.</summary>
    public bool IsManual
    {
        get => !IsAutomatic;
        set { if (value) IsAutomatic = false; }
    }

    public bool IsWelcomeStep      => CurrentStep == 1;
    public bool IsFolderStep       => CurrentStep == 2;
    public bool IsDeviceStep       => CurrentStep == 3;
    public bool IsSyncSettingsStep => CurrentStep == 4;
    public bool IsDoneStep         => CurrentStep == 5;

    public bool ShowNextButton    => CurrentStep < TotalSteps;
    public bool ShowFinishButton  => CurrentStep == TotalSteps;
    public bool CanGoBackToPrevious => CurrentStep > 1 && CurrentStep < TotalSteps;

    public bool HasScanError => !string.IsNullOrEmpty(ScanError);

    public bool IsSourcePathValid
    {
        get
        {
            if (string.IsNullOrWhiteSpace(SourcePath)) return false;
            try { return Directory.Exists(SourcePath); }
            catch { return false; }
        }
    }

    private bool CanGoNext()
    {
        return CurrentStep switch
        {
            1 => true,
            2 => IsSourcePathValid,
            3 => SelectedIpod is not null,
            4 => true,
            _ => false,
        };
    }

    [RelayCommand(CanExecute = nameof(CanGoNext))]
    private async Task NextAsync()
    {
        if (CurrentStep == 4)
        {
            // Save on the 4 → 5 transition; on failure the user stays put
            // with a visible error so they can adjust and retry without
            // losing context.
            try
            {
                await _sendConfigFunc(BuildPayload());
                ScanError = "";
                CurrentStep = 5;
            }
            catch (Exception e)
            {
                ScanError = $"Couldn't save settings: {e.Message}";
            }
        }
        else if (CurrentStep < TotalSteps)
        {
            CurrentStep++;
        }
    }

    [RelayCommand(CanExecute = nameof(CanGoBack))]
    private void Back()
    {
        if (CanGoBack()) CurrentStep--;
    }

    private bool CanGoBack() => CurrentStep > 1 && CurrentStep < TotalSteps;

    private bool CanFinish() => CurrentStep == TotalSteps;

    [RelayCommand(CanExecute = nameof(CanFinish))]
    private void Finish()
    {
        // Save already happened on the 4 → 5 transition; Finish just dismisses.
        WizardFinished?.Invoke();
    }

    /// <summary>
    /// Adds a newly-detected iPod, deduping by serial. If a candidate with
    /// the same serial already exists, replaces it in place — this is how
    /// the daemon's two-phase DeviceConnected broadcast (initial → re-fire
    /// with name from iTunesDB) updates the row's <see cref="IpodIdentityCandidate.Name"/>
    /// so the eventual save_config carries the friendly name, not just the
    /// model label. Selection is never set automatically; the user picks.
    /// </summary>
    public void OnDeviceConnected(IpodIdentityCandidate candidate)
    {
        for (int i = 0; i < Candidates.Count; i++)
        {
            if (Candidates[i].Serial != candidate.Serial) continue;
            if (Candidates[i] == candidate) return;
            var wasSelected = ReferenceEquals(SelectedIpod, Candidates[i]);
            Candidates[i] = candidate;
            if (wasSelected) SelectedIpod = candidate;
            return;
        }
        Candidates.Add(candidate);
        ScanError = "";
    }

    public void OnDeviceDisconnected(string serial)
    {
        for (int i = Candidates.Count - 1; i >= 0; i--)
        {
            if (Candidates[i].Serial == serial)
            {
                if (ReferenceEquals(SelectedIpod, Candidates[i])) SelectedIpod = null;
                Candidates.RemoveAt(i);
            }
        }
    }

    public void BeginScanning() => Scanning = true;
    public void EndScanning() => Scanning = false;

    [RelayCommand]
    private void ClearCandidates()
    {
        Candidates.Clear();
        SelectedIpod = null;
        ScanError = "";
        Scanning = true;
    }

    private SaveConfigPayload BuildPayload()
    {
        var ipod = SelectedIpod!;
        return new SaveConfigPayload(
            Source: SourcePath,
            IpodSerial: ipod.Serial,
            IpodModelLabel: ipod.ModelLabel,
            IpodName: ipod.Name,
            SubsequentSyncMode: IsAutomatic ? "auto_apply" : "review",
            ScheduleMinutes: (uint)ScheduleMinutes,
            AutostartWithWindows: AutostartWithWindows);
    }

    public event Action? WizardFinished;
}
