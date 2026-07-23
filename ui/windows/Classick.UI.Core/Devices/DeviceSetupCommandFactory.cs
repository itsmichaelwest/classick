using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public sealed record DeviceSetupIntent(
    string Source,
    DeviceId DeviceId,
    bool AutoSync,
    TranscodeProfile TranscodeProfile = TranscodeProfile.Alac);

public static class DeviceSetupCommandFactory
{
    public static IReadOnlyList<WireCommand> Create(
        DeviceSetupIntent intent,
        Func<string> newId)
    {
        ArgumentNullException.ThrowIfNull(intent);
        ArgumentNullException.ThrowIfNull(newId);
        return
        [
            new SetSourceLocationCommand(newId(), intent.Source),
            new AdoptDeviceCommand(
                intent.DeviceId,
                newId(),
                newId(),
                new SelectionValue(1, SelectionMode.All, []),
                newId(),
                new SettingsValue(
                    1, intent.AutoSync, RockboxCompat: false, intent.TranscodeProfile),
                newId(),
                new SubscriptionsValue(1, [])),
        ];
    }
}
