using Classick_UI.Devices;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

public sealed class DevicePresentationTests
{
    private static readonly DeviceId DeviceId = Classick_UI.Ipc.DeviceId.Parse("000A27002138B0A8");

    [Fact]
    public void FriendlyNameWinsAndCertainCataloguedFactsSelectExactArtwork()
    {
        var presentation = DevicePresentationFactory.For(Device(
            DeviceReadiness.Ready,
            ProfileStatus.NotAdopted,
            hardware: new HardwareFacts(
                Family: Fact(IpodFamily.Classic, FactSource.Decoded),
                Generation: Fact("Late 2009", FactSource.Decoded),
                ModelCode: Fact("MC293", FactSource.Reported),
                Colour: Fact(IpodColour.Silver, FactSource.Decoded))));

        Assert.Equal("Michael's iPod", presentation.Title);
        Assert.Contains("Late 2009", presentation.HardwareSummary);
        Assert.Contains("MC293", presentation.HardwareSummary);
        Assert.Contains("Model: reported, certain", presentation.HardwareProvenance);
        Assert.Contains("Colour: decoded, certain", presentation.HardwareProvenance);
        Assert.True(presentation.CanAdopt);
        Assert.Equal(DeviceArtworkSpecificity.Exact, presentation.Artwork.Specificity);
        Assert.EndsWith("/ipod.svg", presentation.Artwork.AssetUri, StringComparison.Ordinal);
    }

    [Fact]
    public void MissingOrHeuristicColourUsesGenericArtworkWithoutDefaultingToSilver()
    {
        var presentation = DevicePresentationFactory.For(Device(
            DeviceReadiness.Ready,
            ProfileStatus.NotAdopted,
            hardware: new HardwareFacts(
                Family: Fact(IpodFamily.Classic, FactSource.Decoded),
                ModelCode: Fact("MC293", FactSource.Reported),
                Colour: new HardwareFact<IpodColour>(
                    IpodColour.Silver,
                    FactSource.Inferred,
                    FactConfidence.Heuristic))));

        Assert.Equal(DeviceArtworkSpecificity.Generic, presentation.Artwork.Specificity);
        Assert.EndsWith("/ipod-generic.svg", presentation.Artwork.AssetUri, StringComparison.Ordinal);
        Assert.DoesNotContain("silver", presentation.Artwork.AccessibleDescription, StringComparison.OrdinalIgnoreCase);
    }

    [Theory]
    [InlineData(DeviceReadiness.NeedsAppleInitialization, "Apple setup required", "does not initialize")]
    [InlineData(DeviceReadiness.InvalidDatabase, "iPod database is invalid", "Apple software")]
    public void UnsafeReadinessIsVisibleButNotAdoptable(
        DeviceReadiness readiness,
        string status,
        string guidance)
    {
        var presentation = DevicePresentationFactory.For(Device(readiness, ProfileStatus.NotAdopted));

        Assert.False(presentation.CanAdopt);
        Assert.Equal(status, presentation.Status);
        Assert.Contains(guidance, presentation.Guidance);
    }

    [Fact]
    public void UnidentifiedObservationNeverProducesAnAdoptableTarget()
    {
        var presentation = DevicePresentationFactory.For(new UnidentifiedDeviceSnapshot(
            7,
            DeviceReadiness.IdentityUnavailable,
            new HardwareFacts(Family: Fact(IpodFamily.Classic, FactSource.Inferred, FactConfidence.Heuristic))));

        Assert.False(presentation.CanAdopt);
        Assert.Contains("cannot safely target", presentation.Guidance);
        Assert.Contains("administrator access is not required", presentation.Guidance);
    }

    [Fact]
    public void PendingAdoptionIsNotOfferedAsANewSetupTarget()
    {
        var presentation = DevicePresentationFactory.For(Device(
            DeviceReadiness.Ready,
            ProfileStatus.PendingAdoption));

        Assert.False(presentation.CanAdopt);
        Assert.Equal("Classick setup pending", presentation.Status);
    }

    [Theory]
    [InlineData(DeviceReadiness.NeedsAppleInitialization, "Apple setup required")]
    [InlineData(DeviceReadiness.InvalidDatabase, "database is invalid")]
    public void DisconnectedDevicePreservesUnsafeReadiness(DeviceReadiness readiness, string expected)
    {
        var device = Device(readiness, ProfileStatus.NotAdopted) with
        {
            Connected = false,
            MountPath = null,
            Phase = DevicePhase.Disconnected,
        };

        var presentation = DevicePresentationFactory.For(device);

        Assert.Contains(expected, presentation.Status);
        Assert.Contains("not connected", presentation.Status);
        Assert.False(presentation.CanAdopt);
    }

    private static IdentifiedDeviceSnapshot Device(
        DeviceReadiness readiness,
        ProfileStatus profile,
        HardwareFacts? hardware = null) => new(
            DeviceId,
            "Michael's iPod",
            readiness,
            hardware ?? new HardwareFacts(),
            profile,
            true,
            "D:\\",
            DevicePhase.Unconfigured,
            null,
            null,
            0,
            null,
            null);

    private static HardwareFact<T> Fact<T>(
        T value,
        FactSource source,
        FactConfidence confidence = FactConfidence.Certain) => new(value, source, confidence);
}
