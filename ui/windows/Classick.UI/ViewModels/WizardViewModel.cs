using System.Collections.ObjectModel;
using System.IO;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Classick_UI.ViewModels;

public sealed record WizardDeviceCandidate(
    DeviceId? DeviceId,
    ulong? ObservationId,
    DevicePresentation Presentation)
{
    public string DisplayName => Presentation.Title;
    public string HardwareSummary => Presentation.HardwareSummary;
    public string HardwareProvenance => Presentation.HardwareProvenance;
    public string Status => Presentation.Status;
    public string Guidance => Presentation.Guidance;
    public bool CanAdopt => DeviceId is not null && Presentation.CanAdopt;
    public string ArtworkUri => Presentation.Artwork.AssetUri;
    public string ArtworkDescription => Presentation.Artwork.AccessibleDescription;

    public static WizardDeviceCandidate From(IdentifiedDeviceSnapshot device) =>
        new(device.DeviceId, null, DevicePresentationFactory.For(device));

    public static WizardDeviceCandidate From(UnidentifiedDeviceSnapshot device) =>
        new(null, device.ObservationId, DevicePresentationFactory.For(device));
}

public sealed record SaveConfigPayload(
    string Source,
    DeviceId DeviceId,
    bool AutoSync);

public partial class WizardViewModel : ObservableObject
{
    public const int TotalSteps = 5;

    private readonly Func<SaveConfigPayload, Task> _sendConfigFunc;

    [ObservableProperty] private int currentStep = 1;
    [ObservableProperty] private string sourcePath = "";
    [ObservableProperty] private WizardDeviceCandidate? selectedDevice;
    [ObservableProperty] private bool scanning;
    [ObservableProperty] private string scanError = "";
    [ObservableProperty] private bool isAutomatic = true;

    public ObservableCollection<WizardDeviceCandidate> Candidates { get; } = new();

    public WizardViewModel(Func<SaveConfigPayload, Task> sendConfigFunc)
    {
        _sendConfigFunc = sendConfigFunc;
    }

    partial void OnSourcePathChanged(string value)
    {
        OnPropertyChanged(nameof(IsSourcePathValid));
        NextCommand.NotifyCanExecuteChanged();
    }

    partial void OnSelectedDeviceChanged(WizardDeviceCandidate? value) =>
        NextCommand.NotifyCanExecuteChanged();

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

    public bool IsManual
    {
        get => !IsAutomatic;
        set { if (value) IsAutomatic = false; }
    }

    public bool IsWelcomeStep => CurrentStep == 1;
    public bool IsFolderStep => CurrentStep == 2;
    public bool IsDeviceStep => CurrentStep == 3;
    public bool IsSyncSettingsStep => CurrentStep == 4;
    public bool IsDoneStep => CurrentStep == 5;
    public bool ShowNextButton => CurrentStep < TotalSteps;
    public bool ShowFinishButton => CurrentStep == TotalSteps;
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

    public void ApplyInventory(DeviceInventoryEvent inventory) =>
        ApplyInventory(inventory.Devices, inventory.Unidentified);

    public void ApplyInventory(
        IEnumerable<IdentifiedDeviceSnapshot> identified,
        IEnumerable<UnidentifiedDeviceSnapshot> unidentified)
    {
        var selectedId = SelectedDevice?.DeviceId;
        var replacements = identified
            .Select(WizardDeviceCandidate.From)
            .OrderBy(candidate => candidate.DisplayName, StringComparer.CurrentCultureIgnoreCase)
            .Concat(unidentified
                .Select(WizardDeviceCandidate.From)
                .OrderBy(candidate => candidate.ObservationId))
            .ToArray();

        Candidates.Clear();
        foreach (var candidate in replacements) Candidates.Add(candidate);
        SelectedDevice = selectedId is null
            ? null
            : Candidates.FirstOrDefault(candidate => candidate.DeviceId == selectedId && candidate.CanAdopt);
        Scanning = false;
        ScanError = "";
    }

    public void BeginScanning() => Scanning = true;
    public void EndScanning() => Scanning = false;

    [RelayCommand]
    private void ClearCandidates()
    {
        Candidates.Clear();
        SelectedDevice = null;
        ScanError = "";
        Scanning = true;
    }

    private bool CanGoNext() => CurrentStep switch
    {
        1 => true,
        2 => IsSourcePathValid,
        3 => SelectedDevice?.CanAdopt == true,
        4 => true,
        _ => false,
    };

    [RelayCommand(CanExecute = nameof(CanGoNext))]
    private async Task NextAsync()
    {
        if (CurrentStep == 4)
        {
            try
            {
                await _sendConfigFunc(BuildPayload());
                ScanError = "";
                CurrentStep = 5;
            }
            catch (Exception exception)
            {
                ScanError = $"Couldn't save settings: {exception.Message}";
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
    private void Finish() => WizardFinished?.Invoke();

    private SaveConfigPayload BuildPayload() => new(
        SourcePath,
        SelectedDevice!.DeviceId!,
        IsAutomatic);

    public event Action? WizardFinished;
}
