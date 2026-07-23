using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DeviceSetupCommandFactoryTests
{
    [Fact]
    public void SetupUsesDeviceIdAndContainsNoAppearanceMetadata()
    {
        var ids = new Queue<string>(new[]
        {
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8901",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8902",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8903",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8904",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8905",
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8906",
        });
        var deviceId = DeviceId.Parse("000A27002138B0A8");
        var commands = DeviceSetupCommandFactory.Create(
            new DeviceSetupIntent(
                "D:\\Music", deviceId, false, TranscodeProfile.Aac128),
            ids.Dequeue);

        Assert.Collection(
            commands,
            command => Assert.IsType<SetSourceLocationCommand>(command),
            command =>
            {
                var adopt = Assert.IsType<AdoptDeviceCommand>(command);
                Assert.Equal(deviceId, adopt.DeviceId);
                Assert.False(adopt.Settings.AutoSync);
                Assert.Equal(TranscodeProfile.Aac128, adopt.Settings.TranscodeProfile);
                var json = WireCodec.Encode(adopt);
                Assert.DoesNotContain("colour", json, StringComparison.Ordinal);
                Assert.DoesNotContain("model", json, StringComparison.Ordinal);
                Assert.DoesNotContain("generation", json, StringComparison.Ordinal);
                Assert.DoesNotContain("name", json, StringComparison.Ordinal);
            });
    }

    [Fact]
    public void SetupDoesNotMutateGlobalSyncPolicy()
    {
        var commands = DeviceSetupCommandFactory.Create(
            new DeviceSetupIntent("D:\\Music", DeviceId.Parse("000A27002138B0A8"), true),
            () => Guid.NewGuid().ToString("D"));

        Assert.DoesNotContain(commands, command => command is SetGlobalSettingsCommand);
        Assert.True(Assert.IsType<AdoptDeviceCommand>(commands[1]).Settings.AutoSync);
    }
}
