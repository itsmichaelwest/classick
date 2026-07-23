using System.Collections.ObjectModel;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Classick_UI.ViewModels;

public sealed record PopoverDeviceChoice(DeviceId DeviceId, string Label);

public partial class PopoverViewModel
{
    public ObservableCollection<PopoverDeviceChoice> DeviceChoices { get; } = new();
    [ObservableProperty] private PopoverDeviceChoice? selectedDeviceChoice;
    [ObservableProperty] private bool hasMultipleDeviceChoices;
    private bool _updatingDeviceChoices;

    partial void OnHasMultipleDeviceChoicesChanged(bool value)
    {
        OnPropertyChanged(nameof(ShowConnectedContent));
        OnPropertyChanged(nameof(ShowEmptyState));
        OnPropertyChanged(nameof(ShowEjectButton));
    }

    public void UpdateDeviceChoices(IEnumerable<DeviceClientState> devices, DeviceId? focusedDeviceId)
    {
        var choices = devices
            .Select(device => new PopoverDeviceChoice(
                device.Inventory.DeviceId,
                DevicePresentationFactory.For(device.Inventory).Title))
            .OrderBy(choice => choice.Label, StringComparer.CurrentCultureIgnoreCase)
            .ThenBy(choice => choice.DeviceId.Value, StringComparer.Ordinal)
            .ToArray();

        _updatingDeviceChoices = true;
        try
        {
            if (!DeviceChoices.SequenceEqual(choices))
            {
                DeviceChoices.Clear();
                foreach (var choice in choices) DeviceChoices.Add(choice);
            }
            var selected = choices.FirstOrDefault(choice => choice.DeviceId == focusedDeviceId);
            if (SelectedDeviceChoice?.DeviceId != selected?.DeviceId)
                SelectedDeviceChoice = selected;
            HasMultipleDeviceChoices = choices.Length > 1;
        }
        finally
        {
            _updatingDeviceChoices = false;
        }
    }

    public DeviceId? ConsumeDeviceSelection() =>
        _updatingDeviceChoices ? null : SelectedDeviceChoice?.DeviceId;
}
